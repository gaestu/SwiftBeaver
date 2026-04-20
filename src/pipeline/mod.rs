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
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossbeam_channel::bounded;
use tracing::{info, warn};

use crate::carve::CarveRegistry;
use crate::checkpoint::{CheckpointState, save_checkpoint};
use crate::chunk::{ChunkIter, ScanChunk, chunk_count};
use crate::config::Config;
use crate::constants::{
    CHANNEL_CAPACITY_MULTIPLIER, FAST_HIT_CHANNEL_MULTIPLIER, METADATA_FLUSH_INTERVAL_SECS,
    MIN_CHANNEL_CAPACITY, SLOW_HIT_CHANNEL_MULTIPLIER, WRITE_QUEUE_CAPACITY_MULTIPLIER,
};
use crate::dedup::DedupTracker;
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
    pub files_rejected: u64,
    pub files_prevalidation_rejected: u64,
    pub string_spans: u64,
    pub artefacts_extracted: u64,
    pub scan_time_ms: u64,
    pub carve_time_ms: u64,
    /// Number of hits skipped due to overlap with already-carved files
    pub overlap_skipped: u64,
    pub duplicates_found: u64,
    pub duplicates_skipped: u64,
}

/// Progress snapshot reported during a run.
#[derive(Debug, Clone)]
pub struct ProgressSnapshot {
    pub bytes_scanned: u64,
    pub total_bytes: u64,
    pub chunks_processed: u64,
    pub hits_found: u64,
    pub files_carved: u64,
    pub files_rejected: u64,
    pub files_prevalidation_rejected: u64,
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
    /// Cumulative time spent signature-scanning (milliseconds)
    pub scan_time_ms: u64,
    /// Cumulative time spent carving files (milliseconds)
    pub carve_time_ms: u64,
    /// Number of hits skipped due to overlap with already-carved files
    pub overlap_skipped: u64,
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
#[allow(clippy::too_many_arguments)]
pub fn run_pipeline(
    cfg: &Config,
    evidence: Arc<dyn EvidenceSource>,
    sig_scanner: Arc<dyn SignatureScanner>,
    string_scanner: Option<Arc<dyn StringScanner>>,
    meta_sinks: Vec<Box<dyn MetadataSink>>,
    run_output_dir: &Path,
    scan_workers: usize,
    carve_workers: usize,
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
        meta_sinks,
        run_output_dir,
        scan_workers,
        carve_workers,
        chunk_size,
        overlap,
        max_bytes,
        max_chunks,
        carve_registry,
        None,
        None,
        None,
        false,
    )
}

/// Run the pipeline with an external cancellation flag (e.g., Ctrl+C).
#[allow(clippy::too_many_arguments)]
pub fn run_pipeline_with_cancel(
    cfg: &Config,
    evidence: Arc<dyn EvidenceSource>,
    sig_scanner: Arc<dyn SignatureScanner>,
    string_scanner: Option<Arc<dyn StringScanner>>,
    meta_sinks: Vec<Box<dyn MetadataSink>>,
    run_output_dir: &Path,
    scan_workers: usize,
    carve_workers: usize,
    chunk_size: u64,
    overlap: u64,
    max_bytes: Option<u64>,
    max_chunks: Option<u64>,
    carve_registry: Arc<CarveRegistry>,
    cancel_flag: Arc<AtomicBool>,
    progress: Option<ProgressConfig>,
    checkpoint: Option<CheckpointConfig>,
    metadata_only: bool,
) -> Result<PipelineStats> {
    run_pipeline_inner(
        cfg,
        evidence,
        sig_scanner,
        string_scanner,
        meta_sinks,
        run_output_dir,
        scan_workers,
        carve_workers,
        chunk_size,
        overlap,
        max_bytes,
        max_chunks,
        carve_registry,
        Some(cancel_flag),
        progress,
        checkpoint,
        metadata_only,
    )
}

struct PipelineChannels {
    scan_tx: crossbeam_channel::Sender<ScanJob>,
    scan_rx: crossbeam_channel::Receiver<ScanJob>,
    fast_hit_tx: crossbeam_channel::Sender<crate::scanner::NormalizedHit>,
    fast_hit_rx: crossbeam_channel::Receiver<crate::scanner::NormalizedHit>,
    slow_hit_tx: crossbeam_channel::Sender<crate::scanner::NormalizedHit>,
    slow_hit_rx: crossbeam_channel::Receiver<crate::scanner::NormalizedHit>,
    meta_tx: crossbeam_channel::Sender<MetadataEvent>,
    meta_rx: crossbeam_channel::Receiver<MetadataEvent>,
    string_tx: Option<crossbeam_channel::Sender<StringJob>>,
    string_rx: Option<crossbeam_channel::Receiver<StringJob>>,
    write_tx: crossbeam_channel::Sender<workers::WriteJob>,
    write_rx: crossbeam_channel::Receiver<workers::WriteJob>,
    file_shard_tx: crossbeam_channel::Sender<events::FileShardEvent>,
    file_shard_rx: crossbeam_channel::Receiver<events::FileShardEvent>,
    string_shard_tx: crossbeam_channel::Sender<events::StringShardEvent>,
    string_shard_rx: crossbeam_channel::Receiver<events::StringShardEvent>,
    entropy_shard_tx: crossbeam_channel::Sender<events::EntropyShardEvent>,
    entropy_shard_rx: crossbeam_channel::Receiver<events::EntropyShardEvent>,
}

struct PipelineCounters {
    bytes_scanned: Arc<AtomicU64>,
    chunks_processed: Arc<AtomicU64>,
    hits_found: Arc<AtomicU64>,
    files_rejected: Arc<AtomicU64>,
    files_prevalidation_rejected: Arc<AtomicU64>,
    string_spans: Arc<AtomicU64>,
    artefacts_found: Arc<AtomicU64>,
    carve_errors: Arc<AtomicU64>,
    metadata_errors: Arc<AtomicU64>,
    sqlite_errors: Arc<AtomicU64>,
    scan_time_ms: Arc<AtomicU64>,
    carve_time_ms: Arc<AtomicU64>,
    carve_limiter: Arc<CarveLimiter>,
    overlap_skipped: Arc<AtomicU64>,
    duplicates_found: Arc<AtomicU64>,
    duplicates_skipped: Arc<AtomicU64>,
}

impl PipelineCounters {
    fn new(max_files: Option<u64>) -> Self {
        Self {
            bytes_scanned: Arc::new(AtomicU64::new(0)),
            chunks_processed: Arc::new(AtomicU64::new(0)),
            hits_found: Arc::new(AtomicU64::new(0)),
            files_rejected: Arc::new(AtomicU64::new(0)),
            files_prevalidation_rejected: Arc::new(AtomicU64::new(0)),
            string_spans: Arc::new(AtomicU64::new(0)),
            artefacts_found: Arc::new(AtomicU64::new(0)),
            carve_errors: Arc::new(AtomicU64::new(0)),
            metadata_errors: Arc::new(AtomicU64::new(0)),
            sqlite_errors: Arc::new(AtomicU64::new(0)),
            scan_time_ms: Arc::new(AtomicU64::new(0)),
            carve_time_ms: Arc::new(AtomicU64::new(0)),
            carve_limiter: Arc::new(CarveLimiter::new(max_files)),
            overlap_skipped: Arc::new(AtomicU64::new(0)),
            duplicates_found: Arc::new(AtomicU64::new(0)),
            duplicates_skipped: Arc::new(AtomicU64::new(0)),
        }
    }
}

struct WorkerHandles {
    meta_handle: std::thread::JoinHandle<()>,
    meta_shard_handles: Vec<std::thread::JoinHandle<()>>,
    scan_handles: Vec<std::thread::JoinHandle<()>>,
    fast_carve_handles: Vec<std::thread::JoinHandle<()>>,
    slow_carve_handles: Vec<std::thread::JoinHandle<()>>,
    write_handles: Vec<std::thread::JoinHandle<()>>,
    string_handles: Vec<std::thread::JoinHandle<()>>,
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
    meta_sinks: Vec<Box<dyn MetadataSink>>,
    run_output_dir: PathBuf,
    scan_workers: usize,
    carve_workers: usize,
    chunk_size: u64,
    overlap: u64,
    max_bytes: Option<u64>,
    max_chunks: Option<u64>,
    carve_registry: Arc<CarveRegistry>,
    cancel_flag: Option<Arc<AtomicBool>>,
    progress: Option<ProgressConfig>,
    checkpoint: Option<CheckpointConfig>,
    metadata_only: bool,
}

impl<'a> PipelineRunner<'a> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        cfg: &'a Config,
        evidence: Arc<dyn EvidenceSource>,
        sig_scanner: Arc<dyn SignatureScanner>,
        string_scanner: Option<Arc<dyn StringScanner>>,
        meta_sinks: Vec<Box<dyn MetadataSink>>,
        run_output_dir: &Path,
        scan_workers: usize,
        carve_workers: usize,
        chunk_size: u64,
        overlap: u64,
        max_bytes: Option<u64>,
        max_chunks: Option<u64>,
        carve_registry: Arc<CarveRegistry>,
        cancel_flag: Option<Arc<AtomicBool>>,
        progress: Option<ProgressConfig>,
        checkpoint: Option<CheckpointConfig>,
        metadata_only: bool,
    ) -> Self {
        Self {
            cfg,
            evidence,
            sig_scanner,
            string_scanner,
            meta_sinks,
            run_output_dir: run_output_dir.to_path_buf(),
            scan_workers,
            carve_workers,
            chunk_size,
            overlap,
            max_bytes,
            max_chunks,
            carve_registry,
            cancel_flag,
            progress,
            checkpoint,
            metadata_only,
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

        let channels = self.setup_channels(self.string_scanner.is_some());
        let counters = PipelineCounters::new(self.cfg.max_files);

        let meta_sinks = std::mem::take(&mut self.meta_sinks);
        let entropy_cfg = self.entropy_config();
        let handles = self.spawn_workers(meta_sinks, &channels, &counters, entropy_cfg)?;

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

    fn setup_channels(&self, string_enabled: bool) -> PipelineChannels {
        let scan_cap = self
            .scan_workers
            .saturating_mul(CHANNEL_CAPACITY_MULTIPLIER)
            .max(MIN_CHANNEL_CAPACITY);
        let carve_cap = self
            .carve_workers
            .saturating_mul(CHANNEL_CAPACITY_MULTIPLIER)
            .max(MIN_CHANNEL_CAPACITY);
        let (scan_tx, scan_rx) = bounded::<ScanJob>(scan_cap);
        let (fast_hit_tx, fast_hit_rx) = bounded(
            carve_cap
                .saturating_mul(FAST_HIT_CHANNEL_MULTIPLIER)
                .max(MIN_CHANNEL_CAPACITY),
        );
        let (slow_hit_tx, slow_hit_rx) = bounded(
            carve_cap
                .saturating_mul(SLOW_HIT_CHANNEL_MULTIPLIER)
                .max(MIN_CHANNEL_CAPACITY),
        );
        let meta_cap = scan_cap.max(carve_cap);
        let (meta_tx, meta_rx) = bounded::<MetadataEvent>(meta_cap * 16);

        let (string_tx, string_rx) = if string_enabled {
            let (tx, rx) = bounded::<StringJob>(scan_cap);
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };

        let write_cap = self
            .cfg
            .write_workers
            .max(1)
            .saturating_mul(WRITE_QUEUE_CAPACITY_MULTIPLIER)
            .max(MIN_CHANNEL_CAPACITY);
        let (write_tx, write_rx) = bounded::<workers::WriteJob>(write_cap);

        let shard_cap = meta_cap * 4;
        let (file_shard_tx, file_shard_rx) = bounded::<events::FileShardEvent>(shard_cap);
        // String shard gets larger capacity for high-volume string events
        let string_shard_cap = shard_cap * 4;
        let (string_shard_tx, string_shard_rx) =
            bounded::<events::StringShardEvent>(string_shard_cap);
        let (entropy_shard_tx, entropy_shard_rx) = bounded::<events::EntropyShardEvent>(shard_cap);

        PipelineChannels {
            scan_tx,
            scan_rx,
            fast_hit_tx,
            fast_hit_rx,
            slow_hit_tx,
            slow_hit_rx,
            meta_tx,
            meta_rx,
            string_tx,
            string_rx,
            write_tx,
            write_rx,
            file_shard_tx,
            file_shard_rx,
            string_shard_tx,
            string_shard_rx,
            entropy_shard_tx,
            entropy_shard_rx,
        }
    }

    fn spawn_workers(
        &self,
        meta_sinks: Vec<Box<dyn MetadataSink>>,
        channels: &PipelineChannels,
        counters: &PipelineCounters,
        entropy_cfg: Option<EntropyConfig>,
    ) -> Result<WorkerHandles> {
        let dedup_tracker = if self.cfg.enable_deduplication {
            Some(Arc::new(DedupTracker::new()))
        } else {
            None
        };

        // Split carve workers into fast/slow pools
        let raw_ratio = self.cfg.fast_carve_worker_ratio;
        if !(0.0..=1.0).contains(&raw_ratio) {
            warn!("fast_carve_worker_ratio={raw_ratio} is outside [0.0, 1.0], clamping");
        }
        let ratio = raw_ratio.clamp(0.0, 1.0);
        let (fast_count, slow_count) = if self.carve_workers < 2 || ratio == 0.0 {
            // Single-worker mode or ratio=0: all hits go through the slow pool.
            // Scan workers send all hits to slow channel (split_enabled=false).
            (0usize, self.carve_workers.max(1))
        } else {
            let fc = (self.carve_workers as f64 * ratio).round() as usize;
            let fc = fc.max(1).min(self.carve_workers - 1);
            let sc = self.carve_workers - fc;
            (fc, sc)
        };

        info!("carve worker split: fast={fast_count} slow={slow_count} (ratio={ratio:.2})");

        // Spawn metadata threads: sharded (3 sinks) or single-thread (1 sink).
        // Exactly 1 or 3 sinks are accepted; other counts are rejected.
        let num_sinks = meta_sinks.len();
        anyhow::ensure!(
            num_sinks == 1 || num_sinks == 3,
            "expected 1 or 3 metadata sinks, got {num_sinks}"
        );
        let (meta_handle, meta_shard_handles) =
            match <[Box<dyn MetadataSink>; 3]>::try_from(meta_sinks) {
                Ok([file_sink, string_sink, entropy_sink]) => {
                    // Sharded path: router + 3 shard writer threads
                    let router_handle = workers::spawn_metadata_router(
                        channels.meta_rx.clone(),
                        channels.file_shard_tx.clone(),
                        channels.string_shard_tx.clone(),
                        channels.entropy_shard_tx.clone(),
                        counters.metadata_errors.clone(),
                    );

                    let file_shard_handle = workers::spawn_file_shard_thread(
                        file_sink,
                        channels.file_shard_rx.clone(),
                        counters.metadata_errors.clone(),
                    );
                    let string_shard_handle = workers::spawn_string_shard_thread(
                        string_sink,
                        channels.string_shard_rx.clone(),
                        counters.metadata_errors.clone(),
                    );
                    let entropy_shard_handle = workers::spawn_entropy_shard_thread(
                        entropy_sink,
                        channels.entropy_shard_rx.clone(),
                        counters.metadata_errors.clone(),
                    );

                    info!("metadata writer: sharded (3 shard threads + router)");
                    (
                        router_handle,
                        vec![file_shard_handle, string_shard_handle, entropy_shard_handle],
                    )
                }
                Err(sinks) => {
                    // Single-thread fallback (e.g. DryRunSink, CSV, JSONL)
                    let sink = sinks
                        .into_iter()
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("metadata sink vector was empty"))?;
                    let handle = workers::spawn_metadata_thread(
                        sink,
                        channels.meta_rx.clone(),
                        counters.metadata_errors.clone(),
                    );
                    info!("metadata writer: single-thread");
                    (handle, Vec::new())
                }
            };

        let scan_handles = workers::spawn_scan_workers(
            self.scan_workers,
            self.sig_scanner.clone(),
            self.string_scanner.clone(),
            channels.scan_rx.clone(),
            channels.fast_hit_tx.clone(),
            channels.slow_hit_tx.clone(),
            self.carve_registry.clone(),
            fast_count > 0, // split_enabled: classify hits only when fast pool exists
            channels.string_tx.clone(),
            channels.meta_tx.clone(),
            self.cfg.run_id.clone(),
            entropy_cfg,
            counters.hits_found.clone(),
            counters.string_spans.clone(),
            self.cfg.sqlite_page_max_hits_per_chunk,
            counters.scan_time_ms.clone(),
        );

        // Build per-type semaphores from carver_limits config
        let type_semaphores = workers::build_type_semaphores(&self.cfg.carver_limits);
        let carve_cfg_base = workers::CarveWorkerConfig {
            workers: 1,
            registry: self.carve_registry.clone(),
            evidence: self.evidence.clone(),
            run_id: self.cfg.run_id.clone(),
            run_output_dir: self.run_output_dir.clone(),
            deferred_buffer_bytes: self.cfg.deferred_buffer_kb * 1024,
            metadata_only: self.metadata_only,
            hash_config: crate::hash::HashConfig::from_names(&self.cfg.hash_algorithms),
            dedup_tracker: dedup_tracker.clone(),
            skip_duplicates: self.cfg.skip_duplicate_files,
            type_semaphores: None,
        };

        let fast_carve_handles = if fast_count > 0 {
            workers::spawn_carve_workers(
                workers::CarveWorkerConfig {
                    workers: fast_count,
                    ..carve_cfg_base.clone()
                },
                workers::CarveWorkerRuntime {
                    rx: channels.fast_hit_rx.clone(),
                    write_tx: channels.write_tx.clone(),
                    carve_limiter: counters.carve_limiter.clone(),
                    carve_errors: counters.carve_errors.clone(),
                    carve_time_ms: counters.carve_time_ms.clone(),
                    files_rejected: counters.files_rejected.clone(),
                    files_prevalidation_rejected: counters.files_prevalidation_rejected.clone(),
                    overlap_skipped: counters.overlap_skipped.clone(),
                    duplicates_found: counters.duplicates_found.clone(),
                },
            )
        } else {
            Vec::new()
        };

        let slow_carve_handles = workers::spawn_carve_workers(
            workers::CarveWorkerConfig {
                workers: slow_count,
                type_semaphores: Some(type_semaphores),
                ..carve_cfg_base
            },
            workers::CarveWorkerRuntime {
                rx: channels.slow_hit_rx.clone(),
                write_tx: channels.write_tx.clone(),
                carve_limiter: counters.carve_limiter.clone(),
                carve_errors: counters.carve_errors.clone(),
                carve_time_ms: counters.carve_time_ms.clone(),
                files_rejected: counters.files_rejected.clone(),
                files_prevalidation_rejected: counters.files_prevalidation_rejected.clone(),
                overlap_skipped: counters.overlap_skipped.clone(),
                duplicates_found: counters.duplicates_found.clone(),
            },
        );

        let string_handles = if let Some(rx) = &channels.string_rx {
            let scan_cfg = ArtefactScanConfig {
                urls: self.cfg.enable_url_scan,
                emails: self.cfg.enable_email_scan,
                phones: self.cfg.enable_phone_scan,
            };
            workers::spawn_string_workers(
                self.scan_workers,
                self.cfg.run_id.clone(),
                rx.clone(),
                channels.meta_tx.clone(),
                counters.artefacts_found.clone(),
                scan_cfg,
            )
        } else {
            Vec::new()
        };

        let write_handles = workers::spawn_write_workers(
            self.cfg.write_workers,
            channels.write_rx.clone(),
            channels.meta_tx.clone(),
            counters.carve_errors.clone(),
            counters.duplicates_skipped.clone(),
        );

        Ok(WorkerHandles {
            meta_handle,
            meta_shard_handles,
            scan_handles,
            fast_carve_handles,
            slow_carve_handles,
            write_handles,
            string_handles,
        })
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
        let mut last_flush = Instant::now()
            .checked_sub(Duration::from_secs(METADATA_FLUSH_INTERVAL_SECS))
            .unwrap_or(start_time);
        let mut next_offset = resume_offset;

        // Early exit if already at limits from checkpoint resume
        if hit_max_bytes || hit_max_chunks {
            return Ok(ScanOutcome {
                hit_max_bytes,
                hit_max_chunks,
                hit_max_files,
                cancelled,
                start_time,
                next_offset,
            });
        }

        // Prefetch channel: reader thread → scan loop
        let prefetch_cap = self.scan_workers.max(2);
        let (prefetch_tx, prefetch_rx) = bounded::<(ScanChunk, Arc<Vec<u8>>)>(prefetch_cap);

        // Clone what the reader thread needs
        let evidence = self.evidence.clone();
        let chunk_size = self.chunk_size;
        let overlap = self.overlap;
        let cancel_flag = self.cancel_flag.clone();

        // Spawn dedicated reader thread to decouple I/O from dispatch
        let reader_handle = thread::spawn(move || -> Result<()> {
            for chunk in ChunkIter::new(total_bytes, chunk_size, overlap) {
                if chunk.start < resume_offset {
                    continue;
                }
                if let Some(flag) = &cancel_flag
                    && flag.load(Ordering::Relaxed)
                {
                    break;
                }
                let data = read_chunk_limited(evidence.as_ref(), &chunk, chunk.length as usize)?;
                if data.is_empty() {
                    break;
                }
                if prefetch_tx.send((chunk, Arc::new(data))).is_err() {
                    break; // receiver dropped (scan loop exited early)
                }
            }
            Ok(())
        });

        // Scan loop: receive pre-read chunks and dispatch to workers
        for (chunk, data) in &prefetch_rx {
            if counters.carve_limiter.should_stop() {
                hit_max_files = true;
                break;
            }
            if let Some(flag) = &self.cancel_flag
                && flag.load(Ordering::Relaxed)
            {
                cancelled = true;
                break;
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
            let effective_len = data.len().min(remaining);
            if effective_len == 0 {
                break;
            }
            let data = if effective_len < data.len() {
                Arc::new(data[..effective_len].to_vec())
            } else {
                data
            };
            counters
                .bytes_scanned
                .fetch_add(data.len() as u64, Ordering::Relaxed);
            counters.chunks_processed.fetch_add(1, Ordering::Relaxed);
            chunks_seen += 1;
            next_offset = chunk.start.saturating_add(self.chunk_size);
            let chunk_id = chunk.id;
            channels
                .scan_tx
                .send(ScanJob { chunk, data })
                .with_context(|| format!("scan channel closed while sending chunk {chunk_id}"))?;
            if let Some(progress) = &self.progress
                && (progress.interval.is_zero() || last_progress.elapsed() >= progress.interval)
            {
                let snapshot = build_progress_snapshot(
                    total_bytes,
                    resume_offset,
                    &start_time,
                    &counters.bytes_scanned,
                    &counters.chunks_processed,
                    &counters.hits_found,
                    counters.carve_limiter.carved_counter(),
                    &counters.files_rejected,
                    &counters.files_prevalidation_rejected,
                    &counters.string_spans,
                    &counters.artefacts_found,
                    &counters.carve_errors,
                    &counters.metadata_errors,
                    &counters.sqlite_errors,
                    &counters.scan_time_ms,
                    &counters.carve_time_ms,
                    &counters.overlap_skipped,
                );
                progress.reporter.on_progress(&snapshot);
                last_progress = Instant::now();

                if last_flush.elapsed() >= Duration::from_secs(METADATA_FLUSH_INTERVAL_SECS) {
                    let _ = channels.meta_tx.try_send(MetadataEvent::Flush);
                    last_flush = Instant::now();
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

        // Drop receiver to unblock reader thread if it's waiting on send
        drop(prefetch_rx);

        // Wait for reader thread to finish
        match reader_handle.join() {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                // If we stopped early, reader errors from channel closure are expected
                if !cancelled && !hit_max_files && !hit_max_bytes && !hit_max_chunks {
                    return Err(err);
                }
            }
            Err(_) => {
                warn!("prefetch reader thread panicked");
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

    #[allow(clippy::too_many_arguments)]
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
            fast_hit_tx,
            slow_hit_tx,
            string_tx,
            write_tx,
            meta_tx,
            file_shard_tx,
            string_shard_tx,
            entropy_shard_tx,
            ..
        } = channels;

        drop(scan_tx);
        drop(fast_hit_tx);
        drop(slow_hit_tx);
        drop(string_tx);

        for handle in handles.scan_handles {
            let _ = handle.join();
        }
        for handle in handles.fast_carve_handles {
            let _ = handle.join();
        }
        for handle in handles.slow_carve_handles {
            let _ = handle.join();
        }
        // Drop write channel after carve workers finish (they're the senders)
        drop(write_tx);
        for handle in handles.write_handles {
            let _ = handle.join();
        }
        for handle in handles.string_handles {
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
            files_rejected: counters.files_rejected.load(Ordering::Relaxed),
            files_prevalidation_rejected: counters
                .files_prevalidation_rejected
                .load(Ordering::Relaxed),
            string_spans: counters.string_spans.load(Ordering::Relaxed),
            artefacts_extracted: counters.artefacts_found.load(Ordering::Relaxed),
            duplicates_found: counters.duplicates_found.load(Ordering::Relaxed),
            duplicates_skipped: counters.duplicates_skipped.load(Ordering::Relaxed),
        };
        if let Err(err) = meta_tx.send(MetadataEvent::RunSummary(summary)) {
            warn!("metadata channel closed while sending run summary: {err}");
        }

        drop(meta_tx);
        // Join the router (or single-thread metadata writer)
        let _ = handles.meta_handle.join();
        // Drop shard senders so shard threads see channel closure
        drop(file_shard_tx);
        drop(string_shard_tx);
        drop(entropy_shard_tx);
        // Join shard writer threads (empty vec in single-thread mode)
        for handle in handles.meta_shard_handles {
            let _ = handle.join();
        }

        if let Some(progress) = &self.progress {
            let snapshot = build_progress_snapshot(
                total_bytes,
                resume_offset,
                &outcome.start_time,
                &counters.bytes_scanned,
                &counters.chunks_processed,
                &counters.hits_found,
                counters.carve_limiter.carved_counter(),
                &counters.files_rejected,
                &counters.files_prevalidation_rejected,
                &counters.string_spans,
                &counters.artefacts_found,
                &counters.carve_errors,
                &counters.metadata_errors,
                &counters.sqlite_errors,
                &counters.scan_time_ms,
                &counters.carve_time_ms,
                &counters.overlap_skipped,
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
            files_rejected: counters.files_rejected.load(Ordering::Relaxed),
            files_prevalidation_rejected: counters
                .files_prevalidation_rejected
                .load(Ordering::Relaxed),
            string_spans: counters.string_spans.load(Ordering::Relaxed),
            artefacts_extracted: counters.artefacts_found.load(Ordering::Relaxed),
            scan_time_ms: counters.scan_time_ms.load(Ordering::Relaxed),
            carve_time_ms: counters.carve_time_ms.load(Ordering::Relaxed),
            overlap_skipped: counters.overlap_skipped.load(Ordering::Relaxed),
            duplicates_found: counters.duplicates_found.load(Ordering::Relaxed),
            duplicates_skipped: counters.duplicates_skipped.load(Ordering::Relaxed),
        };

        info!(
            "run_summary bytes_scanned={} chunks_processed={} hits_found={} files_carved={} files_rejected={} files_prevalidation_rejected={} string_spans={} artefacts_extracted={} scan_time_ms={} carve_time_ms={} overlap_skipped={} duplicates_found={} duplicates_skipped={}",
            stats.bytes_scanned,
            stats.chunks_processed,
            stats.hits_found,
            stats.files_carved,
            stats.files_rejected,
            stats.files_prevalidation_rejected,
            stats.string_spans,
            stats.artefacts_extracted,
            stats.scan_time_ms,
            stats.carve_time_ms,
            stats.overlap_skipped,
            stats.duplicates_found,
            stats.duplicates_skipped,
        );

        if (outcome.cancelled
            || outcome.hit_max_bytes
            || outcome.hit_max_chunks
            || outcome.hit_max_files)
            && let Some(path) = checkpoint_path
        {
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

        Ok(stats)
    }
}

#[allow(clippy::too_many_arguments)]
fn run_pipeline_inner(
    cfg: &Config,
    evidence: Arc<dyn EvidenceSource>,
    sig_scanner: Arc<dyn SignatureScanner>,
    string_scanner: Option<Arc<dyn StringScanner>>,
    meta_sinks: Vec<Box<dyn MetadataSink>>,
    run_output_dir: &Path,
    scan_workers: usize,
    carve_workers: usize,
    chunk_size: u64,
    overlap: u64,
    max_bytes: Option<u64>,
    max_chunks: Option<u64>,
    carve_registry: Arc<CarveRegistry>,
    cancel_flag: Option<Arc<AtomicBool>>,
    progress: Option<ProgressConfig>,
    checkpoint: Option<CheckpointConfig>,
    metadata_only: bool,
) -> Result<PipelineStats> {
    PipelineRunner::new(
        cfg,
        evidence,
        sig_scanner,
        string_scanner,
        meta_sinks,
        run_output_dir,
        scan_workers,
        carve_workers,
        chunk_size,
        overlap,
        max_bytes,
        max_chunks,
        carve_registry,
        cancel_flag,
        progress,
        checkpoint,
        metadata_only,
    )
    .run()
}

#[allow(clippy::too_many_arguments)]
fn build_progress_snapshot(
    total_bytes: u64,
    baseline_bytes: u64,
    start_time: &Instant,
    bytes_scanned: &AtomicU64,
    chunks_processed: &AtomicU64,
    hits_found: &AtomicU64,
    files_carved: &AtomicU64,
    files_rejected: &AtomicU64,
    files_prevalidation_rejected: &AtomicU64,
    string_spans: &AtomicU64,
    artefacts_found: &AtomicU64,
    carve_errors: &AtomicU64,
    metadata_errors: &AtomicU64,
    sqlite_errors: &AtomicU64,
    scan_time_ms: &AtomicU64,
    carve_time_ms: &AtomicU64,
    overlap_skipped: &AtomicU64,
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
        files_rejected: files_rejected.load(Ordering::Relaxed),
        files_prevalidation_rejected: files_prevalidation_rejected.load(Ordering::Relaxed),
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
        scan_time_ms: scan_time_ms.load(Ordering::Relaxed),
        carve_time_ms: carve_time_ms.load(Ordering::Relaxed),
        overlap_skipped: overlap_skipped.load(Ordering::Relaxed),
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
