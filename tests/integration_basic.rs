use std::fs;
use std::sync::Arc;

use fastcarve::config;
use fastcarve::evidence::RawFileSource;
use fastcarve::metadata::{self, MetadataBackendKind};
use fastcarve::scanner;
use fastcarve::util;

fn insert_bytes(target: &mut Vec<u8>, offset: usize, data: &[u8]) {
    let end = offset + data.len();
    if end > target.len() {
        target.resize(end, 0u8);
    }
    target[offset..end].copy_from_slice(data);
}

fn sample_jpeg() -> Vec<u8> {
    let mut data = vec![0u8; 32];
    data[0..4].copy_from_slice(&[0xFF, 0xD8, 0xFF, 0xE0]);
    data[4..9].copy_from_slice(b"JFIF\0");
    data[30..32].copy_from_slice(&[0xFF, 0xD9]);
    data
}

fn sample_png() -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    data.extend_from_slice(&[0x00, 0x00, 0x00, 0x0D]);
    data.extend_from_slice(b"IHDR");
    data.extend_from_slice(&[
        0x00, 0x00, 0x00, 0x01,
        0x00, 0x00, 0x00, 0x01,
        0x08, 0x02, 0x00, 0x00, 0x00,
    ]);
    data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    data.extend_from_slice(b"IEND");
    data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    data
}

fn sample_gif() -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(b"GIF89a");
    data.extend_from_slice(&[0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00]);
    data.extend_from_slice(&[0x21, 0xF9, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00]);
    data.push(0x3B);
    data
}

fn sample_sqlite() -> Vec<u8> {
    let mut data = vec![0u8; 1024];
    data[0..16].copy_from_slice(b"SQLite format 3\0");
    data[16..18].copy_from_slice(&[0x04, 0x00]); // page size 1024
    data[28..32].copy_from_slice(&[0x00, 0x00, 0x00, 0x01]); // page count 1
    data
}

fn sample_pdf() -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(b"%PDF-1.4\n");
    data.extend_from_slice(b"1 0 obj\n<<>>\nendobj\n");
    while data.len() < 64 {
        data.push(b' ');
    }
    data.extend_from_slice(b"%%EOF");
    data
}

fn sample_docx_zip() -> Vec<u8> {
    let name = b"word/document.xml";
    let name_len = name.len() as u16;
    let mut out = Vec::new();

    out.extend_from_slice(b"PK\x03\x04");
    out.extend_from_slice(&[0x14, 0x00]);
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    out.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]);
    out.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]);
    out.extend_from_slice(&name_len.to_le_bytes());
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(name);
    out.extend_from_slice(b"x");

    out.extend_from_slice(b"PK\x01\x02");
    out.extend_from_slice(&[0x14, 0x00]);
    out.extend_from_slice(&[0x14, 0x00]);
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    out.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]);
    out.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]);
    out.extend_from_slice(&name_len.to_le_bytes());
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    out.extend_from_slice(name);

    let cd_size = 46 + name.len();
    let cd_offset = 30 + name.len() + 1;
    out.extend_from_slice(b"PK\x05\x06");
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(&[0x01, 0x00]);
    out.extend_from_slice(&[0x01, 0x00]);
    out.extend_from_slice(&(cd_size as u32).to_le_bytes());
    out.extend_from_slice(&(cd_offset as u32).to_le_bytes());
    out.extend_from_slice(&[0x00, 0x00]);

    out
}

fn sample_webp() -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&12u32.to_le_bytes());
    data.extend_from_slice(b"WEBP");
    data.extend_from_slice(b"VP8 ");
    data.extend_from_slice(&0u32.to_le_bytes());
    data
}

#[test]
fn integration_carves_basic_formats() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let input_path = temp_dir.path().join("image.bin");

    let mut image = vec![0u8; 300_000];
    insert_bytes(&mut image, 1024, &sample_jpeg());
    insert_bytes(&mut image, 65_536, &sample_png());
    insert_bytes(&mut image, 131_072, &sample_gif());
    insert_bytes(&mut image, 150_000, &sample_sqlite());
    insert_bytes(&mut image, 200_000, &sample_pdf());
    insert_bytes(&mut image, 220_000, &sample_docx_zip());
    insert_bytes(&mut image, 260_000, &sample_webp());

    fs::write(&input_path, &image).expect("write input");

    let loaded = config::load_config(None).expect("config");
    let mut cfg = loaded.config;
    cfg.run_id = "test_run".to_string();

    let evidence = RawFileSource::open(&input_path).expect("evidence");
    let evidence: Arc<dyn fastcarve::evidence::EvidenceSource> = Arc::new(evidence);

    let run_output_dir = temp_dir.path().join("run");
    fs::create_dir_all(&run_output_dir).expect("output dir");

    let meta_sink = metadata::build_sink(
        MetadataBackendKind::Jsonl,
        &cfg,
        &cfg.run_id,
        "0.1.0",
        &loaded.config_hash,
        &input_path,
        "",
        &run_output_dir,
    )
    .expect("metadata sink");

    let sig_scanner = scanner::build_signature_scanner(&cfg, false).expect("scanner");
    let sig_scanner: Arc<dyn fastcarve::scanner::SignatureScanner> = Arc::from(sig_scanner);

    util::run_pipeline(
        &cfg,
        evidence,
        sig_scanner,
        None,
        meta_sink,
        &run_output_dir,
        2,
        64 * 1024,
        64,
    )
    .expect("pipeline");

    let carved_root = run_output_dir.join("carved");
    assert!(carved_root.join("jpeg").exists());
    assert!(carved_root.join("png").exists());
    assert!(carved_root.join("gif").exists());
    assert!(carved_root.join("sqlite").exists());
    assert!(carved_root.join("pdf").exists());
    assert!(carved_root.join("docx").exists());
    assert!(carved_root.join("webp").exists());

    let meta_path = run_output_dir.join("metadata").join("carved_files.jsonl");
    let contents = fs::read_to_string(meta_path).expect("metadata read");
    let lines: Vec<&str> = contents.lines().collect();
    assert!(lines.len() >= 3, "expected at least 3 records");

    let mut types = Vec::new();
    for line in lines {
        let v: serde_json::Value = serde_json::from_str(line).expect("json");
        if let Some(t) = v.get("file_type").and_then(|v| v.as_str()) {
            types.push(t.to_string());
        }
    }

    assert!(types.contains(&"jpeg".to_string()));
    assert!(types.contains(&"png".to_string()));
    assert!(types.contains(&"gif".to_string()));
    assert!(types.contains(&"sqlite".to_string()));
    assert!(types.contains(&"pdf".to_string()));
    assert!(types.contains(&"docx".to_string()));
    assert!(types.contains(&"webp".to_string()));
}
