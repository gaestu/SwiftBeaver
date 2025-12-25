pub mod gif;
pub mod jpeg;
pub mod png;

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

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
    fn process_hit(&self, hit: &NormalizedHit, ctx: &ExtractionContext) -> Result<Option<CarvedFile>, CarveError>;
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
    let dir = output_root.join(file_type);
    std::fs::create_dir_all(&dir)?;
    let filename = format!("{}_{}.{}", file_type, format!("{:012X}", global_start), extension);
    let full_path = dir.join(&filename);
    let rel_path = full_path
        .strip_prefix(output_root)
        .unwrap_or(&full_path)
        .to_string_lossy()
        .to_string();
    Ok((full_path, rel_path))
}

pub fn sanitize_extension(ext: &str) -> String {
    ext.trim_start_matches('.').to_ascii_lowercase()
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

    pub(crate) fn bytes_written(&self) -> u64 {
        self.written
    }

    pub(crate) fn finish(mut self) -> Result<(u64, String, String), CarveError> {
        self.writer.flush()?;
        let md5 = format!("{:x}", self.md5.compute());
        let sha256 = hex::encode(self.sha256.finalize());
        Ok((self.written, md5, sha256))
    }
}
