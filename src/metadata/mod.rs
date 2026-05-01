pub mod csv;
pub mod jsonl;
pub mod parquet;
pub mod windows;

use std::path::Path;

use thiserror::Error;

use crate::carve::CarvedFile;
use crate::carve::bek::BitlockerBekRecord;
use crate::carve::windows::WindowsArtefactRecord;
use crate::parsers::browser::{BrowserCookieRecord, BrowserDownloadRecord, BrowserHistoryRecord};
use crate::strings::artifacts::StringArtefact;

#[derive(Debug, Clone, serde::Serialize)]
pub struct RunSummary {
    pub run_id: String,
    pub bytes_scanned: u64,
    pub chunks_processed: u64,
    pub hits_found: u64,
    pub files_carved: u64,
    pub files_rejected: u64,
    pub files_prevalidation_rejected: u64,
    pub files_capped: u64,
    pub overlap_skipped: u64,
    pub string_spans: u64,
    pub artefacts_extracted: u64,
    pub duplicates_found: u64,
    pub duplicates_skipped: u64,
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

impl MetadataBackendKind {
    /// Whether this backend supports multiple independent sink instances
    /// writing to the same output directory without file conflicts.
    ///
    /// Parquet uses lazy per-category writers so each shard only creates
    /// the files it needs. CSV and JSONL eagerly create all output files
    /// in `new()`, so multiple instances would truncate each other.
    pub fn supports_sharding(self) -> bool {
        matches!(self, MetadataBackendKind::Parquet)
    }
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

/// Metadata output sink for carved files and artefacts.
///
/// # Example
/// ```rust
/// use swiftbeaver::config;
/// use swiftbeaver::metadata::{self, MetadataBackendKind, MetadataSink, RunSummary};
/// use std::path::PathBuf;
///
/// let loaded = config::load_config(None).unwrap();
/// let run_output_dir = std::env::temp_dir().join("SwiftBeaver_meta_example");
/// std::fs::create_dir_all(&run_output_dir).unwrap();
///
/// let sink = metadata::build_sink(
///     MetadataBackendKind::Jsonl,
///     &loaded.config,
///     "example_run",
///     "0.1.0",
///     &loaded.config_hash,
///     PathBuf::from("image.raw").as_path(),
///     "",
///     &run_output_dir,
/// )
/// .unwrap();
///
/// let summary = RunSummary {
///     run_id: "example_run".to_string(),
///     bytes_scanned: 0,
///     chunks_processed: 0,
///     hits_found: 0,
///     files_carved: 0,
///     files_rejected: 0,
///     files_prevalidation_rejected: 0,
///     files_capped: 0,
///     overlap_skipped: 0,
///     string_spans: 0,
///     artefacts_extracted: 0,
///     duplicates_found: 0,
///     duplicates_skipped: 0,
/// };
/// sink.record_run_summary(&summary).unwrap();
/// sink.flush().unwrap();
/// ```
pub trait MetadataSink: Send + Sync {
    fn record_file(&self, file: &CarvedFile) -> Result<(), MetadataError>;
    fn record_bitlocker_bek(&self, record: &BitlockerBekRecord) -> Result<(), MetadataError>;
    fn record_string(&self, artefact: &StringArtefact) -> Result<(), MetadataError>;
    fn record_windows_artefact(&self, record: &WindowsArtefactRecord) -> Result<(), MetadataError>;
    fn record_history(&self, record: &BrowserHistoryRecord) -> Result<(), MetadataError>;
    fn record_cookie(&self, record: &BrowserCookieRecord) -> Result<(), MetadataError>;
    fn record_download(&self, record: &BrowserDownloadRecord) -> Result<(), MetadataError>;
    fn record_run_summary(&self, summary: &RunSummary) -> Result<(), MetadataError>;
    fn record_entropy(&self, region: &EntropyRegion) -> Result<(), MetadataError>;
    fn flush(&self) -> Result<(), MetadataError>;
}

/// A no-op sink for dry-run mode that doesn't write any files
pub struct DryRunSink;

impl MetadataSink for DryRunSink {
    fn record_file(&self, _file: &CarvedFile) -> Result<(), MetadataError> {
        Ok(())
    }
    fn record_bitlocker_bek(&self, _record: &BitlockerBekRecord) -> Result<(), MetadataError> {
        Ok(())
    }
    fn record_string(&self, _artefact: &StringArtefact) -> Result<(), MetadataError> {
        Ok(())
    }
    fn record_windows_artefact(
        &self,
        _record: &WindowsArtefactRecord,
    ) -> Result<(), MetadataError> {
        Ok(())
    }
    fn record_history(&self, _record: &BrowserHistoryRecord) -> Result<(), MetadataError> {
        Ok(())
    }
    fn record_cookie(&self, _record: &BrowserCookieRecord) -> Result<(), MetadataError> {
        Ok(())
    }
    fn record_download(&self, _record: &BrowserDownloadRecord) -> Result<(), MetadataError> {
        Ok(())
    }
    fn record_run_summary(&self, _summary: &RunSummary) -> Result<(), MetadataError> {
        Ok(())
    }
    fn record_entropy(&self, _region: &EntropyRegion) -> Result<(), MetadataError> {
        Ok(())
    }
    fn flush(&self) -> Result<(), MetadataError> {
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
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

/// Build a dry-run sink that doesn't write any files
pub fn build_dry_run_sink() -> Box<dyn MetadataSink> {
    Box::new(DryRunSink)
}
