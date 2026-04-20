pub mod avi;
pub mod bmp;
pub mod bzip2;
pub mod elf;
pub mod eml;
pub mod fb2;
pub mod footer;
pub mod gif;
pub mod gzip;
pub mod heic;
pub mod ico;
pub mod jpeg;
pub mod lrf;
pub mod mobi;
pub mod mov;
pub mod mp3;
pub mod mp4;
pub mod ogg;
pub mod ole;
pub mod pdf;
pub mod png;
pub mod rar;
pub mod riff;
pub mod rtf;
pub mod sevenz;
pub mod sqlite;
pub mod sqlite_page;
pub mod sqlite_wal;
pub mod tar;
pub mod tiff;
pub mod wav;
pub mod webm;
pub mod webp;
pub mod windows;
pub mod wmv;
pub mod xz;
pub mod zip;

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

/// Result of lightweight pre-validation before disk I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreValidation {
    /// Candidate looks plausible — proceed to full carving.
    Proceed,
    /// Candidate is definitely invalid — skip without disk I/O.
    Reject(String),
}

/// Metadata about a carved file.
///
/// # Example
/// ```rust
/// use swiftbeaver::carve::CarvedFile;
///
/// let file = CarvedFile {
///     run_id: "example_run".to_string(),
///     file_type: "jpeg".to_string(),
///     path: "jpeg/jpeg_000000001000.jpg".to_string(),
///     extension: "jpg".to_string(),
///     global_start: 4096,
///     global_end: 8191,
///     size: 4096,
///     md5: None,
///     sha256: Some("deadbeef".to_string()),
///     validated: true,
///     truncated: false,
///     errors: Vec::new(),
///     pattern_id: Some("jpeg_soi".to_string()),
///     is_duplicate: false,
///     duplicate_of_offset: None,
/// };
/// let _ = file;
/// ```
#[derive(Debug, Clone, Serialize)]
pub struct CarvedFile {
    pub run_id: String,
    pub file_type: String,
    pub path: String,
    pub extension: String,
    pub global_start: u64,
    pub global_end: u64,
    pub size: u64,
    pub md5: Option<String>,
    pub sha256: Option<String>,
    pub validated: bool,
    pub truncated: bool,
    pub errors: Vec<String>,
    pub pattern_id: Option<String>,
    pub is_duplicate: bool,
    pub duplicate_of_offset: Option<u64>,
}

/// A carved file whose data has been hashed but not yet materialized on disk.
/// The carve worker decides whether to flush (non-duplicate) or discard (duplicate).
pub struct PendingCarve {
    pub file: CarvedFile,
    writer: DeferredWriter,
}

impl PendingCarve {
    pub(crate) fn new(file: CarvedFile, writer: DeferredWriter) -> Self {
        Self { file, writer }
    }

    /// Materialize the file on disk. Call for non-duplicate files.
    pub fn flush(mut self) -> Result<CarvedFile, CarveError> {
        if let Err(e) = self.writer.flush_to_disk() {
            self.writer.discard();
            return Err(e);
        }
        Ok(self.file)
    }

    /// Discard without writing (or remove if already materialized). Call for duplicates.
    pub fn discard(mut self) -> CarvedFile {
        self.writer.discard();
        self.file
    }
}

pub struct ExtractionContext<'a> {
    pub run_id: &'a str,
    pub output_root: &'a Path,
    pub evidence: &'a dyn EvidenceSource,
    pub deferred_buffer_bytes: usize,
    /// When true, skip all file I/O (metadata-only mode).
    pub metadata_only: bool,
    /// Hash algorithm selection for CarveStream-based carvers.
    pub hash_config: crate::hash::HashConfig,
    /// Per-worker reusable I/O buffer for carve operations.
    /// Persists across hits within the same worker thread.
    pub(crate) io_buf: RefCell<Vec<u8>>,
    /// Scan chunk buffer for the current hit (set per-hit by carve worker).
    pub(crate) chunk_data: Option<Arc<Vec<u8>>>,
    /// Global byte offset where chunk_data begins.
    pub(crate) chunk_start: u64,
}

impl<'a> std::fmt::Debug for ExtractionContext<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtractionContext")
            .field("run_id", &self.run_id)
            .field("output_root", &self.output_root)
            .field("evidence", &"<dyn EvidenceSource>")
            .field("deferred_buffer_bytes", &self.deferred_buffer_bytes)
            .field("metadata_only", &self.metadata_only)
            .field("hash_config", &self.hash_config)
            .field("io_buf_capacity", &self.io_buf.borrow().capacity())
            .field("chunk_data_len", &self.chunk_data.as_ref().map(|d| d.len()))
            .field("chunk_start", &self.chunk_start)
            .finish()
    }
}

impl<'a> ExtractionContext<'a> {
    pub fn new(
        run_id: &'a str,
        output_root: &'a Path,
        evidence: &'a dyn EvidenceSource,
        deferred_buffer_bytes: usize,
    ) -> Self {
        Self {
            run_id,
            output_root,
            evidence,
            deferred_buffer_bytes,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
            io_buf: RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
        }
    }
}

#[derive(Debug, Error)]
pub enum CarveError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("evidence error: {0}")]
    Evidence(String),
    #[error("invalid format: {0}")]
    Invalid(String),
    #[error("truncated output")]
    Truncated,
    #[error("unexpected eof")]
    Eof,
}

pub trait CarveHandler: Send + Sync {
    fn file_type(&self) -> &str;
    fn extension(&self) -> &str;

    /// Whether this carver is "fast" (small fixed-size reads, typically < 10 MB).
    /// Fast carvers are routed to a dedicated high-throughput worker pool so they
    /// are not blocked behind slow, I/O-heavy carvers like SQLite or MP3.
    /// Default: `false` (conservative — unknown carvers go to the slow pool).
    fn is_fast(&self) -> bool {
        false
    }

    /// Quick in-memory validation of a signature hit.
    /// Default returns `Proceed` (backward compatible).
    /// Implementations should read minimal bytes from evidence
    /// and validate magic/structure before any disk I/O.
    fn pre_validate(
        &self,
        _evidence: &dyn EvidenceSource,
        _offset: u64,
    ) -> Result<PreValidation, CarveError> {
        Ok(PreValidation::Proceed)
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<PendingCarve>, CarveError>;
}

pub struct CarveRegistry {
    handlers: HashMap<String, Box<dyn CarveHandler>>,
}

impl CarveRegistry {
    pub fn new(handlers: HashMap<String, Box<dyn CarveHandler>>) -> Self {
        Self { handlers }
    }

    pub fn get(&self, file_type_id: &str) -> Option<&dyn CarveHandler> {
        self.handlers.get(file_type_id).map(|h| h.as_ref())
    }

    /// Check whether the carver for `file_type_id` is classified as fast.
    pub fn is_fast(&self, file_type_id: &str) -> bool {
        self.handlers.get(file_type_id).is_some_and(|h| h.is_fast())
    }
}

pub fn output_path(
    output_root: &Path,
    file_type: &str,
    extension: &str,
    global_start: u64,
) -> Result<(PathBuf, String), CarveError> {
    let safe_type = sanitize_component(file_type);
    let safe_ext = sanitize_extension(extension);
    let dir = output_root.join(&safe_type);
    let base = format!("{safe_type}_{global_start:012X}");
    let filename = if safe_ext.is_empty() {
        base
    } else {
        format!("{base}.{safe_ext}")
    };
    let full_path = dir.join(&filename);
    let rel_path = full_path
        .strip_prefix(output_root)
        .unwrap_or(&full_path)
        .to_string_lossy()
        .to_string();
    Ok((full_path, rel_path))
}

fn sanitize_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    while out.contains("..") {
        out = out.replace("..", "_");
    }
    let trimmed = out.trim_matches('.').to_string();
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed
    }
}

pub fn sanitize_extension(ext: &str) -> String {
    sanitize_component(ext)
        .trim_start_matches('.')
        .to_ascii_lowercase()
}

/// Helper to build a CarvedFile result, reducing boilerplate in handlers
#[allow(clippy::too_many_arguments)]
pub fn build_carved_file(
    run_id: &str,
    file_type: &str,
    extension: &str,
    rel_path: String,
    global_start: u64,
    size: u64,
    md5_hex: Option<String>,
    sha256_hex: Option<String>,
    validated: bool,
    truncated: bool,
    errors: Vec<String>,
    pattern_id: &str,
) -> CarvedFile {
    let global_end = if size == 0 {
        global_start
    } else {
        global_start + size - 1
    };

    CarvedFile {
        run_id: run_id.to_string(),
        file_type: file_type.to_string(),
        path: rel_path,
        extension: extension.to_string(),
        global_start,
        global_end,
        size,
        md5: md5_hex,
        sha256: sha256_hex,
        validated,
        truncated,
        errors,
        pattern_id: Some(pattern_id.to_string()),
        is_duplicate: false,
        duplicate_of_offset: None,
    }
}

/// Deferred file writer that buffers initial bytes in memory before creating
/// an output file on disk. This eliminates the create-write-delete I/O waste
/// for candidates that fail structural validation during carving.
///
/// When `buffer_limit` is 0, the file is created on the first write (no buffering).
pub(crate) struct DeferredWriter {
    path: PathBuf,
    buffer_limit: usize,
    buffer: Vec<u8>,
    writer: Option<BufWriter<File>>,
    /// When true, all I/O operations are skipped (metadata-only mode).
    skip_io: bool,
    /// Whether a file has been created on disk (buffer overflow or flush).
    materialized: bool,
}

impl DeferredWriter {
    pub(crate) fn new(path: PathBuf, buffer_limit: usize, skip_io: bool) -> Self {
        Self {
            path,
            buffer_limit,
            buffer: Vec::new(),
            writer: None,
            skip_io,
            materialized: false,
        }
    }

    pub(crate) fn update_path(&mut self, new_path: PathBuf) {
        self.path = new_path;
    }

    pub(crate) fn write_all(&mut self, data: &[u8]) -> Result<(), CarveError> {
        if self.skip_io {
            return Ok(());
        }
        if let Some(ref mut writer) = self.writer {
            writer.write_all(data)?;
        } else if self.buffer.len() + data.len() <= self.buffer_limit {
            self.buffer.extend_from_slice(data);
        } else {
            // Transition: create file, flush buffer, write new data
            if let Some(parent) = self.path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut writer = BufWriter::new(File::create(&self.path)?);
            if !self.buffer.is_empty() {
                writer.write_all(&self.buffer)?;
                self.buffer = Vec::new();
            }
            writer.write_all(data)?;
            self.writer = Some(writer);
            self.materialized = true;
        }
        Ok(())
    }

    /// Materialize the file (if still buffering with data) and flush.
    pub(crate) fn flush_to_disk(&mut self) -> Result<(), CarveError> {
        if self.skip_io {
            return Ok(());
        }
        if let Some(ref mut writer) = self.writer {
            writer.flush()?;
        } else if !self.buffer.is_empty() {
            if let Some(parent) = self.path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut file = File::create(&self.path)?;
            file.write_all(&self.buffer)?;
            file.flush()?;
            self.materialized = true;
        }
        Ok(())
    }

    /// Discard buffered data without creating a file. If the file was already
    /// created (buffer overflowed), close and remove it.
    pub(crate) fn discard(&mut self) {
        if self.skip_io {
            return;
        }
        if let Some(writer) = self.writer.take() {
            drop(writer);
        }
        self.buffer.clear();
        if self.materialized {
            let _ = std::fs::remove_file(&self.path);
            self.materialized = false;
        }
    }
}

/// Read from chunk data if available and covers the range, otherwise from evidence.
fn read_from_ctx(
    ctx: &ExtractionContext,
    offset: u64,
    buf: &mut [u8],
) -> Result<usize, CarveError> {
    if let Some(chunk) = &ctx.chunk_data
        && offset >= ctx.chunk_start
    {
        let local = (offset - ctx.chunk_start) as usize;
        if local < chunk.len() {
            let available = chunk.len() - local;
            let n = buf.len().min(available);
            buf[..n].copy_from_slice(&chunk[local..local + n]);
            return Ok(n);
        }
    }
    ctx.evidence
        .read_at(offset, buf)
        .map_err(|e| CarveError::Evidence(e.to_string()))
}

pub(crate) struct CarveStream<'a> {
    ctx: &'a ExtractionContext<'a>,
    offset: u64,
    max_size: u64,
    written: u64,
    writer: Option<DeferredWriter>,
    md5: Option<md5::Context>,
    sha256: Option<Sha256>,
    reuse_buf: Vec<u8>,
}

impl<'a> CarveStream<'a> {
    pub(crate) fn new(
        ctx: &'a ExtractionContext<'a>,
        offset: u64,
        max_size: u64,
        path: PathBuf,
    ) -> Self {
        let reuse_buf = std::mem::take(&mut *ctx.io_buf.borrow_mut());
        let md5 = if ctx.hash_config.has_md5() {
            Some(md5::Context::new())
        } else {
            None
        };
        let sha256 = if ctx.hash_config.has_sha256() {
            Some(Sha256::new())
        } else {
            None
        };
        Self {
            ctx,
            offset,
            max_size,
            written: 0,
            writer: Some(DeferredWriter::new(
                path,
                ctx.deferred_buffer_bytes,
                ctx.metadata_only,
            )),
            md5,
            sha256,
            reuse_buf,
        }
    }

    /// Read from chunk data if the requested range is covered, otherwise fall back to evidence.
    fn read_from_source(&self, offset: u64, buf: &mut [u8]) -> Result<usize, CarveError> {
        read_from_ctx(self.ctx, offset, buf)
    }

    pub(crate) fn read_exact(&mut self, len: usize) -> Result<Vec<u8>, CarveError> {
        if self.max_size > 0 && self.written.saturating_add(len as u64) > self.max_size {
            return Err(CarveError::Truncated);
        }

        let mut buf = std::mem::take(&mut self.reuse_buf);
        buf.clear();
        buf.resize(len, 0);
        let mut read = 0usize;
        while read < len {
            let n = self.read_from_source(self.offset, &mut buf[read..])?;
            if n == 0 {
                self.reuse_buf = buf;
                return Err(CarveError::Eof);
            }
            self.write_bytes(&buf[read..read + n])?;
            read += n;
        }

        let result = buf.clone();
        self.reuse_buf = buf;
        Ok(result)
    }

    pub(crate) fn write_bytes(&mut self, buf: &[u8]) -> Result<(), CarveError> {
        if self.max_size > 0 && self.written.saturating_add(buf.len() as u64) > self.max_size {
            return Err(CarveError::Truncated);
        }
        self.writer
            .as_mut()
            .ok_or_else(|| CarveError::Invalid("stream already finalized".into()))?
            .write_all(buf)?;
        if let Some(ref mut md5) = self.md5 {
            md5.consume(buf);
        }
        if let Some(ref mut sha256) = self.sha256 {
            sha256.update(buf);
        }
        self.offset = self.offset.saturating_add(buf.len() as u64);
        self.written = self.written.saturating_add(buf.len() as u64);
        Ok(())
    }

    /// Finalize hashes and return the unflushed writer.
    /// The caller decides whether to flush or discard.
    pub(crate) fn finalize(
        mut self,
    ) -> Result<(u64, Option<String>, Option<String>, DeferredWriter), CarveError> {
        let writer = self
            .writer
            .take()
            .ok_or_else(|| CarveError::Invalid("stream already finalized".into()))?;
        let md5 = self.md5.take().map(|ctx| format!("{:x}", ctx.compute()));
        let sha256 = self.sha256.take().map(|ctx| hex::encode(ctx.finalize()));
        Ok((self.written, md5, sha256, writer))
    }

    /// Discard the stream without creating a file (for validation failures).
    pub(crate) fn discard(mut self) {
        if let Some(ref mut w) = self.writer {
            w.discard();
        }
    }

    /// Get the number of bytes written so far
    pub(crate) fn bytes_written(&self) -> u64 {
        self.written
    }

    /// Read and hash `len` bytes in fixed-size chunks without allocating the full buffer.
    /// Use this for the "consume remainder" case where the returned data isn't needed.
    pub(crate) fn consume_remaining(&mut self, len: u64) -> Result<(), CarveError> {
        if self.max_size > 0 && self.written.saturating_add(len) > self.max_size {
            return Err(CarveError::Truncated);
        }

        let chunk_size = 64 * 1024;
        let mut buf = std::mem::take(&mut self.reuse_buf);
        if buf.len() < chunk_size {
            buf.resize(chunk_size, 0);
        }
        let mut left = len;
        while left > 0 {
            let to_read = (left as usize).min(chunk_size);
            let n = self.read_from_source(self.offset, &mut buf[..to_read])?;
            if n == 0 {
                self.reuse_buf = buf;
                return Err(CarveError::Eof);
            }
            self.write_bytes(&buf[..n])?;
            left = left.saturating_sub(n as u64);
        }
        self.reuse_buf = buf;
        Ok(())
    }

    /// Read data from current offset without advancing or writing.
    /// Used to peek ahead for validation without consuming data.
    pub(crate) fn peek_exact(&self, len: usize) -> Result<Vec<u8>, CarveError> {
        let mut buf = vec![0u8; len];
        let mut read = 0usize;
        let mut offset = self.offset;
        while read < len {
            let n = self.read_from_source(offset, &mut buf[read..])?;
            if n == 0 {
                return Err(CarveError::Eof);
            }
            read += n;
            offset += n as u64;
        }
        Ok(buf)
    }
}

impl Drop for CarveStream<'_> {
    fn drop(&mut self) {
        let buf = std::mem::take(&mut self.reuse_buf);
        if !buf.is_empty() || buf.capacity() > 0 {
            *self.ctx.io_buf.borrow_mut() = buf;
        }
    }
}

/// Create optional hash contexts based on configuration.
pub(crate) fn create_hashers(
    config: &crate::hash::HashConfig,
) -> (Option<md5::Context>, Option<Sha256>) {
    let md5 = if config.has_md5() {
        Some(md5::Context::new())
    } else {
        None
    };
    let sha256 = if config.has_sha256() {
        Some(Sha256::new())
    } else {
        None
    };
    (md5, sha256)
}

/// Finalize optional hash contexts into hex strings.
pub(crate) fn finalize_hashers(
    md5: Option<md5::Context>,
    sha256: Option<Sha256>,
) -> (Option<String>, Option<String>) {
    let md5_hex = md5.map(|c| format!("{:x}", c.compute()));
    let sha256_hex = sha256.map(|s| hex::encode(s.finalize()));
    (md5_hex, sha256_hex)
}

pub(crate) fn write_range(
    ctx: &ExtractionContext,
    start: u64,
    end: u64,
    path: &Path,
    mut md5: Option<&mut md5::Context>,
    mut sha256: Option<&mut Sha256>,
) -> Result<(u64, bool, DeferredWriter), CarveError> {
    let mut writer = DeferredWriter::new(
        path.to_path_buf(),
        ctx.deferred_buffer_bytes,
        ctx.metadata_only,
    );
    let mut offset = start;
    let mut remaining = end.saturating_sub(start);
    let mut bytes_written = 0u64;
    let buf_size = 64 * 1024;
    let mut buf = vec![0u8; buf_size];

    while remaining > 0 {
        let read_len = remaining.min(buf_size as u64) as usize;
        let n = read_from_ctx(ctx, offset, &mut buf[..read_len])?;
        if n == 0 {
            return Ok((bytes_written, true, writer));
        }
        writer.write_all(&buf[..n])?;
        if let Some(ref mut m) = md5 {
            m.consume(&buf[..n]);
        }
        if let Some(ref mut s) = sha256 {
            s.update(&buf[..n]);
        }
        bytes_written = bytes_written.saturating_add(n as u64);
        offset = offset.saturating_add(n as u64);
        remaining = remaining.saturating_sub(n as u64);
    }

    Ok((bytes_written, false, writer))
}

#[cfg(test)]
mod tests {
    use super::{DeferredWriter, output_path, sanitize_component, sanitize_extension};
    use tempfile::tempdir;

    #[test]
    fn sanitizes_output_path_components() {
        let dir = tempdir().expect("tempdir");
        let (full, rel) =
            output_path(dir.path(), "../weird", "../JPG", 0x1234).expect("output path");
        assert!(full.starts_with(dir.path()));
        assert!(!rel.contains(".."));
        assert!(sanitize_component("../weird").contains("weird"));
    }

    #[test]
    fn sanitizes_extension() {
        assert_eq!(sanitize_extension(".JPG"), "jpg");
        assert_eq!(sanitize_extension("..bad"), "_bad");
    }

    #[test]
    fn deferred_writer_buffer_only_path() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("small.bin");
        let data = b"hello world";

        let mut writer = DeferredWriter::new(path.clone(), 1024, false);
        writer.write_all(data).expect("write");
        // File should NOT exist yet (still buffering)
        assert!(!path.exists());
        writer.flush_to_disk().expect("flush");
        // File should now exist with correct content
        assert_eq!(std::fs::read(&path).expect("read"), data);
    }

    #[test]
    fn deferred_writer_transition_to_streaming() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("large.bin");
        let limit = 16;

        let mut writer = DeferredWriter::new(path.clone(), limit, false);
        // Write within limit
        writer.write_all(&[0xAA; 10]).expect("write1");
        assert!(!path.exists());
        // Write beyond limit — triggers file creation
        writer.write_all(&[0xBB; 10]).expect("write2");
        assert!(path.exists());
        // Write more
        writer.write_all(&[0xCC; 5]).expect("write3");
        writer.flush_to_disk().expect("flush");

        let content = std::fs::read(&path).expect("read");
        assert_eq!(content.len(), 25);
        assert_eq!(&content[..10], &[0xAA; 10]);
        assert_eq!(&content[10..20], &[0xBB; 10]);
        assert_eq!(&content[20..25], &[0xCC; 5]);
    }

    #[test]
    fn deferred_writer_discard_while_buffering() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("discarded.bin");

        let mut writer = DeferredWriter::new(path.clone(), 1024, false);
        writer.write_all(b"data").expect("write");
        assert!(!path.exists());
        writer.discard();
        assert!(!path.exists());
    }

    #[test]
    fn deferred_writer_discard_while_streaming() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("discarded_streaming.bin");

        let mut writer = DeferredWriter::new(path.clone(), 4, false);
        writer.write_all(b"exceeds limit").expect("write");
        assert!(path.exists());
        writer.discard();
        // File should be removed by discard
        assert!(!path.exists());
    }

    #[test]
    fn deferred_writer_empty_discard() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("empty.bin");

        let mut writer = DeferredWriter::new(path.clone(), 1024, false);
        writer.discard();
        assert!(!path.exists());
    }

    #[test]
    fn deferred_writer_empty_flush_no_file() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("empty.bin");

        let mut writer = DeferredWriter::new(path.clone(), 1024, false);
        writer.flush_to_disk().expect("flush");
        // No data written → no file should be created
        assert!(!path.exists());
    }

    #[test]
    fn deferred_writer_zero_limit_immediate_streaming() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("eager.bin");

        let mut writer = DeferredWriter::new(path.clone(), 0, false);
        // With limit=0, any data triggers immediate file creation
        writer.write_all(b"eager").expect("write");
        assert!(path.exists());
        writer.flush_to_disk().expect("flush");
        assert_eq!(std::fs::read(&path).expect("read"), b"eager");
    }

    #[test]
    fn deferred_writer_skip_io_no_file_created() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("skipped.bin");

        let mut writer = DeferredWriter::new(path.clone(), 1024, true);
        writer.write_all(b"should not be written").expect("write");
        writer.flush_to_disk().expect("flush");
        assert!(!path.exists(), "skip_io should prevent file creation");
    }

    #[test]
    fn deferred_writer_skip_io_discard_is_noop() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("skipped_discard.bin");

        let mut writer = DeferredWriter::new(path.clone(), 1024, true);
        writer.write_all(b"data").expect("write");
        writer.discard();
        assert!(!path.exists());
    }

    #[test]
    fn carve_registry_is_fast_delegates_to_handler() {
        use super::{CarveError, CarveHandler, CarveRegistry, NormalizedHit, PendingCarve};

        struct FastHandler;
        impl CarveHandler for FastHandler {
            fn file_type(&self) -> &str {
                "bmp"
            }
            fn extension(&self) -> &str {
                "bmp"
            }
            fn is_fast(&self) -> bool {
                true
            }
            fn process_hit(
                &self,
                _: &NormalizedHit,
                _: &super::ExtractionContext,
            ) -> Result<Option<PendingCarve>, CarveError> {
                Ok(None)
            }
        }

        struct SlowHandler;
        impl CarveHandler for SlowHandler {
            fn file_type(&self) -> &str {
                "sqlite"
            }
            fn extension(&self) -> &str {
                "sqlite"
            }
            // is_fast() defaults to false
            fn process_hit(
                &self,
                _: &NormalizedHit,
                _: &super::ExtractionContext,
            ) -> Result<Option<PendingCarve>, CarveError> {
                Ok(None)
            }
        }

        let mut handlers: std::collections::HashMap<String, Box<dyn CarveHandler>> =
            std::collections::HashMap::new();
        handlers.insert("bmp".to_string(), Box::new(FastHandler));
        handlers.insert("sqlite".to_string(), Box::new(SlowHandler));

        let registry = CarveRegistry::new(handlers);
        assert!(registry.is_fast("bmp"));
        assert!(!registry.is_fast("sqlite"));
        assert!(!registry.is_fast("nonexistent"));
    }
}
