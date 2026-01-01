use std::fs;
use std::sync::Arc;

use serde_json::Value;

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

fn run_pipeline_with_bytes(bytes: Vec<u8>) -> Vec<Value> {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let input_path = temp_dir.path().join("input.bin");
    fs::write(&input_path, bytes).expect("write input");

    let loaded = config::load_config(None).expect("config");
    let mut cfg = loaded.config;
    cfg.run_id = "malformed_test".to_string();

    let evidence = RawFileSource::open(&input_path).expect("evidence");
    let evidence: Arc<dyn fastcarve::evidence::EvidenceSource> = Arc::new(evidence);

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
    let sig_scanner: Arc<dyn fastcarve::scanner::SignatureScanner> = Arc::from(sig_scanner);

    let carve_registry = Arc::new(util::build_carve_registry(&cfg).expect("registry"));

    pipeline::run_pipeline(
        &cfg,
        evidence,
        sig_scanner,
        None,
        meta_sink,
        &run_output_dir,
        1,
        64,
        8,
        None,
        None,
        carve_registry,
    )
    .expect("pipeline");

    let meta_path = run_output_dir.join("metadata").join("carved_files.jsonl");
    let content = fs::read_to_string(meta_path).expect("metadata read");
    content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("json"))
        .collect()
}

#[test]
fn malformed_inputs_are_handled() {
    let mut data = vec![0u8; 4096];

    let mut jpeg = vec![0u8; 32];
    jpeg[0..4].copy_from_slice(&[0xFF, 0xD8, 0xFF, 0xE0]);
    jpeg[4..9].copy_from_slice(b"JFIF\0");
    insert_bytes(&mut data, 0, &jpeg);

    let mut png = Vec::new();
    png.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    png.extend_from_slice(&0x00001000u32.to_be_bytes());
    png.extend_from_slice(b"IHDR");
    insert_bytes(&mut data, 512, &png);

    let mut gif = Vec::new();
    gif.extend_from_slice(b"GIF89a");
    gif.extend_from_slice(&[0x01, 0x00]);
    insert_bytes(&mut data, 1024, &gif);

    let mut sqlite = vec![0u8; 100];
    sqlite[0..16].copy_from_slice(b"SQLite format 3\0");
    sqlite[16] = 0x03;
    sqlite[17] = 0xE8; // 1000, invalid
    insert_bytes(&mut data, 1536, &sqlite);

    let zip = vec![0x50, 0x4B, 0x03, 0x04, 0x00, 0x00, 0x00, 0x00];
    insert_bytes(&mut data, 2048, &zip);

    let records = run_pipeline_with_bytes(data);

    let mut types = Vec::new();
    for record in &records {
        if let Some(t) = record.get("file_type").and_then(|v| v.as_str()) {
            types.push(t.to_string());
        }
    }

    assert!(types.contains(&"jpeg".to_string()));
    assert!(types.contains(&"png".to_string()));
    assert!(!types.contains(&"gif".to_string()));
    assert!(!types.contains(&"sqlite".to_string()));
    assert!(!types.contains(&"zip".to_string()));

    let jpeg_rec = records
        .iter()
        .find(|r| r.get("file_type").and_then(|v| v.as_str()) == Some("jpeg"))
        .expect("jpeg record");
    assert_eq!(jpeg_rec.get("validated").and_then(|v| v.as_bool()), Some(false));
    assert_eq!(jpeg_rec.get("truncated").and_then(|v| v.as_bool()), Some(true));

    let png_rec = records
        .iter()
        .find(|r| r.get("file_type").and_then(|v| v.as_str()) == Some("png"))
        .expect("png record");
    assert_eq!(png_rec.get("validated").and_then(|v| v.as_bool()), Some(false));
    assert_eq!(png_rec.get("truncated").and_then(|v| v.as_bool()), Some(true));
}
