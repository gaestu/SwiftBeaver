//! # Pipeline Module
//!
//! Orchestrates the scanning, carving, and metadata recording pipeline.
//! This module handles multi-threaded processing of evidence sources.

pub mod events;
mod limiter;
pub mod workers;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossbeam_channel::bounded;
use tracing::{info, warn};

use crate::carve::CarveRegistry;
use crate::checkpoint::{CheckpointState, save_checkpoint};
use crate::chunk::{ChunkIter, ScanChunk, chunk_count};
use crate::config::Config;
use crate::constants::{CHANNEL_CAPACITY_MULTIPLIER, MIN_CHANNEL_CAPACITY};
use crate::evidence::EvidenceSource;
use crate::metadata::{MetadataSink, RunSummary};
use crate::scanner::SignatureScanner;
use crate::strings::StringScanner;
use crate::strings::artifacts::ArtefactScanConfig;

use events::MetadataEvent;
use limiter::CarveLimiter;
use workers::{ScanJob, StringJob};

/// Configuration for entropy detection during scanning
#[derive(Debug, Clone, Copy)]
pub struct EntropyConfig {
    pub window_size: usize,
    pub threshold: f64,
}

/// Pipeline statistics collected during a run
#[derive(Debug, Clone)]
pub struct PipelineStats {
    pub bytes_scanned: u64,
    pub chunks_processed: u64,
    pub hits_found: u64,
    pub files_carved: u64,
    pub string_spans: u64,
    pub artefacts_extracted: u64,
}

/// Progress snapshot reported during a run.
#[derive(Debug, Clone)]
pub struct ProgressSnapshot {
    pub bytes_scanned: u64,
    pub total_bytes: u64,
    pub chunks_processed: u64,
    pub hits_found: u64,
    pub files_carved: u64,
    pub string_spans: u64,
    pub artefacts_extracted: u64,
    pub carve_errors: u64,
    pub metadata_errors: u64,
    pub sqlite_errors: u64,
    pub elapsed_seconds: f64,
    pub throughput_mib: f64,
    pub eta_seconds: Option<u64>,
    /// Completion percentage (0.0 - 100.0)
    pub completion_pct: f64,
    /// Number of files that passed validation (if validation enabled)
    pub validation_pass: u64,
    /// Number of files that failed validation (if validation enabled)
    pub validation_fail: u64,
}

/// Progress callback trait for long-running scans.
pub trait ProgressReporter: Send + Sync {
    fn on_progress(&self, snapshot: &ProgressSnapshot);
}

pub struct ProgressConfig {
    pub reporter: Arc<dyn ProgressReporter>,
    pub interval: Duration,
}

pub struct CheckpointConfig {
    pub path: PathBuf,
    pub resume: Option<CheckpointState>,
}

/// Run the main processing pipeline.
///
/// This orchestrates:
/// - Chunk-based reading from evidence source
/// - Signature scanning (CPU or GPU)
/// - File carving based on detected signatures
/// - Optional string scanning and artefact extraction
/// - Optional entropy detection
/// - Metadata recording
pub fn run_pipeline(
    cfg: &Config,
    evidence: Arc<dyn EvidenceSource>,
    sig_scanner: Arc<dyn SignatureScanner>,
    string_scanner: Option<Arc<dyn StringScanner>>,
    meta_sink: Box<dyn MetadataSink>,
    run_output_dir: &Path,
    workers: usize,
    chunk_size: u64,
    overlap: u64,
    max_bytes: Option<u64>,
    max_chunks: Option<u64>,
    carve_registry: Arc<CarveRegistry>,
) -> Result<PipelineStats> {
    run_pipeline_inner(
        cfg,
        evidence,
        sig_scanner,
        string_scanner,
        meta_sink,
        run_output_dir,
        workers,
        chunk_size,
        overlap,
        max_bytes,
        max_chunks,
        carve_registry,
        None,
        None,
        None,
    )
}

/// Run the pipeline with an external cancellation flag (e.g., Ctrl+C).
pub fn run_pipeline_with_cancel(
    cfg: &Config,
    evidence: Arc<dyn EvidenceSource>,
    sig_scanner: Arc<dyn SignatureScanner>,
    string_scanner: Option<Arc<dyn StringScanner>>,
    meta_sink: Box<dyn MetadataSink>,
    run_output_dir: &Path,
    workers: usize,
    chunk_size: u64,
    overlap: u64,
    max_bytes: Option<u64>,
    max_chunks: Option<u64>,
    carve_registry: Arc<CarveRegistry>,
    cancel_flag: Arc<AtomicBool>,
    progress: Option<ProgressConfig>,
    checkpoint: Option<CheckpointConfig>,
) -> Result<PipelineStats> {
    run_pipeline_inner(
        cfg,
        evidence,
        sig_scanner,
        string_scanner,
        meta_sink,
        run_output_dir,
        workers,
        chunk_size,
        overlap,
        max_bytes,
        max_chunks,
        carve_registry,
        Some(cancel_flag),
        progress,
        checkpoint,
    )
}

struct PipelineChannels {
    scan_tx: crossbeam_channel::Sender<ScanJob>,
    scan_rx: crossbeam_channel::Receiver<ScanJob>,
    hit_tx: crossbeam_channel::Sender<crate::scanner::NormalizedHit>,
    hit_rx: crossbeam_channel::Receiver<crate::scanner::NormalizedHit>,
    meta_tx: crossbeam_channel::Sender<MetadataEvent>,
    meta_rx: crossbeam_channel::Receiver<MetadataEvent>,
    string_tx: Option<crossbeam_channel::Sender<StringJob>>,
    string_rx: Option<crossbeam_channel::Receiver<StringJob>>,
    sqlite_tx: Option<crossbeam_channel::Sender<workers::SqliteJob>>,
    sqlite_rx: Option<crossbeam_channel::Receiver<workers::SqliteJob>>,
}

struct PipelineCounters {
    bytes_scanned: Arc<AtomicU64>,
    chunks_processed: Arc<AtomicU64>,
    hits_found: Arc<AtomicU64>,
    string_spans: Arc<AtomicU64>,
    artefacts_found: Arc<AtomicU64>,
    carve_errors: Arc<AtomicU64>,
    metadata_errors: Arc<AtomicU64>,
    sqlite_errors: Arc<AtomicU64>,
    carve_limiter: Arc<CarveLimiter>,
}

impl PipelineCounters {
    fn new(max_files: Option<u64>) -> Self {
        Self {
            bytes_scanned: Arc::new(AtomicU64::new(0)),
            chunks_processed: Arc::new(AtomicU64::new(0)),
            hits_found: Arc::new(AtomicU64::new(0)),
            string_spans: Arc::new(AtomicU64::new(0)),
            artefacts_found: Arc::new(AtomicU64::new(0)),
            carve_errors: Arc::new(AtomicU64::new(0)),
            metadata_errors: Arc::new(AtomicU64::new(0)),
            sqlite_errors: Arc::new(AtomicU64::new(0)),
            carve_limiter: Arc::new(CarveLimiter::new(max_files)),
        }
    }
}

struct WorkerHandles {
    meta_handle: std::thread::JoinHandle<()>,
    scan_handles: Vec<std::thread::JoinHandle<()>>,
    carve_handles: Vec<std::thread::JoinHandle<()>>,
    string_handles: Vec<std::thread::JoinHandle<()>>,
    sqlite_handles: Vec<std::thread::JoinHandle<()>>,
}

struct ScanOutcome {
    hit_max_bytes: bool,
    hit_max_chunks: bool,
    hit_max_files: bool,
    cancelled: bool,
    start_time: Instant,
    next_offset: u64,
}

struct PipelineRunner<'a> {
    cfg: &'a Config,
    evidence: Arc<dyn EvidenceSource>,
    sig_scanner: Arc<dyn SignatureScanner>,
    string_scanner: Option<Arc<dyn StringScanner>>,
    meta_sink: Option<Box<dyn MetadataSink>>,
    run_output_dir: PathBuf,
    workers: usize,
    chunk_size: u64,
    overlap: u64,
    max_bytes: Option<u64>,
    max_chunks: Option<u64>,
    carve_registry: Arc<CarveRegistry>,
    cancel_flag: Option<Arc<AtomicBool>>,
    progress: Option<ProgressConfig>,
    checkpoint: Option<CheckpointConfig>,
}

impl<'a> PipelineRunner<'a> {
    fn new(
        cfg: &'a Config,
        evidence: Arc<dyn EvidenceSource>,
        sig_scanner: Arc<dyn SignatureScanner>,
        string_scanner: Option<Arc<dyn StringScanner>>,
        meta_sink: Box<dyn MetadataSink>,
        run_output_dir: &Path,
        workers: usize,
        chunk_size: u64,
        overlap: u64,
        max_bytes: Option<u64>,
        max_chunks: Option<u64>,
        carve_registry: Arc<CarveRegistry>,
        cancel_flag: Option<Arc<AtomicBool>>,
        progress: Option<ProgressConfig>,
        checkpoint: Option<CheckpointConfig>,
    ) -> Self {
        Self {
            cfg,
            evidence,
            sig_scanner,
            string_scanner,
            meta_sink: Some(meta_sink),
            run_output_dir: run_output_dir.to_path_buf(),
            workers,
            chunk_size,
            overlap,
            max_bytes,
            max_chunks,
            carve_registry,
            cancel_flag,
            progress,
            checkpoint,
        }
    }

    fn run(mut self) -> Result<PipelineStats> {
        let total_bytes = self.evidence.len();
        let (checkpoint_path, resume_offset, resume_chunks) =
            self.validate_checkpoint(total_bytes)?;
        let total_chunks = chunk_count(total_bytes, self.chunk_size);
        info!(
            "chunk_count={} chunk_size={} overlap={}",
            total_chunks, self.chunk_size, self.overlap
        );

        let sqlite_enabled = self.sqlite_enabled();
        let channels = self.setup_channels(self.string_scanner.is_some(), sqlite_enabled);
        let counters = PipelineCounters::new(self.cfg.max_files);

        let meta_sink = self.meta_sink.take().expect("metadata sink already taken");
        let entropy_cfg = self.entropy_config();
        let handles =
            self.spawn_workers(meta_sink, &channels, &counters, entropy_cfg, sqlite_enabled);

        let outcome = self.scan_loop(
            total_bytes,
            resume_offset,
            resume_chunks,
            &channels,
            &counters,
        )?;

        self.finalize(
            total_bytes,
            resume_offset,
            resume_chunks,
            channels,
            handles,
            counters,
            outcome,
            checkpoint_path,
        )
    }

    fn validate_checkpoint(&self, total_bytes: u64) -> Result<(Option<PathBuf>, u64, u64)> {
        let (resume_state, checkpoint_path) = match &self.checkpoint {
            Some(cfg) => (cfg.resume.clone(), Some(cfg.path.clone())),
            None => (None, None),
        };

        if let Some(state) = &resume_state {
            if state.chunk_size != self.chunk_size {
                return Err(anyhow::anyhow!(
                    "checkpoint chunk_size {} does not match requested {}",
                    state.chunk_size,
                    self.chunk_size
                ));
            }
            if state.overlap != self.overlap {
                return Err(anyhow::anyhow!(
                    "checkpoint overlap {} does not match requested {}",
                    state.overlap,
                    self.overlap
                ));
            }
            if state.evidence_len != total_bytes {
                return Err(anyhow::anyhow!(
                    "checkpoint evidence size {} does not match evidence length {}",
                    state.evidence_len,
                    total_bytes
                ));
            }
            if state.next_offset >= total_bytes {
                return Err(anyhow::anyhow!(
                    "checkpoint offset {} is beyond evidence size {}",
                    state.next_offset,
                    total_bytes
                ));
            }
            if state.run_id != self.cfg.run_id {
                warn!(
                    "checkpoint run_id={} does not match config run_id={}",
                    state.run_id, self.cfg.run_id
                );
            }
        }

        let resume_offset = resume_state.as_ref().map(|s| s.next_offset).unwrap_or(0);
        let resume_chunks = if self.chunk_size > 0 {
            resume_offset / self.chunk_size
        } else {
            0
        };
        Ok((checkpoint_path, resume_offset, resume_chunks))
    }

    fn sqlite_enabled(&self) -> bool {
        self.cfg.file_types.iter().any(|file_type| {
            file_type.id.eq_ignore_ascii_case("sqlite")
                || file_type.validator.trim().eq_ignore_ascii_case("sqlite")
        })
    }

    fn entropy_config(&self) -> Option<EntropyConfig> {
        if self.cfg.enable_entropy_detection && self.cfg.entropy_window_size > 0 {
            Some(EntropyConfig {
                window_size: self.cfg.entropy_window_size,
                threshold: self.cfg.entropy_threshold,
            })
        } else {
            None
        }
    }

    fn setup_channels(&self, string_enabled: bool, sqlite_enabled: bool) -> PipelineChannels {
        let channel_cap = self
            .workers
            .saturating_mul(CHANNEL_CAPACITY_MULTIPLIER)
            .max(MIN_CHANNEL_CAPACITY);
        let (scan_tx, scan_rx) = bounded::<ScanJob>(channel_cap);
        let (hit_tx, hit_rx) = bounded(channel_cap * 2);
        let (meta_tx, meta_rx) = bounded::<MetadataEvent>(channel_cap * 2);

        let (string_tx, string_rx) = if string_enabled {
            let (tx, rx) = bounded::<StringJob>(channel_cap);
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };

        let (sqlite_tx, sqlite_rx) = if sqlite_enabled {
            let (tx, rx) = bounded::<workers::SqliteJob>(channel_cap);
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };

        PipelineChannels {
            scan_tx,
            scan_rx,
            hit_tx,
            hit_rx,
            meta_tx,
            meta_rx,
            string_tx,
            string_rx,
            sqlite_tx,
            sqlite_rx,
        }
    }

    fn spawn_workers(
        &self,
        meta_sink: Box<dyn MetadataSink>,
        channels: &PipelineChannels,
        counters: &PipelineCounters,
        entropy_cfg: Option<EntropyConfig>,
        sqlite_enabled: bool,
    ) -> WorkerHandles {
        let meta_handle = workers::spawn_metadata_thread(
            meta_sink,
            channels.meta_rx.clone(),
            counters.metadata_errors.clone(),
        );

        let scan_handles = workers::spawn_scan_workers(
            self.workers,
            self.sig_scanner.clone(),
            self.string_scanner.clone(),
            channels.scan_rx.clone(),
            channels.hit_tx.clone(),
            channels.string_tx.clone(),
            channels.meta_tx.clone(),
            self.cfg.run_id.clone(),
            entropy_cfg,
            counters.hits_found.clone(),
            counters.string_spans.clone(),
        );

        let carve_handles = workers::spawn_carve_workers(
            self.workers,
            self.carve_registry.clone(),
            self.evidence.clone(),
            self.cfg.run_id.clone(),
            self.run_output_dir.clone(),
            channels.hit_rx.clone(),
            channels.meta_tx.clone(),
            counters.carve_limiter.clone(),
            channels.sqlite_tx.clone(),
            self.cfg.enable_sqlite_page_recovery,
            counters.carve_errors.clone(),
        );

        let string_handles = if let Some(rx) = &channels.string_rx {
            let scan_cfg = ArtefactScanConfig {
                urls: self.cfg.enable_url_scan,
                emails: self.cfg.enable_email_scan,
                phones: self.cfg.enable_phone_scan,
            };
            workers::spawn_string_workers(
                self.workers,
                self.cfg.run_id.clone(),
                rx.clone(),
                channels.meta_tx.clone(),
                counters.artefacts_found.clone(),
                scan_cfg,
            )
        } else {
            Vec::new()
        };

        let sqlite_handles = if sqlite_enabled {
            if let Some(rx) = &channels.sqlite_rx {
                workers::spawn_sqlite_workers(
                    self.workers,
                    rx.clone(),
                    channels.meta_tx.clone(),
                    counters.sqlite_errors.clone(),
                )
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        WorkerHandles {
            meta_handle,
            scan_handles,
            carve_handles,
            string_handles,
            sqlite_handles,
        }
    }

    fn scan_loop(
        &self,
        total_bytes: u64,
        resume_offset: u64,
        resume_chunks: u64,
        channels: &PipelineChannels,
        counters: &PipelineCounters,
    ) -> Result<ScanOutcome> {
        let max_bytes = self.max_bytes.unwrap_or(u64::MAX);
        let max_chunks = self.max_chunks.unwrap_or(u64::MAX);
        let mut chunks_seen = 0u64;
        let mut hit_max_bytes = resume_offset >= max_bytes;
        let mut hit_max_chunks = resume_chunks >= max_chunks;
        let mut hit_max_files = false;
        let mut cancelled = false;
        let start_time = Instant::now();
        let mut last_progress = Instant::now();
        let mut next_offset = resume_offset;

        for chunk in ChunkIter::new(total_bytes, self.chunk_size, self.overlap) {
            if hit_max_bytes || hit_max_chunks {
                break;
            }
            if chunk.start < resume_offset {
                continue;
            }
            if counters.carve_limiter.should_stop() {
                hit_max_files = true;
                break;
            }
            if let Some(flag) = &self.cancel_flag {
                if flag.load(Ordering::Relaxed) {
                    cancelled = true;
                    break;
                }
            }
            let chunks_seen_total = chunks_seen.saturating_add(resume_chunks);
            if chunks_seen_total >= max_chunks {
                hit_max_chunks = true;
                break;
            }
            let scanned_total = counters
                .bytes_scanned
                .load(Ordering::Relaxed)
                .saturating_add(resume_offset);
            if scanned_total >= max_bytes {
                hit_max_bytes = true;
                break;
            }
            let remaining = (max_bytes - scanned_total).min(chunk.length) as usize;
            let data = read_chunk_limited(self.evidence.as_ref(), &chunk, remaining)?;
            if data.is_empty() {
                break;
            }
            counters
                .bytes_scanned
                .fetch_add(data.len() as u64, Ordering::Relaxed);
            counters.chunks_processed.fetch_add(1, Ordering::Relaxed);
            chunks_seen += 1;
            next_offset = chunk.start.saturating_add(self.chunk_size);
            let chunk_id = chunk.id;
            channels
                .scan_tx
                .send(ScanJob {
                    chunk,
                    data: Arc::new(data),
                })
                .with_context(|| format!("scan channel closed while sending chunk {chunk_id}"))?;
            if let Some(progress) = &self.progress {
                if progress.interval.is_zero() || last_progress.elapsed() >= progress.interval {
                    let snapshot = build_progress_snapshot(
                        total_bytes,
                        resume_offset,
                        &start_time,
                        &counters.bytes_scanned,
                        &counters.chunks_processed,
                        &counters.hits_found,
                        counters.carve_limiter.carved_counter(),
                        &counters.string_spans,
                        &counters.artefacts_found,
                        &counters.carve_errors,
                        &counters.metadata_errors,
                        &counters.sqlite_errors,
                    );
                    progress.reporter.on_progress(&snapshot);
                    last_progress = Instant::now();

                    let _ = channels.meta_tx.send(MetadataEvent::Flush);
                }
            }
            let scanned_total = counters
                .bytes_scanned
                .load(Ordering::Relaxed)
                .saturating_add(resume_offset);
            if scanned_total >= max_bytes {
                hit_max_bytes = true;
                break;
            }
        }

        Ok(ScanOutcome {
            hit_max_bytes,
            hit_max_chunks,
            hit_max_files,
            cancelled,
            start_time,
            next_offset,
        })
    }

    fn finalize(
        &self,
        total_bytes: u64,
        resume_offset: u64,
        resume_chunks: u64,
        channels: PipelineChannels,
        handles: WorkerHandles,
        counters: PipelineCounters,
        outcome: ScanOutcome,
        checkpoint_path: Option<PathBuf>,
    ) -> Result<PipelineStats> {
        let PipelineChannels {
            scan_tx,
            hit_tx,
            string_tx,
            sqlite_tx,
            meta_tx,
            ..
        } = channels;

        drop(scan_tx);
        drop(hit_tx);
        drop(string_tx);
        drop(sqlite_tx);

        for handle in handles.scan_handles {
            let _ = handle.join();
        }
        for handle in handles.carve_handles {
            let _ = handle.join();
        }
        for handle in handles.string_handles {
            let _ = handle.join();
        }
        for handle in handles.sqlite_handles {
            let _ = handle.join();
        }

        let bytes_scanned_total = counters
            .bytes_scanned
            .load(Ordering::Relaxed)
            .saturating_add(resume_offset);
        let chunks_processed_total = counters
            .chunks_processed
            .load(Ordering::Relaxed)
            .saturating_add(resume_chunks);
        let summary = RunSummary {
            run_id: self.cfg.run_id.clone(),
            bytes_scanned: bytes_scanned_total,
            chunks_processed: chunks_processed_total,
            hits_found: counters.hits_found.load(Ordering::Relaxed),
            files_carved: counters.carve_limiter.carved(),
            string_spans: counters.string_spans.load(Ordering::Relaxed),
            artefacts_extracted: counters.artefacts_found.load(Ordering::Relaxed),
        };
        if let Err(err) = meta_tx.send(MetadataEvent::RunSummary(summary)) {
            warn!("metadata channel closed while sending run summary: {err}");
        }

        drop(meta_tx);
        let _ = handles.meta_handle.join();

        if let Some(progress) = &self.progress {
            let snapshot = build_progress_snapshot(
                total_bytes,
                resume_offset,
                &outcome.start_time,
                &counters.bytes_scanned,
                &counters.chunks_processed,
                &counters.hits_found,
                counters.carve_limiter.carved_counter(),
                &counters.string_spans,
                &counters.artefacts_found,
                &counters.carve_errors,
                &counters.metadata_errors,
                &counters.sqlite_errors,
            );
            progress.reporter.on_progress(&snapshot);
        }

        if outcome.cancelled {
            info!("shutdown requested; stopping early");
        }
        if outcome.hit_max_files {
            info!("max_files limit reached; stopping early");
        }
        if outcome.hit_max_bytes {
            info!("max_bytes limit reached; stopping early");
        }
        if outcome.hit_max_chunks {
            info!("max_chunks limit reached; stopping early");
        }

        let stats = PipelineStats {
            bytes_scanned: bytes_scanned_total,
            chunks_processed: chunks_processed_total,
            hits_found: counters.hits_found.load(Ordering::Relaxed),
            files_carved: counters.carve_limiter.carved(),
            string_spans: counters.string_spans.load(Ordering::Relaxed),
            artefacts_extracted: counters.artefacts_found.load(Ordering::Relaxed),
        };

        info!(
            "run_summary bytes_scanned={} chunks_processed={} hits_found={} files_carved={} string_spans={} artefacts_extracted={}",
            stats.bytes_scanned,
            stats.chunks_processed,
            stats.hits_found,
            stats.files_carved,
            stats.string_spans,
            stats.artefacts_extracted
        );

        if outcome.cancelled
            || outcome.hit_max_bytes
            || outcome.hit_max_chunks
            || outcome.hit_max_files
        {
            if let Some(path) = checkpoint_path {
                let state = CheckpointState::new(
                    &self.cfg.run_id,
                    self.chunk_size,
                    self.overlap,
                    outcome.next_offset.min(total_bytes),
                    total_bytes,
                );
                if let Err(err) = save_checkpoint(&path, &state) {
                    warn!("failed to write checkpoint {}: {err}", path.display());
                } else {
                    info!("checkpoint saved to {}", path.display());
                }
            }
        }

        Ok(stats)
    }
}

fn run_pipeline_inner(
    cfg: &Config,
    evidence: Arc<dyn EvidenceSource>,
    sig_scanner: Arc<dyn SignatureScanner>,
    string_scanner: Option<Arc<dyn StringScanner>>,
    meta_sink: Box<dyn MetadataSink>,
    run_output_dir: &Path,
    workers: usize,
    chunk_size: u64,
    overlap: u64,
    max_bytes: Option<u64>,
    max_chunks: Option<u64>,
    carve_registry: Arc<CarveRegistry>,
    cancel_flag: Option<Arc<AtomicBool>>,
    progress: Option<ProgressConfig>,
    checkpoint: Option<CheckpointConfig>,
) -> Result<PipelineStats> {
    PipelineRunner::new(
        cfg,
        evidence,
        sig_scanner,
        string_scanner,
        meta_sink,
        run_output_dir,
        workers,
        chunk_size,
        overlap,
        max_bytes,
        max_chunks,
        carve_registry,
        cancel_flag,
        progress,
        checkpoint,
    )
    .run()
}

fn build_progress_snapshot(
    total_bytes: u64,
    baseline_bytes: u64,
    start_time: &Instant,
    bytes_scanned: &AtomicU64,
    chunks_processed: &AtomicU64,
    hits_found: &AtomicU64,
    files_carved: &AtomicU64,
    string_spans: &AtomicU64,
    artefacts_found: &AtomicU64,
    carve_errors: &AtomicU64,
    metadata_errors: &AtomicU64,
    sqlite_errors: &AtomicU64,
) -> ProgressSnapshot {
    let elapsed_seconds = start_time.elapsed().as_secs_f64();
    let scanned = bytes_scanned.load(Ordering::Relaxed);
    let scanned_total = scanned.saturating_add(baseline_bytes);
    let throughput_mib = if elapsed_seconds > 0.0 {
        scanned as f64 / crate::constants::MIB as f64 / elapsed_seconds
    } else {
        0.0
    };
    let bytes_per_sec = if elapsed_seconds > 0.0 {
        scanned as f64 / elapsed_seconds
    } else {
        0.0
    };
    let eta_seconds = if bytes_per_sec > 0.0 && scanned_total < total_bytes {
        Some(((total_bytes - scanned_total) as f64 / bytes_per_sec).round() as u64)
    } else {
        None
    };

    let completion_pct = if total_bytes > 0 {
        (scanned_total as f64 / total_bytes as f64) * 100.0
    } else {
        0.0
    };

    ProgressSnapshot {
        bytes_scanned: scanned_total,
        total_bytes,
        chunks_processed: chunks_processed.load(Ordering::Relaxed),
        hits_found: hits_found.load(Ordering::Relaxed),
        files_carved: files_carved.load(Ordering::Relaxed),
        string_spans: string_spans.load(Ordering::Relaxed),
        artefacts_extracted: artefacts_found.load(Ordering::Relaxed),
        carve_errors: carve_errors.load(Ordering::Relaxed),
        metadata_errors: metadata_errors.load(Ordering::Relaxed),
        sqlite_errors: sqlite_errors.load(Ordering::Relaxed),
        elapsed_seconds,
        throughput_mib,
        eta_seconds,
        completion_pct,
        validation_pass: 0, // To be populated when validation is enabled
        validation_fail: 0, // To be populated when validation is enabled
    }
}

/// Read a chunk from evidence, limited to max_len bytes
fn read_chunk_limited(
    evidence: &dyn EvidenceSource,
    chunk: &ScanChunk,
    max_len: usize,
) -> Result<Vec<u8>> {
    if max_len == 0 {
        return Ok(Vec::new());
    }
    let mut buf = vec![0u8; max_len];
    let mut read = 0usize;
    while read < buf.len() {
        let n = evidence
            .read_at(chunk.start + read as u64, &mut buf[read..])
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        if n == 0 {
            break;
        }
        read += n;
    }
    buf.truncate(read);
    Ok(buf)
}
