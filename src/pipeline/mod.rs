//! # Pipeline Module
//!
//! Orchestrates the scanning, carving, and metadata recording pipeline.
//! This module handles multi-threaded processing of evidence sources.

pub mod events;
pub mod workers;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossbeam_channel::bounded;
use tracing::{info, warn};

use crate::carve::CarveRegistry;
use crate::checkpoint::{save_checkpoint, CheckpointState};
use crate::chunk::{build_chunks, ScanChunk};
use crate::config::Config;
use crate::constants::{CHANNEL_CAPACITY_MULTIPLIER, MIN_CHANNEL_CAPACITY};
use crate::evidence::EvidenceSource;
use crate::metadata::{MetadataSink, RunSummary};
use crate::scanner::SignatureScanner;
use crate::strings::artifacts::ArtefactScanConfig;
use crate::strings::StringScanner;

use events::MetadataEvent;
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
    let total_bytes = evidence.len();
    let (resume_state, checkpoint_path) = match &checkpoint {
        Some(cfg) => (cfg.resume.clone(), Some(cfg.path.clone())),
        None => (None, None),
    };
    if let Some(state) = &resume_state {
        if state.chunk_size != chunk_size {
            return Err(anyhow::anyhow!(
                "checkpoint chunk_size {} does not match requested {}",
                state.chunk_size,
                chunk_size
            ));
        }
        if state.overlap != overlap {
            return Err(anyhow::anyhow!(
                "checkpoint overlap {} does not match requested {}",
                state.overlap,
                overlap
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
        if state.run_id != cfg.run_id {
            warn!(
                "checkpoint run_id={} does not match config run_id={}",
                state.run_id, cfg.run_id
            );
        }
    }
    let resume_offset = resume_state.as_ref().map(|s| s.next_offset).unwrap_or(0);
    let resume_chunks = if chunk_size > 0 {
        resume_offset / chunk_size
    } else {
        0
    };
    let chunks = build_chunks(total_bytes, chunk_size, overlap);
    info!(
        "chunk_count={} chunk_size={} overlap={}",
        chunks.len(),
        chunk_size,
        overlap
    );

    // Create channels
    let channel_cap = workers
        .saturating_mul(CHANNEL_CAPACITY_MULTIPLIER)
        .max(MIN_CHANNEL_CAPACITY);
    let (scan_tx, scan_rx) = bounded::<ScanJob>(channel_cap);
    let (hit_tx, hit_rx) = bounded(channel_cap * 2);
    let (meta_tx, meta_rx) = bounded::<MetadataEvent>(channel_cap * 2);

    let (string_tx, string_rx) = if string_scanner.is_some() {
        let (tx, rx) = bounded::<StringJob>(channel_cap);
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    // Atomic counters for statistics
    let bytes_scanned = Arc::new(AtomicU64::new(0));
    let chunks_processed = Arc::new(AtomicU64::new(0));
    let hits_found = Arc::new(AtomicU64::new(0));
    let files_carved = Arc::new(AtomicU64::new(0));
    let string_spans = Arc::new(AtomicU64::new(0));
    let artefacts_found = Arc::new(AtomicU64::new(0));
    let carve_errors = Arc::new(AtomicU64::new(0));
    let metadata_errors = Arc::new(AtomicU64::new(0));
    let sqlite_errors = Arc::new(AtomicU64::new(0));

    // Start metadata recording thread
    let meta_handle = workers::spawn_metadata_thread(meta_sink, meta_rx, metadata_errors.clone());

    // Build entropy config if enabled
    let entropy_cfg = if cfg.enable_entropy_detection && cfg.entropy_window_size > 0 {
        Some(EntropyConfig {
            window_size: cfg.entropy_window_size,
            threshold: cfg.entropy_threshold,
        })
    } else {
        None
    };

    // Spawn worker threads
    let scan_handles = workers::spawn_scan_workers(
        workers,
        sig_scanner,
        string_scanner.clone(),
        scan_rx,
        hit_tx.clone(),
        string_tx.clone(),
        meta_tx.clone(),
        cfg.run_id.clone(),
        entropy_cfg,
        hits_found.clone(),
        string_spans.clone(),
    );

    let carve_handles = workers::spawn_carve_workers(
        workers,
        carve_registry,
        evidence.clone(),
        cfg.run_id.clone(),
        run_output_dir.to_path_buf(),
        hit_rx,
        meta_tx.clone(),
        files_carved.clone(),
        cfg.enable_sqlite_page_recovery,
        cfg.max_files,
        carve_errors.clone(),
        sqlite_errors.clone(),
    );

    let string_handles = if let Some(rx) = string_rx {
        let scan_cfg = ArtefactScanConfig {
            urls: cfg.enable_url_scan,
            emails: cfg.enable_email_scan,
            phones: cfg.enable_phone_scan,
        };
        workers::spawn_string_workers(
            workers,
            cfg.run_id.clone(),
            rx,
            meta_tx.clone(),
            artefacts_found.clone(),
            scan_cfg,
        )
    } else {
        Vec::new()
    };

    // Process chunks
    let max_bytes = max_bytes.unwrap_or(u64::MAX);
    let max_chunks = max_chunks.unwrap_or(u64::MAX);
    let mut chunks_seen = 0u64;
    let mut hit_max_bytes = resume_offset >= max_bytes;
    let mut hit_max_chunks = resume_chunks >= max_chunks;
    let mut hit_max_files = false;
    let mut cancelled = false;
    let start_time = Instant::now();
    let mut last_progress = Instant::now();
    let mut next_offset = resume_offset;

    for chunk in chunks {
        if hit_max_bytes || hit_max_chunks {
            break;
        }
        if chunk.start < resume_offset {
            continue;
        }
        if let Some(limit) = cfg.max_files {
            if files_carved.load(Ordering::Relaxed) >= limit {
                hit_max_files = true;
                break;
            }
        }
        if let Some(flag) = &cancel_flag {
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
        let scanned_total = bytes_scanned
            .load(Ordering::Relaxed)
            .saturating_add(resume_offset);
        if scanned_total >= max_bytes {
            hit_max_bytes = true;
            break;
        }
        let remaining = (max_bytes - scanned_total).min(chunk.length) as usize;
        let data = read_chunk_limited(evidence.as_ref(), &chunk, remaining)?;
        if data.is_empty() {
            break;
        }
        bytes_scanned.fetch_add(data.len() as u64, Ordering::Relaxed);
        chunks_processed.fetch_add(1, Ordering::Relaxed);
        chunks_seen += 1;
        next_offset = chunk.start.saturating_add(chunk_size);
        let chunk_id = chunk.id;
        scan_tx
            .send(ScanJob {
                chunk,
                data: Arc::new(data),
            })
            .with_context(|| format!("scan channel closed while sending chunk {chunk_id}"))?;
        if let Some(progress) = &progress {
            if progress.interval.is_zero() || last_progress.elapsed() >= progress.interval {
                let snapshot = build_progress_snapshot(
                    total_bytes,
                    resume_offset,
                    &start_time,
                    &bytes_scanned,
                    &chunks_processed,
                    &hits_found,
                    &files_carved,
                    &string_spans,
                    &artefacts_found,
                    &carve_errors,
                    &metadata_errors,
                    &sqlite_errors,
                );
                progress.reporter.on_progress(&snapshot);
                last_progress = Instant::now();
            }
        }
        let scanned_total = bytes_scanned
            .load(Ordering::Relaxed)
            .saturating_add(resume_offset);
        if scanned_total >= max_bytes {
            hit_max_bytes = true;
            break;
        }
    }

    // Close channels and wait for workers
    drop(scan_tx);
    drop(hit_tx);
    drop(string_tx);

    for handle in scan_handles {
        let _ = handle.join();
    }
    for handle in carve_handles {
        let _ = handle.join();
    }
    for handle in string_handles {
        let _ = handle.join();
    }

    // Send run summary
    let bytes_scanned_total = bytes_scanned
        .load(Ordering::Relaxed)
        .saturating_add(resume_offset);
    let chunks_processed_total = chunks_processed
        .load(Ordering::Relaxed)
        .saturating_add(resume_chunks);
    let summary = RunSummary {
        run_id: cfg.run_id.clone(),
        bytes_scanned: bytes_scanned_total,
        chunks_processed: chunks_processed_total,
        hits_found: hits_found.load(Ordering::Relaxed),
        files_carved: files_carved.load(Ordering::Relaxed),
        string_spans: string_spans.load(Ordering::Relaxed),
        artefacts_extracted: artefacts_found.load(Ordering::Relaxed),
    };
    if let Err(err) = meta_tx.send(MetadataEvent::RunSummary(summary)) {
        warn!("metadata channel closed while sending run summary: {err}");
    }

    drop(meta_tx);
    let _ = meta_handle.join();

    if let Some(progress) = &progress {
        let snapshot = build_progress_snapshot(
            total_bytes,
            resume_offset,
            &start_time,
            &bytes_scanned,
            &chunks_processed,
            &hits_found,
            &files_carved,
            &string_spans,
            &artefacts_found,
            &carve_errors,
            &metadata_errors,
            &sqlite_errors,
        );
        progress.reporter.on_progress(&snapshot);
    }

    if cancelled {
        info!("shutdown requested; stopping early");
    }
    if hit_max_files {
        info!("max_files limit reached; stopping early");
    }
    if hit_max_bytes {
        info!("max_bytes limit reached; stopping early");
    }
    if hit_max_chunks {
        info!("max_chunks limit reached; stopping early");
    }

    let stats = PipelineStats {
        bytes_scanned: bytes_scanned_total,
        chunks_processed: chunks_processed_total,
        hits_found: hits_found.load(Ordering::Relaxed),
        files_carved: files_carved.load(Ordering::Relaxed),
        string_spans: string_spans.load(Ordering::Relaxed),
        artefacts_extracted: artefacts_found.load(Ordering::Relaxed),
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

    if cancelled || hit_max_bytes || hit_max_chunks || hit_max_files {
        if let Some(path) = checkpoint_path {
            let state = CheckpointState::new(
                &cfg.run_id,
                chunk_size,
                overlap,
                next_offset.min(total_bytes),
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
