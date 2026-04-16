//! # Pipeline Workers
//!
//! Worker thread spawning and management for the processing pipeline.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender};
use tracing::{debug, warn};

use crate::carve::{CarveRegistry, ExtractionContext};
use crate::chunk::ScanChunk;
use crate::entropy;
use crate::evidence::EvidenceSource;
use crate::metadata::MetadataSink;
use crate::scanner::{NormalizedHit, SignatureScanner};
use crate::strings::artifacts::ArtefactScanConfig;
use crate::strings::{self, StringScanner, StringSpan};

use super::EntropyConfig;
use super::events::MetadataEvent;
use super::limiter::CarveLimiter;

/// Job containing a chunk of data to scan
pub struct ScanJob {
    pub chunk: ScanChunk,
    pub data: Arc<Vec<u8>>,
}

/// Job containing string spans to process for artefacts
pub struct StringJob {
    pub chunk: ScanChunk,
    pub data: Arc<Vec<u8>>,
    pub spans: Vec<StringSpan>,
}

/// Spawn the metadata recording thread
pub fn spawn_metadata_thread(
    sink: Box<dyn MetadataSink>,
    rx: Receiver<MetadataEvent>,
    error_count: Arc<AtomicU64>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        for event in rx {
            match event {
                MetadataEvent::File(file) => {
                    if let Err(err) = sink.record_file(&file) {
                        error_count.fetch_add(1, Ordering::Relaxed);
                        warn!("metadata record error: {err}");
                    }
                }
                MetadataEvent::String(artefact) => {
                    if let Err(err) = sink.record_string(&artefact) {
                        error_count.fetch_add(1, Ordering::Relaxed);
                        warn!("metadata record error: {err}");
                    }
                }
                MetadataEvent::History(record) => {
                    if let Err(err) = sink.record_history(&record) {
                        error_count.fetch_add(1, Ordering::Relaxed);
                        warn!("metadata record error: {err}");
                    }
                }
                MetadataEvent::Cookie(record) => {
                    if let Err(err) = sink.record_cookie(&record) {
                        error_count.fetch_add(1, Ordering::Relaxed);
                        warn!("metadata record error: {err}");
                    }
                }
                MetadataEvent::Download(record) => {
                    if let Err(err) = sink.record_download(&record) {
                        error_count.fetch_add(1, Ordering::Relaxed);
                        warn!("metadata record error: {err}");
                    }
                }
                MetadataEvent::RunSummary(summary) => {
                    if let Err(err) = sink.record_run_summary(&summary) {
                        error_count.fetch_add(1, Ordering::Relaxed);
                        warn!("metadata record error: {err}");
                    }
                }
                MetadataEvent::Entropy(region) => {
                    if let Err(err) = sink.record_entropy(&region) {
                        error_count.fetch_add(1, Ordering::Relaxed);
                        warn!("metadata record error: {err}");
                    }
                }
                MetadataEvent::Flush => {
                    if let Err(err) = sink.flush() {
                        error_count.fetch_add(1, Ordering::Relaxed);
                        warn!("metadata flush error: {err}");
                    }
                }
            }
        }
        // Final flush when channel closes
        if let Err(err) = sink.flush() {
            error_count.fetch_add(1, Ordering::Relaxed);
            warn!("metadata flush error: {err}");
        }
    })
}

/// Spawn signature scanning worker threads
#[allow(clippy::too_many_arguments)]
pub fn spawn_scan_workers(
    workers: usize,
    scanner: Arc<dyn SignatureScanner>,
    string_scanner: Option<Arc<dyn StringScanner>>,
    rx: Receiver<ScanJob>,
    hit_tx: Sender<NormalizedHit>,
    string_tx: Option<Sender<StringJob>>,
    meta_tx: Sender<MetadataEvent>,
    run_id: String,
    entropy_cfg: Option<EntropyConfig>,
    hits_found: Arc<AtomicU64>,
    string_spans: Arc<AtomicU64>,
    sqlite_page_max_hits_per_chunk: usize,
    scan_time_ms: Arc<AtomicU64>,
) -> Vec<thread::JoinHandle<()>> {
    let mut handles = Vec::new();
    let worker_count = workers.max(1);

    for _ in 0..worker_count {
        let scanner = scanner.clone();
        let rx = rx.clone();
        let hit_tx = hit_tx.clone();
        let string_scanner = string_scanner.clone();
        let string_tx = string_tx.clone();
        let hits_found = hits_found.clone();
        let string_spans = string_spans.clone();
        let meta_tx = meta_tx.clone();
        let run_id = run_id.clone();
        let scan_time_ms = scan_time_ms.clone();
        let sqlite_page_max_hits_per_chunk = sqlite_page_max_hits_per_chunk.max(1);

        handles.push(thread::spawn(move || {
            for job in rx {
                let effective_valid = job.chunk.valid_length.min(job.data.len() as u64);
                let valid_len = effective_valid as usize;
                let mut sqlite_page_hits = 0usize;
                let mut sqlite_page_hits_dropped = 0usize;

                // Scan for file signatures
                let scan_start = Instant::now();
                let scan_hits = scanner.scan_chunk(&job.chunk, &job.data);
                scan_time_ms.fetch_add(scan_start.elapsed().as_millis() as u64, Ordering::Relaxed);
                for hit in scan_hits {
                    if hit.local_offset >= effective_valid {
                        continue;
                    }
                    if hit.file_type_id == "sqlite_page" {
                        if sqlite_page_hits >= sqlite_page_max_hits_per_chunk {
                            sqlite_page_hits_dropped = sqlite_page_hits_dropped.saturating_add(1);
                            continue;
                        }
                        sqlite_page_hits = sqlite_page_hits.saturating_add(1);
                    }
                    hits_found.fetch_add(1, Ordering::Relaxed);
                    let global_offset = job.chunk.start + hit.local_offset;
                    let normalized = NormalizedHit {
                        global_offset,
                        file_type_id: hit.file_type_id,
                        pattern_id: hit.pattern_id,
                        chunk_data: Some(Arc::clone(&job.data)),
                        chunk_start: job.chunk.start,
                    };
                    if let Err(err) = hit_tx.send(normalized) {
                        warn!("hit channel closed while sending hit: {err}");
                        break;
                    }
                }
                if sqlite_page_hits_dropped > 0 {
                    debug!(
                        "chunk {} sqlite_page hits capped: kept={} dropped={} cap={}",
                        job.chunk.id,
                        sqlite_page_hits,
                        sqlite_page_hits_dropped,
                        sqlite_page_max_hits_per_chunk
                    );
                }

                // Scan for strings if enabled
                if let (Some(scanner), Some(tx)) = (&string_scanner, &string_tx) {
                    let spans = scanner.scan_chunk(&job.chunk, &job.data);
                    if !spans.is_empty() {
                        let filtered: Vec<StringSpan> = spans
                            .into_iter()
                            .filter(|span| span.local_start < effective_valid)
                            .collect();
                        if !filtered.is_empty() {
                            string_spans.fetch_add(filtered.len() as u64, Ordering::Relaxed);
                            let string_job = StringJob {
                                chunk: job.chunk.clone(),
                                data: Arc::clone(&job.data),
                                spans: filtered,
                            };
                            if let Err(err) = tx.send(string_job) {
                                warn!("string channel closed while sending spans: {err}");
                                break;
                            }
                        }
                    }
                }

                // Detect high entropy regions if enabled
                if let Some(cfg) = entropy_cfg
                    && valid_len >= cfg.window_size
                {
                    let regions = entropy::detect_entropy_regions(
                        &run_id,
                        job.chunk.start,
                        &job.data[..valid_len],
                        cfg.window_size,
                        cfg.threshold,
                    );
                    for region in regions {
                        if let Err(err) = meta_tx.send(MetadataEvent::Entropy(region)) {
                            warn!("metadata channel closed while sending entropy region: {err}");
                            break;
                        }
                    }
                }
            }
        }));
    }

    handles
}

/// Spawn file carving worker threads
#[allow(clippy::too_many_arguments)]
pub fn spawn_carve_workers(
    workers: usize,
    registry: Arc<CarveRegistry>,
    evidence: Arc<dyn EvidenceSource>,
    run_id: String,
    run_output_dir: PathBuf,
    rx: Receiver<NormalizedHit>,
    meta_tx: Sender<MetadataEvent>,
    carve_limiter: Arc<CarveLimiter>,
    carve_errors: Arc<AtomicU64>,
    carve_time_ms: Arc<AtomicU64>,
    files_rejected: Arc<AtomicU64>,
    files_prevalidation_rejected: Arc<AtomicU64>,
    deferred_buffer_bytes: usize,
    metadata_only: bool,
) -> Vec<thread::JoinHandle<()>> {
    let mut handles = Vec::new();
    let worker_count = workers.max(1);

    for _ in 0..worker_count {
        let registry = registry.clone();
        let evidence = evidence.clone();
        let run_id = run_id.clone();
        let run_output_dir = run_output_dir.clone();
        let rx = rx.clone();
        let meta_tx = meta_tx.clone();
        let carve_limiter = carve_limiter.clone();
        let carve_errors = carve_errors.clone();
        let carve_time_ms = carve_time_ms.clone();
        let files_rejected = files_rejected.clone();
        let files_prevalidation_rejected = files_prevalidation_rejected.clone();

        handles.push(thread::spawn(move || {
            let carved_root = run_output_dir.join("carved");
            let mut ctx = ExtractionContext {
                run_id: &run_id,
                output_root: &carved_root,
                evidence: evidence.as_ref(),
                deferred_buffer_bytes,
                metadata_only,
                io_buf: std::cell::RefCell::new(Vec::new()),
                chunk_data: None,
                chunk_start: 0,
            };

            for hit in rx {
                ctx.chunk_data = hit.chunk_data.clone();
                ctx.chunk_start = hit.chunk_start;
                let handler = match registry.get(&hit.file_type_id) {
                    Some(handler) => handler,
                    None => {
                        debug!("no handler for file_type={}", hit.file_type_id);
                        continue;
                    }
                };

                if !carve_limiter.try_reserve() {
                    continue;
                }

                match handler.pre_validate(evidence.as_ref(), hit.global_offset) {
                    Ok(crate::carve::PreValidation::Proceed) => { /* continue to process_hit */ }
                    Ok(crate::carve::PreValidation::Reject(reason)) => {
                        carve_limiter.release();
                        files_prevalidation_rejected.fetch_add(1, Ordering::Relaxed);
                        debug!(
                            "pre_validate rejected {} at offset {}: {reason}",
                            hit.file_type_id, hit.global_offset
                        );
                        continue;
                    }
                    Err(err) => {
                        carve_limiter.release();
                        carve_errors.fetch_add(1, Ordering::Relaxed);
                        warn!(
                            "pre_validate error for {} at offset {}: {err}",
                            hit.file_type_id, hit.global_offset
                        );
                        continue;
                    }
                }

                let carve_start = Instant::now();
                let result = handler.process_hit(&hit, &ctx);
                carve_time_ms
                    .fetch_add(carve_start.elapsed().as_millis() as u64, Ordering::Relaxed);
                match result {
                    Ok(Some(file)) => {
                        carve_limiter.commit();
                        if let Err(err) = meta_tx.send(MetadataEvent::File(file)) {
                            warn!("metadata channel closed while sending carved file: {err}");
                        }
                    }
                    Ok(None) => {
                        carve_limiter.release();
                        files_rejected.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(err) => {
                        carve_limiter.release();
                        carve_errors.fetch_add(1, Ordering::Relaxed);
                        warn!("carve error at offset {}: {err}", hit.global_offset);
                    }
                }
            }
        }));
    }

    handles
}

/// Spawn string artefact extraction worker threads
pub fn spawn_string_workers(
    workers: usize,
    run_id: String,
    rx: Receiver<StringJob>,
    meta_tx: Sender<MetadataEvent>,
    artefacts_found: Arc<AtomicU64>,
    scan_cfg: ArtefactScanConfig,
) -> Vec<thread::JoinHandle<()>> {
    let mut handles = Vec::new();
    let worker_count = workers.max(1);

    for _ in 0..worker_count {
        let rx = rx.clone();
        let meta_tx = meta_tx.clone();
        let run_id = run_id.clone();
        let artefacts_found = artefacts_found.clone();

        handles.push(thread::spawn(move || {
            for job in rx {
                for span in job.spans {
                    let start = span.local_start as usize;
                    let end = start.saturating_add(span.length as usize);
                    if end > job.data.len() {
                        continue;
                    }
                    let slice = &job.data[start..end];
                    let artefacts = strings::artifacts::extract_artefacts(
                        &run_id,
                        job.chunk.start,
                        span.local_start,
                        span.flags,
                        slice,
                        scan_cfg,
                    );
                    artefacts_found.fetch_add(artefacts.len() as u64, Ordering::Relaxed);
                    for artefact in artefacts {
                        if let Err(err) = meta_tx.send(MetadataEvent::String(artefact)) {
                            warn!("metadata channel closed while sending string artefact: {err}");
                            break;
                        }
                    }
                }
            }
        }));
    }

    handles
}
