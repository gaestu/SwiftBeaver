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
}

pub struct ExtractionContext<'a> {
    pub run_id: &'a str,
    pub output_root: &'a Path,
    pub evidence: &'a dyn EvidenceSource,
    pub deferred_buffer_bytes: usize,
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
    ) -> Result<Option<CarvedFile>, CarveError>;
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
    std::fs::create_dir_all(&dir)?;
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
    md5_hex: String,
    sha256_hex: String,
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
        md5: Some(md5_hex),
        sha256: Some(sha256_hex),
        validated,
        truncated,
        errors,
        pattern_id: Some(pattern_id.to_string()),
    }
}

/// Check if carved size meets minimum requirement, delete file if not
pub fn check_min_size(full_path: &Path, size: u64, min_size: u64) -> bool {
    if size < min_size {
        let _ = std::fs::remove_file(full_path);
        false
    } else {
        true
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
}

impl DeferredWriter {
    pub(crate) fn new(path: PathBuf, buffer_limit: usize) -> Self {
        Self {
            path,
            buffer_limit,
            buffer: Vec::new(),
            writer: None,
        }
    }

    pub(crate) fn write_all(&mut self, data: &[u8]) -> Result<(), CarveError> {
        if let Some(ref mut writer) = self.writer {
            writer.write_all(data)?;
        } else if self.buffer.len() + data.len() <= self.buffer_limit {
            self.buffer.extend_from_slice(data);
        } else {
            // Transition: create file, flush buffer, write new data
            let mut writer = BufWriter::new(File::create(&self.path)?);
            if !self.buffer.is_empty() {
                writer.write_all(&self.buffer)?;
                self.buffer = Vec::new();
            }
            writer.write_all(data)?;
            self.writer = Some(writer);
        }
        Ok(())
    }

    /// Materialize the file (if still buffering with data) and flush.
    pub(crate) fn flush_to_disk(&mut self) -> Result<(), CarveError> {
        if let Some(ref mut writer) = self.writer {
            writer.flush()?;
        } else if !self.buffer.is_empty() {
            let mut file = File::create(&self.path)?;
            file.write_all(&self.buffer)?;
            file.flush()?;
        }
        Ok(())
    }

    /// Discard buffered data without creating a file. If the file was already
    /// created (buffer overflowed), close and remove it.
    pub(crate) fn discard(&mut self) {
        if let Some(writer) = self.writer.take() {
            drop(writer);
            let _ = std::fs::remove_file(&self.path);
        }
        self.buffer.clear();
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
    writer: DeferredWriter,
    md5: md5::Context,
    sha256: Sha256,
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
        Self {
            ctx,
            offset,
            max_size,
            written: 0,
            writer: DeferredWriter::new(path, ctx.deferred_buffer_bytes),
            md5: md5::Context::new(),
            sha256: Sha256::new(),
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
        self.writer.write_all(buf)?;
        self.md5.consume(buf);
        self.sha256.update(buf);
        self.offset = self.offset.saturating_add(buf.len() as u64);
        self.written = self.written.saturating_add(buf.len() as u64);
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<(u64, String, String), CarveError> {
        self.writer.flush_to_disk()?;
        let md5_ctx = std::mem::replace(&mut self.md5, md5::Context::new());
        let sha_ctx = std::mem::replace(&mut self.sha256, Sha256::new());
        let md5 = format!("{:x}", md5_ctx.compute());
        let sha256 = hex::encode(sha_ctx.finalize());
        Ok((self.written, md5, sha256))
    }

    /// Discard the stream without creating a file (for validation failures).
    pub(crate) fn discard(mut self) {
        self.writer.discard();
    }

    /// Get the number of bytes written so far
    pub(crate) fn bytes_written(&self) -> u64 {
        self.written
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

pub(crate) fn write_range(
    ctx: &ExtractionContext,
    start: u64,
    end: u64,
    path: &Path,
    md5: &mut md5::Context,
    sha256: &mut Sha256,
) -> Result<(u64, bool), CarveError> {
    let mut writer = DeferredWriter::new(path.to_path_buf(), ctx.deferred_buffer_bytes);
    let mut offset = start;
    let mut remaining = end.saturating_sub(start);
    let mut bytes_written = 0u64;
    let buf_size = 64 * 1024;
    let mut buf = vec![0u8; buf_size];

    while remaining > 0 {
        let read_len = remaining.min(buf_size as u64) as usize;
        let n = read_from_ctx(ctx, offset, &mut buf[..read_len])?;
        if n == 0 {
            writer.flush_to_disk()?;
            return Ok((bytes_written, true));
        }
        writer.write_all(&buf[..n])?;
        md5.consume(&buf[..n]);
        sha256.update(&buf[..n]);
        bytes_written = bytes_written.saturating_add(n as u64);
        offset = offset.saturating_add(n as u64);
        remaining = remaining.saturating_sub(n as u64);
        if n < read_len {
            writer.flush_to_disk()?;
            return Ok((bytes_written, true));
        }
    }

    writer.flush_to_disk()?;
    Ok((bytes_written, false))
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

        let mut writer = DeferredWriter::new(path.clone(), 1024);
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

        let mut writer = DeferredWriter::new(path.clone(), limit);
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

        let mut writer = DeferredWriter::new(path.clone(), 1024);
        writer.write_all(b"data").expect("write");
        assert!(!path.exists());
        writer.discard();
        assert!(!path.exists());
    }

    #[test]
    fn deferred_writer_discard_while_streaming() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("discarded_streaming.bin");

        let mut writer = DeferredWriter::new(path.clone(), 4);
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

        let mut writer = DeferredWriter::new(path.clone(), 1024);
        writer.discard();
        assert!(!path.exists());
    }

    #[test]
    fn deferred_writer_empty_flush_no_file() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("empty.bin");

        let mut writer = DeferredWriter::new(path.clone(), 1024);
        writer.flush_to_disk().expect("flush");
        // No data written → no file should be created
        assert!(!path.exists());
    }

    #[test]
    fn deferred_writer_zero_limit_immediate_streaming() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("eager.bin");

        let mut writer = DeferredWriter::new(path.clone(), 0);
        // With limit=0, any data triggers immediate file creation
        writer.write_all(b"eager").expect("write");
        assert!(path.exists());
        writer.flush_to_disk().expect("flush");
        assert_eq!(std::fs::read(&path).expect("read"), b"eager");
    }
}
