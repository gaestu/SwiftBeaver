pub mod jsonl;
pub mod csv;
pub mod parquet;

use std::path::Path;

use thiserror::Error;

use crate::carve::CarvedFile;
use crate::parsers::browser::{BrowserCookieRecord, BrowserDownloadRecord, BrowserHistoryRecord};
use crate::strings::artifacts::StringArtefact;

#[derive(Debug, Clone, serde::Serialize)]
pub struct RunSummary {
    pub run_id: String,
    pub bytes_scanned: u64,
    pub chunks_processed: u64,
    pub hits_found: u64,
    pub files_carved: u64,
    pub string_spans: u64,
    pub artefacts_extracted: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct EntropyRegion {
    pub run_id: String,
    pub global_start: u64,
    pub global_end: u64,
    pub entropy: f64,
    pub window_size: u64,
}

#[derive(Debug, Clone, Copy)]
pub enum MetadataBackendKind {
    Jsonl,
    Csv,
    Parquet,
}

#[derive(Debug, Error)]
pub enum MetadataError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("csv error: {0}")]
    Csv(#[from] ::csv::Error),
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("other error: {0}")]
    Other(String),
}

pub trait MetadataSink: Send + Sync {
    fn record_file(&self, file: &CarvedFile) -> Result<(), MetadataError>;
    fn record_string(&self, artefact: &StringArtefact) -> Result<(), MetadataError>;
    fn record_history(&self, record: &BrowserHistoryRecord) -> Result<(), MetadataError>;
    fn record_cookie(&self, record: &BrowserCookieRecord) -> Result<(), MetadataError>;
    fn record_download(&self, record: &BrowserDownloadRecord) -> Result<(), MetadataError>;
    fn record_run_summary(&self, summary: &RunSummary) -> Result<(), MetadataError>;
    fn record_entropy(&self, region: &EntropyRegion) -> Result<(), MetadataError>;
    fn flush(&self) -> Result<(), MetadataError>;
}

pub fn build_sink(
    backend: MetadataBackendKind,
    cfg: &crate::config::Config,
    run_id: &str,
    tool_version: &str,
    config_hash: &str,
    evidence_path: &Path,
    evidence_sha256: &str,
    run_output_dir: &Path,
) -> Result<Box<dyn MetadataSink>, MetadataError> {
    match backend {
        MetadataBackendKind::Jsonl => Ok(Box::new(jsonl::JsonlSink::new(
            run_id,
            tool_version,
            config_hash,
            evidence_path,
            evidence_sha256,
            run_output_dir,
        )?)),
        MetadataBackendKind::Csv => Ok(Box::new(csv::CsvSink::new(
            run_id,
            tool_version,
            config_hash,
            evidence_path,
            evidence_sha256,
            run_output_dir,
        )?)),
        MetadataBackendKind::Parquet => parquet::build_parquet_sink(
            cfg,
            run_id,
            tool_version,
            config_hash,
            evidence_path,
            evidence_sha256,
            run_output_dir,
        ),
    }
}
