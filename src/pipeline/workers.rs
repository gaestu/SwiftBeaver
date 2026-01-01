//! # Pipeline Workers
//!
//! Worker thread spawning and management for the processing pipeline.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;

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

use super::events::MetadataEvent;
use super::EntropyConfig;

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
            }
        }
        if let Err(err) = sink.flush() {
            error_count.fetch_add(1, Ordering::Relaxed);
            warn!("metadata flush error: {err}");
        }
    })
}

/// Spawn signature scanning worker threads
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
        let entropy_cfg = entropy_cfg;

        handles.push(thread::spawn(move || {
            for job in rx {
                let effective_valid = job.chunk.valid_length.min(job.data.len() as u64);
                let valid_len = effective_valid as usize;

                // Scan for file signatures
                for hit in scanner.scan_chunk(&job.chunk, &job.data) {
                    if hit.local_offset >= effective_valid {
                        continue;
                    }
                    hits_found.fetch_add(1, Ordering::Relaxed);
                    let global_offset = job.chunk.start + hit.local_offset;
                    let normalized = NormalizedHit {
                        global_offset,
                        file_type_id: hit.file_type_id,
                        pattern_id: hit.pattern_id,
                    };
                    if let Err(err) = hit_tx.send(normalized) {
                        warn!("hit channel closed while sending hit: {err}");
                        break;
                    }
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
                if let Some(cfg) = entropy_cfg {
                    if valid_len >= cfg.window_size {
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
            }
        }));
    }

    handles
}

/// Spawn file carving worker threads
pub fn spawn_carve_workers(
    workers: usize,
    registry: Arc<CarveRegistry>,
    evidence: Arc<dyn EvidenceSource>,
    run_id: String,
    run_output_dir: PathBuf,
    rx: Receiver<NormalizedHit>,
    meta_tx: Sender<MetadataEvent>,
    files_carved: Arc<AtomicU64>,
    enable_sqlite_page_recovery: bool,
    max_files: Option<u64>,
    carve_errors: Arc<AtomicU64>,
    sqlite_errors: Arc<AtomicU64>,
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
        let files_carved = files_carved.clone();
        let max_files = max_files;
        let carve_errors = carve_errors.clone();
        let sqlite_errors = sqlite_errors.clone();

        handles.push(thread::spawn(move || {
            let carved_root = run_output_dir.join("carved");
            let ctx = ExtractionContext {
                run_id: &run_id,
                output_root: &carved_root,
                evidence: evidence.as_ref(),
            };

            for hit in rx {
                if let Some(limit) = max_files {
                    if files_carved.load(Ordering::Relaxed) >= limit {
                        break;
                    }
                }
                let handler = match registry.get(&hit.file_type_id) {
                    Some(handler) => handler,
                    None => {
                        debug!("no handler for file_type={}", hit.file_type_id);
                        continue;
                    }
                };

                match handler.process_hit(&hit, &ctx) {
                    Ok(Some(file)) => {
                        let new_total = files_carved.fetch_add(1, Ordering::Relaxed) + 1;
                        let path = carved_root.join(&file.path);
                        let file_type = file.file_type.clone();
                        let rel_path = file.path.clone();
                        if let Err(err) = meta_tx.send(MetadataEvent::File(file)) {
                            warn!("metadata channel closed while sending carved file: {err}");
                        }

                        // Process SQLite files for browser artifacts
                        if file_type == "sqlite" {
                            process_sqlite_artifacts(
                                &path,
                                &run_id,
                                &rel_path,
                                &meta_tx,
                                enable_sqlite_page_recovery,
                                &sqlite_errors,
                            );
                        }
                        if let Some(limit) = max_files {
                            if new_total >= limit {
                                break;
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(err) => {
                        carve_errors.fetch_add(1, Ordering::Relaxed);
                        warn!("carve error at offset {}: {err}", hit.global_offset);
                    }
                }
            }
        }));
    }

    handles
}

/// Process SQLite files for browser artifacts (history, cookies, downloads)
fn process_sqlite_artifacts(
    path: &std::path::Path,
    run_id: &str,
    rel_path: &str,
    meta_tx: &Sender<MetadataEvent>,
    enable_page_recovery: bool,
    sqlite_errors: &Arc<AtomicU64>,
) {
    // Extract browser history
    let mut records = match crate::parsers::sqlite_db::extract_browser_history(path, run_id, rel_path)
    {
        Ok(records) => records,
        Err(err) => {
            sqlite_errors.fetch_add(1, Ordering::Relaxed);
            warn!("sqlite parse failed for {}: {err}", path.display());
            Vec::new()
        }
    };

    // Try page-level recovery if no records found
    if records.is_empty() && enable_page_recovery {
        match crate::parsers::sqlite_pages::extract_history_from_pages(path, run_id, rel_path) {
            Ok(mut recovered) => records.append(&mut recovered),
            Err(err) => {
                sqlite_errors.fetch_add(1, Ordering::Relaxed);
                warn!("sqlite page recovery failed for {}: {err}", path.display());
            }
        }
    }

    for record in records {
        if let Err(err) = meta_tx.send(MetadataEvent::History(record)) {
            warn!("metadata channel closed while sending history record: {err}");
            return;
        }
    }

    // Extract browser cookies
    match crate::parsers::sqlite_db::extract_browser_cookies(path, run_id, rel_path) {
        Ok(records) => {
            for record in records {
                if let Err(err) = meta_tx.send(MetadataEvent::Cookie(record)) {
                    warn!("metadata channel closed while sending cookie record: {err}");
                    return;
                }
            }
        }
        Err(err) => {
            sqlite_errors.fetch_add(1, Ordering::Relaxed);
            warn!("sqlite cookie parse failed for {}: {err}", path.display());
        }
    }

    // Extract browser downloads
    match crate::parsers::sqlite_db::extract_browser_downloads(path, run_id, rel_path) {
        Ok(records) => {
            for record in records {
                if let Err(err) = meta_tx.send(MetadataEvent::Download(record)) {
                    warn!("metadata channel closed while sending download record: {err}");
                    return;
                }
            }
        }
        Err(err) => {
            sqlite_errors.fetch_add(1, Ordering::Relaxed);
            warn!("sqlite download parse failed for {}: {err}", path.display());
        }
    }
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
