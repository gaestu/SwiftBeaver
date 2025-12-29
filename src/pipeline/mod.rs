//! # Pipeline Module
//!
//! Orchestrates the scanning, carving, and metadata recording pipeline.
//! This module handles multi-threaded processing of evidence sources.

pub mod events;
pub mod workers;

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::Result;
use crossbeam_channel::bounded;
use tracing::info;

use crate::carve::CarveRegistry;
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
    let chunks = build_chunks(evidence.len(), chunk_size, overlap);
    info!(
        "chunk_count={} chunk_size={} overlap={}",
        chunks.len(),
        chunk_size,
        overlap
    );

    // Create channels
    let channel_cap = workers.saturating_mul(CHANNEL_CAPACITY_MULTIPLIER).max(MIN_CHANNEL_CAPACITY);
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

    // Start metadata recording thread
    let meta_handle = workers::spawn_metadata_thread(meta_sink, meta_rx);

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
    let mut hit_max_bytes = false;
    let mut hit_max_chunks = false;

    for chunk in chunks {
        if chunks_seen >= max_chunks {
            hit_max_chunks = true;
            break;
        }
        let scanned = bytes_scanned.load(Ordering::Relaxed);
        if scanned >= max_bytes {
            hit_max_bytes = true;
            break;
        }
        let remaining = (max_bytes - scanned).min(chunk.length) as usize;
        let data = read_chunk_limited(evidence.as_ref(), &chunk, remaining)?;
        if data.is_empty() {
            break;
        }
        bytes_scanned.fetch_add(data.len() as u64, Ordering::Relaxed);
        chunks_processed.fetch_add(1, Ordering::Relaxed);
        chunks_seen += 1;
        scan_tx.send(ScanJob {
            chunk,
            data: Arc::new(data),
        })?;
        if bytes_scanned.load(Ordering::Relaxed) >= max_bytes {
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
    let summary = RunSummary {
        run_id: cfg.run_id.clone(),
        bytes_scanned: bytes_scanned.load(Ordering::Relaxed),
        chunks_processed: chunks_processed.load(Ordering::Relaxed),
        hits_found: hits_found.load(Ordering::Relaxed),
        files_carved: files_carved.load(Ordering::Relaxed),
        string_spans: string_spans.load(Ordering::Relaxed),
        artefacts_extracted: artefacts_found.load(Ordering::Relaxed),
    };
    let _ = meta_tx.send(MetadataEvent::RunSummary(summary));

    drop(meta_tx);
    let _ = meta_handle.join();

    if hit_max_bytes {
        info!("max_bytes limit reached; stopping early");
    }
    if hit_max_chunks {
        info!("max_chunks limit reached; stopping early");
    }

    let stats = PipelineStats {
        bytes_scanned: bytes_scanned.load(Ordering::Relaxed),
        chunks_processed: chunks_processed.load(Ordering::Relaxed),
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

    Ok(stats)
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
