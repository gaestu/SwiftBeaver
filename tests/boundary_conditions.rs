use std::fs;
use std::sync::Arc;

use serde_json::Value;

use swiftbeaver::config;
use swiftbeaver::evidence::RawFileSource;
use swiftbeaver::metadata::{self, MetadataBackendKind};
use swiftbeaver::pipeline;
use swiftbeaver::scanner;
use swiftbeaver::util;

fn insert_bytes(target: &mut Vec<u8>, offset: usize, data: &[u8]) {
    let end = offset + data.len();
    if end > target.len() {
        target.resize(end, 0u8);
    }
    target[offset..end].copy_from_slice(data);
}

fn run_pipeline_with_bytes(
    bytes: Vec<u8>,
    chunk_size: u64,
    overlap: u64,
    max_files: Option<u64>,
) -> (pipeline::PipelineStats, Vec<Value>) {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let input_path = temp_dir.path().join("input.bin");
    fs::write(&input_path, bytes).expect("write input");

    let loaded = config::load_config(None).expect("config");
    let mut cfg = loaded.config;
    cfg.run_id = "boundary_test".to_string();
    cfg.max_files = max_files;

    // Override min_size for image formats to allow smaller test files
    for ft in cfg.file_types.iter_mut() {
        if ft.id == "gif" || ft.id == "jpeg" || ft.id == "png" || ft.id == "bmp" {
            ft.min_size = 16;
        }
    }

    let evidence = RawFileSource::open(&input_path).expect("evidence");
    let evidence: Arc<dyn swiftbeaver::evidence::EvidenceSource> = Arc::new(evidence);

    let run_output_dir = temp_dir.path().join("run");
    fs::create_dir_all(&run_output_dir).expect("output dir");

    let meta_sink = metadata::build_sink(
        MetadataBackendKind::Jsonl,
        &cfg,
        &cfg.run_id,
        env!("CARGO_PKG_VERSION"),
        &loaded.config_hash,
        &input_path,
        "",
        &run_output_dir,
    )
    .expect("metadata sink");

    let sig_scanner = scanner::build_signature_scanner(&cfg, false).expect("scanner");
    let sig_scanner: Arc<dyn swiftbeaver::scanner::SignatureScanner> = Arc::from(sig_scanner);

    let carve_registry = Arc::new(util::build_carve_registry(&cfg, false).expect("registry"));

    let stats = pipeline::run_pipeline(
        &cfg,
        evidence,
        sig_scanner,
        None,
        meta_sink,
        &run_output_dir,
        1,
        chunk_size,
        overlap,
        None,
        None,
        carve_registry,
    )
    .expect("pipeline");

    let meta_path = run_output_dir.join("metadata").join("carved_files.jsonl");
    let content = fs::read_to_string(meta_path).expect("metadata read");
    let records = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("json"))
        .collect();

    (stats, records)
}

#[test]
fn file_spans_chunk_boundary() {
    let mut data = vec![0u8; 80];
    let mut jpeg = vec![0u8; 20];
    jpeg[0..4].copy_from_slice(&[0xFF, 0xD8, 0xFF, 0xE0]);
    jpeg[4..9].copy_from_slice(b"JFIF\0");
    let end = jpeg.len();
    jpeg[end - 2..end].copy_from_slice(&[0xFF, 0xD9]);
    insert_bytes(&mut data, 28, &jpeg);

    let (_stats, records) = run_pipeline_with_bytes(data, 32, 8, None);
    let jpeg_rec = records
        .iter()
        .find(|r| r.get("file_type").and_then(|v| v.as_str()) == Some("jpeg"))
        .expect("jpeg record");
    assert_eq!(
        jpeg_rec.get("validated").and_then(|v| v.as_bool()),
        Some(true)
    );
}

#[test]
fn file_at_exact_chunk_size() {
    let mut data = vec![0u8; 32];
    let mut jpeg = vec![0u8; 32];
    jpeg[0..4].copy_from_slice(&[0xFF, 0xD8, 0xFF, 0xE0]);
    jpeg[4..9].copy_from_slice(b"JFIF\0");
    jpeg[30..32].copy_from_slice(&[0xFF, 0xD9]);
    insert_bytes(&mut data, 0, &jpeg);

    let (_stats, records) = run_pipeline_with_bytes(data, 32, 0, None);
    let jpeg_rec = records
        .iter()
        .find(|r| r.get("file_type").and_then(|v| v.as_str()) == Some("jpeg"))
        .expect("jpeg record");
    assert_eq!(jpeg_rec.get("size").and_then(|v| v.as_u64()), Some(32));
}

#[test]
fn empty_evidence_produces_no_hits() {
    let (stats, records) = run_pipeline_with_bytes(Vec::new(), 64, 0, None);
    assert_eq!(stats.hits_found, 0);
    assert_eq!(stats.files_carved, 0);
    assert!(records.is_empty());
}

#[test]
fn max_files_stops_after_limit() {
    let mut data = vec![0u8; 128];
    let mut jpeg = vec![0u8; 32];
    jpeg[0..4].copy_from_slice(&[0xFF, 0xD8, 0xFF, 0xE0]);
    jpeg[4..9].copy_from_slice(b"JFIF\0");
    jpeg[30..32].copy_from_slice(&[0xFF, 0xD9]);
    insert_bytes(&mut data, 0, &jpeg);
    insert_bytes(&mut data, 64, &jpeg);

    let (stats, records) = run_pipeline_with_bytes(data, 32, 0, Some(1));
    assert_eq!(stats.files_carved, 1);
    let count = records
        .iter()
        .filter(|r| r.get("file_type").and_then(|v| v.as_str()) == Some("jpeg"))
        .count();
    assert_eq!(count, 1);
}
