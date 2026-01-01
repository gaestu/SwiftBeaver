use std::fs;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde_json::Value;

use fastcarve::checkpoint;
use fastcarve::config;
use fastcarve::evidence::RawFileSource;
use fastcarve::metadata::{self, MetadataBackendKind};
use fastcarve::pipeline;
use fastcarve::scanner;
use fastcarve::util;

fn insert_bytes(target: &mut Vec<u8>, offset: usize, data: &[u8]) {
    let end = offset + data.len();
    if end > target.len() {
        target.resize(end, 0u8);
    }
    target[offset..end].copy_from_slice(data);
}

fn minimal_jpeg() -> Vec<u8> {
    let mut jpeg = vec![0u8; 32];
    jpeg[0..4].copy_from_slice(&[0xFF, 0xD8, 0xFF, 0xE0]);
    jpeg[4..9].copy_from_slice(b"JFIF\0");
    jpeg[30..32].copy_from_slice(&[0xFF, 0xD9]);
    jpeg
}

fn read_carved_records(run_output_dir: &std::path::Path) -> Vec<Value> {
    let meta_path = run_output_dir.join("metadata").join("carved_files.jsonl");
    let content = fs::read_to_string(meta_path).expect("metadata read");
    content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("json"))
        .collect()
}

#[test]
fn resume_from_checkpoint_skips_scanned_chunks() {
    let mut data = vec![0u8; 160];
    let jpeg = minimal_jpeg();
    insert_bytes(&mut data, 0, &jpeg);
    insert_bytes(&mut data, 96, &jpeg);

    let temp_dir = tempfile::tempdir().expect("tempdir");
    let input_path = temp_dir.path().join("input.bin");
    fs::write(&input_path, data).expect("write input");

    let loaded = config::load_config(None).expect("config");
    let mut cfg = loaded.config;
    cfg.run_id = "checkpoint_test".to_string();

    let checkpoint_path = temp_dir.path().join("checkpoint.json");

    let evidence = RawFileSource::open(&input_path).expect("evidence");
    let evidence: Arc<dyn fastcarve::evidence::EvidenceSource> = Arc::new(evidence);
    let run_output_dir = temp_dir.path().join("run1");
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
    let sig_scanner: Arc<dyn fastcarve::scanner::SignatureScanner> = Arc::from(sig_scanner);
    let carve_registry = Arc::new(util::build_carve_registry(&cfg).expect("registry"));

    let checkpoint_cfg = Some(pipeline::CheckpointConfig {
        path: checkpoint_path.clone(),
        resume: None,
    });

    let cancel_flag = Arc::new(AtomicBool::new(false));
    pipeline::run_pipeline_with_cancel(
        &cfg,
        evidence,
        sig_scanner,
        None,
        meta_sink,
        &run_output_dir,
        1,
        64,
        0,
        None,
        Some(1),
        carve_registry,
        cancel_flag,
        None,
        checkpoint_cfg,
    )
    .expect("pipeline");

    assert!(checkpoint_path.exists());

    let resume_state = checkpoint::load_checkpoint(&checkpoint_path).expect("load checkpoint");

    let evidence = RawFileSource::open(&input_path).expect("evidence");
    let evidence: Arc<dyn fastcarve::evidence::EvidenceSource> = Arc::new(evidence);
    let run_output_dir = temp_dir.path().join("run2");
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
    let sig_scanner: Arc<dyn fastcarve::scanner::SignatureScanner> = Arc::from(sig_scanner);
    let carve_registry = Arc::new(util::build_carve_registry(&cfg).expect("registry"));

    let checkpoint_cfg = Some(pipeline::CheckpointConfig {
        path: checkpoint_path,
        resume: Some(resume_state),
    });

    let cancel_flag = Arc::new(AtomicBool::new(false));
    pipeline::run_pipeline_with_cancel(
        &cfg,
        evidence,
        sig_scanner,
        None,
        meta_sink,
        &run_output_dir,
        1,
        64,
        0,
        None,
        None,
        carve_registry,
        cancel_flag,
        None,
        checkpoint_cfg,
    )
    .expect("pipeline");

    let records = read_carved_records(&run_output_dir);
    assert_eq!(records.len(), 1, "expected one carved file after resume");
    let start = records[0]
        .get("global_start")
        .and_then(|v| v.as_u64())
        .expect("global_start");
    assert!(start >= 64, "expected carved file from resumed chunk");
}
