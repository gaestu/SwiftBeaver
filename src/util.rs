use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;

use anyhow::Result;
use crossbeam_channel::{bounded, Receiver, Sender};
use tracing::{debug, info, warn};

use crate::carve::{CarveRegistry, ExtractionContext};
use crate::chunk::{build_chunks, ScanChunk};
use crate::config::Config;
use crate::evidence::EvidenceSource;
use crate::metadata::{MetadataBackendKind, MetadataSink};
use crate::scanner::{NormalizedHit, SignatureScanner};
use crate::strings::{self, StringScanner, StringSpan};
use crate::carve;

struct ScanJob {
    chunk: ScanChunk,
    data: Arc<Vec<u8>>,
}

struct StringJob {
    chunk: ScanChunk,
    data: Arc<Vec<u8>>,
    spans: Vec<StringSpan>,
}

enum MetadataEvent {
    File(crate::carve::CarvedFile),
    String(crate::strings::artifacts::StringArtefact),
    History(crate::parsers::browser::BrowserHistoryRecord),
}

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
) -> Result<()> {
    let carve_registry = Arc::new(build_carve_registry(cfg));

    let chunks = build_chunks(evidence.len(), chunk_size, overlap);
    info!("chunk_count={} chunk_size={} overlap={}", chunks.len(), chunk_size, overlap);

    let (scan_tx, scan_rx) = bounded::<ScanJob>(workers.saturating_mul(2).max(1));
    let (hit_tx, hit_rx) = bounded::<NormalizedHit>(workers.saturating_mul(4).max(1));
    let (meta_tx, meta_rx) = bounded::<MetadataEvent>(workers.saturating_mul(4).max(1));
    let (string_tx, string_rx) = if string_scanner.is_some() {
        let cap = workers.saturating_mul(2).max(1);
        let (tx, rx) = bounded::<StringJob>(cap);
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    let bytes_scanned = Arc::new(AtomicU64::new(0));
    let hits_found = Arc::new(AtomicU64::new(0));
    let files_carved = Arc::new(AtomicU64::new(0));

    let meta_handle = spawn_metadata_thread(meta_sink, meta_rx);

    let scan_handles = spawn_scan_workers(
        workers,
        sig_scanner.clone(),
        string_scanner.clone(),
        scan_rx,
        hit_tx.clone(),
        string_tx.clone(),
        hits_found.clone(),
    );

    let carve_handles = spawn_carve_workers(
        workers,
        carve_registry.clone(),
        evidence.clone(),
        cfg.run_id.clone(),
        run_output_dir.to_path_buf(),
        hit_rx,
        meta_tx.clone(),
        files_carved.clone(),
    );

    let string_handles = if let Some(rx) = string_rx {
        spawn_string_workers(
            workers,
            cfg.run_id.clone(),
            rx,
            meta_tx.clone(),
        )
    } else {
        Vec::new()
    };

    for chunk in chunks {
        let data = read_chunk(evidence.as_ref(), &chunk)?;
        if data.is_empty() {
            break;
        }
        bytes_scanned.fetch_add(data.len() as u64, Ordering::Relaxed);
        scan_tx.send(ScanJob { chunk, data: Arc::new(data) })?;
    }

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

    drop(meta_tx);
    let _ = meta_handle.join();

    info!(
        "run_summary bytes_scanned={} hits_found={} files_carved={}",
        bytes_scanned.load(Ordering::Relaxed),
        hits_found.load(Ordering::Relaxed),
        files_carved.load(Ordering::Relaxed)
    );

    Ok(())
}

fn spawn_metadata_thread(
    sink: Box<dyn MetadataSink>,
    rx: Receiver<MetadataEvent>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        for event in rx {
            match event {
                MetadataEvent::File(file) => {
                    if let Err(err) = sink.record_file(&file) {
                        warn!("metadata record error: {err}");
                    }
                }
                MetadataEvent::String(artefact) => {
                    if let Err(err) = sink.record_string(&artefact) {
                        warn!("metadata record error: {err}");
                    }
                }
                MetadataEvent::History(record) => {
                    if let Err(err) = sink.record_history(&record) {
                        warn!("metadata record error: {err}");
                    }
                }
            }
        }
        if let Err(err) = sink.flush() {
            warn!("metadata flush error: {err}");
        }
    })
}

fn spawn_scan_workers(
    workers: usize,
    scanner: Arc<dyn SignatureScanner>,
    string_scanner: Option<Arc<dyn StringScanner>>,
    rx: Receiver<ScanJob>,
    hit_tx: Sender<NormalizedHit>,
    string_tx: Option<Sender<StringJob>>,
    hits_found: Arc<AtomicU64>,
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
        handles.push(thread::spawn(move || {
            for job in rx {
                let effective_valid = job.chunk.valid_length.min(job.data.len() as u64);
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
                    if hit_tx.send(normalized).is_err() {
                        break;
                    }
                }

                if let (Some(scanner), Some(tx)) = (&string_scanner, &string_tx) {
                    let spans = scanner.scan_chunk(&job.chunk, &job.data);
                    if !spans.is_empty() {
                        let filtered: Vec<StringSpan> = spans
                            .into_iter()
                            .filter(|span| span.local_start < effective_valid)
                            .collect();
                        if !filtered.is_empty() {
                            let job = StringJob {
                                chunk: job.chunk.clone(),
                                data: Arc::clone(&job.data),
                                spans: filtered,
                            };
                            if tx.send(job).is_err() {
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

fn spawn_carve_workers(
    workers: usize,
    registry: Arc<CarveRegistry>,
    evidence: Arc<dyn EvidenceSource>,
    run_id: String,
    run_output_dir: std::path::PathBuf,
    rx: Receiver<NormalizedHit>,
    meta_tx: Sender<MetadataEvent>,
    files_carved: Arc<AtomicU64>,
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

        handles.push(thread::spawn(move || {
            let carved_root = run_output_dir.join("carved");
            let ctx = ExtractionContext {
                run_id: &run_id,
                output_root: &carved_root,
                evidence: evidence.as_ref(),
            };

            for hit in rx {
                let handler = match registry.get(&hit.file_type_id) {
                    Some(handler) => handler,
                    None => {
                        debug!("no handler for file_type={}", hit.file_type_id);
                        continue;
                    }
                };

                match handler.process_hit(&hit, &ctx) {
                    Ok(Some(file)) => {
                        files_carved.fetch_add(1, Ordering::Relaxed);
                        let path = carved_root.join(&file.path);
                        let file_type = file.file_type.clone();
                        let rel_path = file.path.clone();
                        let _ = meta_tx.send(MetadataEvent::File(file));

                        if file_type == "sqlite" {
                            if let Ok(records) = crate::parsers::sqlite_db::extract_browser_history(
                                &path,
                                &run_id,
                                &rel_path,
                            ) {
                                for record in records {
                                    let _ = meta_tx.send(MetadataEvent::History(record));
                                }
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(err) => {
                        warn!("carve error at offset {}: {err}", hit.global_offset);
                    }
                }
            }
        }));
    }

    handles
}

fn spawn_string_workers(
    workers: usize,
    run_id: String,
    rx: Receiver<StringJob>,
    meta_tx: Sender<MetadataEvent>,
) -> Vec<thread::JoinHandle<()>> {
    let mut handles = Vec::new();
    let worker_count = workers.max(1);

    for _ in 0..worker_count {
        let rx = rx.clone();
        let meta_tx = meta_tx.clone();
        let run_id = run_id.clone();
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
                        slice,
                    );
                    for artefact in artefacts {
                        let _ = meta_tx.send(MetadataEvent::String(artefact));
                    }
                }
            }
        }));
    }

    handles
}

fn read_chunk(evidence: &dyn EvidenceSource, chunk: &ScanChunk) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; chunk.length as usize];
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

fn build_carve_registry(cfg: &Config) -> CarveRegistry {
    let mut handlers: std::collections::HashMap<String, Box<dyn carve::CarveHandler>> =
        std::collections::HashMap::new();

    for file_type in &cfg.file_types {
        let ext = file_type
            .extensions
            .get(0)
            .cloned()
            .unwrap_or_else(|| file_type.id.clone());
        let ext = carve::sanitize_extension(&ext);

        match file_type.id.as_str() {
            "jpeg" => {
                handlers.insert(
                    file_type.id.clone(),
                    Box::new(carve::jpeg::JpegCarveHandler::new(
                        ext,
                        file_type.min_size,
                        file_type.max_size,
                    )),
                );
            }
            "png" => {
                handlers.insert(
                    file_type.id.clone(),
                    Box::new(carve::png::PngCarveHandler::new(
                        ext,
                        file_type.min_size,
                        file_type.max_size,
                    )),
                );
            }
            "gif" => {
                handlers.insert(
                    file_type.id.clone(),
                    Box::new(carve::gif::GifCarveHandler::new(
                        ext,
                        file_type.min_size,
                        file_type.max_size,
                    )),
                );
            }
            "sqlite" => {
                handlers.insert(
                    file_type.id.clone(),
                    Box::new(carve::sqlite::SqliteCarveHandler::new(
                        ext,
                        file_type.min_size,
                        file_type.max_size,
                    )),
                );
            }
            "pdf" => {
                handlers.insert(
                    file_type.id.clone(),
                    Box::new(carve::pdf::PdfCarveHandler::new(
                        ext,
                        file_type.min_size,
                        file_type.max_size,
                    )),
                );
            }
            "zip" => {
                handlers.insert(
                    file_type.id.clone(),
                    Box::new(carve::zip::ZipCarveHandler::new(
                        ext,
                        file_type.min_size,
                        file_type.max_size,
                        file_type.require_eocd,
                    )),
                );
            }
            "webp" => {
                handlers.insert(
                    file_type.id.clone(),
                    Box::new(carve::webp::WebpCarveHandler::new(
                        ext,
                        file_type.min_size,
                        file_type.max_size,
                    )),
                );
            }
            _ => {
                debug!("no carve handler for file_type={}", file_type.id);
            }
        }
    }

    CarveRegistry::new(handlers)
}

pub fn backend_from_cli(backend: crate::cli::MetadataBackend) -> MetadataBackendKind {
    match backend {
        crate::cli::MetadataBackend::Jsonl => MetadataBackendKind::Jsonl,
        crate::cli::MetadataBackend::Csv => MetadataBackendKind::Csv,
        crate::cli::MetadataBackend::Parquet => MetadataBackendKind::Parquet,
    }
}
