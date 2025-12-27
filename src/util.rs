use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;

use anyhow::{anyhow, Result};
use crossbeam_channel::{bounded, Receiver, Sender};
use tracing::{debug, info, warn};

use crate::carve::{CarveRegistry, ExtractionContext};
use crate::chunk::{build_chunks, ScanChunk};
use crate::config::Config;
use crate::evidence::EvidenceSource;
use crate::metadata::{MetadataBackendKind, MetadataSink, RunSummary};
use crate::scanner::{NormalizedHit, SignatureScanner};
use crate::strings::{self, StringScanner, StringSpan};
use crate::strings::artifacts::ArtefactScanConfig;
use crate::carve;
use crate::entropy;

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
    RunSummary(RunSummary),
    Entropy(crate::metadata::EntropyRegion),
}

#[derive(Debug, Clone, Copy)]
struct EntropyConfig {
    window_size: usize,
    threshold: f64,
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
    let carve_registry = Arc::new(build_carve_registry(cfg)?);

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
    let chunks_processed = Arc::new(AtomicU64::new(0));
    let hits_found = Arc::new(AtomicU64::new(0));
    let files_carved = Arc::new(AtomicU64::new(0));
    let string_spans = Arc::new(AtomicU64::new(0));
    let artefacts_found = Arc::new(AtomicU64::new(0));

    let meta_handle = spawn_metadata_thread(meta_sink, meta_rx);

    let entropy_cfg = if cfg.enable_entropy_detection && cfg.entropy_window_size > 0 {
        Some(EntropyConfig {
            window_size: cfg.entropy_window_size,
            threshold: cfg.entropy_threshold,
        })
    } else {
        None
    };

    let scan_handles = spawn_scan_workers(
        workers,
        sig_scanner.clone(),
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
        let scan_cfg = ArtefactScanConfig {
            urls: cfg.enable_url_scan,
            emails: cfg.enable_email_scan,
            phones: cfg.enable_phone_scan,
        };
        spawn_string_workers(
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

    for chunk in chunks {
        let data = read_chunk(evidence.as_ref(), &chunk)?;
        if data.is_empty() {
            break;
        }
        bytes_scanned.fetch_add(data.len() as u64, Ordering::Relaxed);
        chunks_processed.fetch_add(1, Ordering::Relaxed);
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

    info!(
        "run_summary bytes_scanned={} chunks_processed={} hits_found={} files_carved={} string_spans={} artefacts_extracted={}",
        bytes_scanned.load(Ordering::Relaxed),
        chunks_processed.load(Ordering::Relaxed),
        hits_found.load(Ordering::Relaxed),
        files_carved.load(Ordering::Relaxed),
        string_spans.load(Ordering::Relaxed),
        artefacts_found.load(Ordering::Relaxed)
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
                MetadataEvent::RunSummary(summary) => {
                    if let Err(err) = sink.record_run_summary(&summary) {
                        warn!("metadata record error: {err}");
                    }
                }
                MetadataEvent::Entropy(region) => {
                    if let Err(err) = sink.record_entropy(&region) {
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
                            string_spans.fetch_add(filtered.len() as u64, Ordering::Relaxed);
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
                            let _ = meta_tx.send(MetadataEvent::Entropy(region));
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
        let scan_cfg = scan_cfg;
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

fn build_carve_registry(cfg: &Config) -> Result<CarveRegistry> {
    let mut handlers: std::collections::HashMap<String, Box<dyn carve::CarveHandler>> =
        std::collections::HashMap::new();

    for file_type in &cfg.file_types {
        let validator = if file_type.validator.trim().is_empty() {
            file_type.id.as_str()
        } else {
            file_type.validator.as_str()
        };
        let ext = file_type
            .extensions
            .get(0)
            .cloned()
            .unwrap_or_else(|| file_type.id.clone());
        let ext = carve::sanitize_extension(&ext);

        if !file_type.footer_patterns.is_empty() && validator != "footer" {
            debug!(
                "footer patterns configured for file_type={} but validator={} does not use them",
                file_type.id, validator
            );
        }

        match validator {
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
            "footer" => {
                let headers = decode_patterns(&file_type.header_patterns, &file_type.id, "header")?;
                let footers = decode_patterns(&file_type.footer_patterns, &file_type.id, "footer")?;
                if headers.is_empty() {
                    debug!("footer handler skipped for file_type={} (no header patterns)", file_type.id);
                    continue;
                }
                if footers.is_empty() {
                    debug!("footer handler skipped for file_type={} (no footer patterns)", file_type.id);
                    continue;
                }
                handlers.insert(
                    file_type.id.clone(),
                    Box::new(carve::footer::FooterCarveHandler::new(
                        file_type.id.clone(),
                        ext,
                        file_type.min_size,
                        file_type.max_size,
                        headers,
                        footers,
                    )),
                );
            }
            _ => {
                debug!(
                    "no carve handler for file_type={} validator={}",
                    file_type.id, validator
                );
            }
        }
    }

    Ok(CarveRegistry::new(handlers))
}

fn decode_patterns(
    patterns: &[crate::config::PatternConfig],
    file_type: &str,
    kind: &str,
) -> Result<Vec<Vec<u8>>> {
    let mut out = Vec::new();
    for pattern in patterns {
        let bytes = hex::decode(pattern.hex.trim()).map_err(|e| {
            anyhow!(
                "invalid {} pattern {} for file_type {}: {e}",
                kind,
                pattern.id,
                file_type
            )
        })?;
        if !bytes.is_empty() {
            out.push(bytes);
        }
    }
    Ok(out)
}

pub fn backend_from_cli(backend: crate::cli::MetadataBackend) -> MetadataBackendKind {
    match backend {
        crate::cli::MetadataBackend::Jsonl => MetadataBackendKind::Jsonl,
        crate::cli::MetadataBackend::Csv => MetadataBackendKind::Csv,
        crate::cli::MetadataBackend::Parquet => MetadataBackendKind::Parquet,
    }
}

pub fn filter_file_types(
    cfg: &mut Config,
    allow_list: Option<&[String]>,
    disable_zip: bool,
) -> Vec<String> {
    use std::collections::HashSet;

    let mut unknown = Vec::new();
    if let Some(list) = allow_list {
        let mut allow = HashSet::new();
        for entry in list {
            let trimmed = entry.trim();
            if trimmed.is_empty() {
                continue;
            }
            allow.insert(trimmed.to_ascii_lowercase());
        }

        let mut known = HashSet::new();
        for file_type in &cfg.file_types {
            known.insert(file_type.id.to_ascii_lowercase());
            if !file_type.validator.trim().is_empty() {
                known.insert(file_type.validator.to_ascii_lowercase());
            }
        }

        for entry in &allow {
            if !known.contains(entry) {
                unknown.push(entry.clone());
            }
        }

        cfg.file_types.retain(|file_type| {
            let id = file_type.id.to_ascii_lowercase();
            let validator = if file_type.validator.trim().is_empty() {
                id.clone()
            } else {
                file_type.validator.to_ascii_lowercase()
            };
            allow.contains(&id) || allow.contains(&validator)
        });
    }

    if disable_zip {
        cfg.file_types.retain(|file_type| {
            let is_zip = file_type.id.eq_ignore_ascii_case("zip")
                || file_type.validator.eq_ignore_ascii_case("zip");
            !is_zip
        });
    }

    unknown.sort();
    unknown
}

#[cfg(test)]
mod tests {
    use super::filter_file_types;
    use crate::config;

    #[test]
    fn filters_allowed_types() {
        let loaded = config::load_config(None).expect("config");
        let mut cfg = loaded.config;
        let unknown = filter_file_types(
            &mut cfg,
            Some(&vec!["jpeg".to_string(), "sqlite".to_string()]),
            false,
        );
        assert!(unknown.is_empty());
        let ids: Vec<&str> = cfg.file_types.iter().map(|ft| ft.id.as_str()).collect();
        assert_eq!(ids, vec!["jpeg", "sqlite"]);
    }

    #[test]
    fn disable_zip_removes_zip_validator() {
        let loaded = config::load_config(None).expect("config");
        let mut cfg = loaded.config;
        let _ = filter_file_types(&mut cfg, Some(&vec!["zip".to_string()]), true);
        assert!(cfg.file_types.iter().all(|ft| !ft.id.eq_ignore_ascii_case("zip")));
    }

    #[test]
    fn reports_unknown_types() {
        let loaded = config::load_config(None).expect("config");
        let mut cfg = loaded.config;
        let unknown = filter_file_types(
            &mut cfg,
            Some(&vec!["jpeg".to_string(), "nope".to_string()]),
            false,
        );
        assert_eq!(unknown, vec!["nope"]);
    }
}
