pub mod avi;
pub mod bmp;
pub mod footer;
pub mod gif;
pub mod jpeg;
pub mod mp3;
pub mod mp4;
pub mod ole;
pub mod pdf;
pub mod png;
pub mod rar;
pub mod riff;
pub mod sevenz;
pub mod sqlite;
pub mod tiff;
pub mod wav;
pub mod webp;
pub mod zip;

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

/// Metadata about a carved file.
///
/// # Example
/// ```rust
/// use fastcarve::carve::CarvedFile;
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
}

impl<'a> std::fmt::Debug for ExtractionContext<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtractionContext")
            .field("run_id", &self.run_id)
            .field("output_root", &self.output_root)
            .field("evidence", &"<dyn EvidenceSource>")
            .finish()
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
    let base = format!("{}_{}", safe_type, format!("{:012X}", global_start));
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

pub(crate) struct CarveStream<'a> {
    evidence: &'a dyn EvidenceSource,
    offset: u64,
    max_size: u64,
    written: u64,
    writer: BufWriter<File>,
    md5: md5::Context,
    sha256: Sha256,
}

impl<'a> CarveStream<'a> {
    pub(crate) fn new(
        evidence: &'a dyn EvidenceSource,
        offset: u64,
        max_size: u64,
        writer: File,
    ) -> Self {
        Self {
            evidence,
            offset,
            max_size,
            written: 0,
            writer: BufWriter::new(writer),
            md5: md5::Context::new(),
            sha256: Sha256::new(),
        }
    }

    pub(crate) fn read_exact(&mut self, len: usize) -> Result<Vec<u8>, CarveError> {
        if self.max_size > 0 && self.written.saturating_add(len as u64) > self.max_size {
            return Err(CarveError::Truncated);
        }

        let mut buf = vec![0u8; len];
        let mut read = 0usize;
        while read < len {
            let n = self
                .evidence
                .read_at(self.offset, &mut buf[read..])
                .map_err(|e| CarveError::Evidence(e.to_string()))?;
            if n == 0 {
                return Err(CarveError::Eof);
            }
            self.write_bytes(&buf[read..read + n])?;
            read += n;
        }

        Ok(buf)
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
        self.writer.flush()?;
        let md5 = format!("{:x}", self.md5.compute());
        let sha256 = hex::encode(self.sha256.finalize());
        Ok((self.written, md5, sha256))
    }

    /// Get the number of bytes written so far
    pub(crate) fn bytes_written(&self) -> u64 {
        self.written
    }
}

pub(crate) fn write_range(
    ctx: &ExtractionContext,
    start: u64,
    end: u64,
    file: &mut File,
    md5: &mut md5::Context,
    sha256: &mut Sha256,
) -> Result<(u64, bool), CarveError> {
    let mut offset = start;
    let mut remaining = end.saturating_sub(start);
    let mut bytes_written = 0u64;
    let buf_size = 64 * 1024;

    while remaining > 0 {
        let read_len = remaining.min(buf_size as u64) as usize;
        let mut buf = vec![0u8; read_len];
        let n = ctx
            .evidence
            .read_at(offset, &mut buf)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n == 0 {
            return Ok((bytes_written, true));
        }
        buf.truncate(n);
        file.write_all(&buf)?;
        md5.consume(&buf);
        sha256.update(&buf);
        bytes_written = bytes_written.saturating_add(buf.len() as u64);
        offset = offset.saturating_add(buf.len() as u64);
        remaining = remaining.saturating_sub(buf.len() as u64);
        if n < read_len {
            return Ok((bytes_written, true));
        }
    }

    Ok((bytes_written, false))
}

#[cfg(test)]
mod tests {
    use super::{output_path, sanitize_component, sanitize_extension};
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
}
