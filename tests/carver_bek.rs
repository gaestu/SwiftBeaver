use std::sync::Arc;

use swiftbeaver::config;
use swiftbeaver::evidence::RawFileSource;
use swiftbeaver::metadata::{self, MetadataBackendKind};
use swiftbeaver::pipeline;
use swiftbeaver::scanner;
use swiftbeaver::util;

fn metadata_entry(entry_type: u16, value_type: u16, value: &[u8]) -> Vec<u8> {
    let size = 8u16 + u16::try_from(value.len()).expect("entry size");
    let mut entry = Vec::new();
    entry.extend_from_slice(&size.to_le_bytes());
    entry.extend_from_slice(&entry_type.to_le_bytes());
    entry.extend_from_slice(&value_type.to_le_bytes());
    entry.extend_from_slice(&1u16.to_le_bytes());
    entry.extend_from_slice(value);
    entry
}

fn key_property() -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&0u32.to_le_bytes());
    body.extend_from_slice(&[0x42u8; 32]);
    metadata_entry(0, 1, &body)
}

fn description_property(value: &str) -> Vec<u8> {
    let mut body = Vec::new();
    for unit in value.encode_utf16() {
        body.extend_from_slice(&unit.to_le_bytes());
    }
    body.extend_from_slice(&0u16.to_le_bytes());
    metadata_entry(0, 2, &body)
}

fn synthetic_bek(with_description: bool) -> Vec<u8> {
    let mut external = vec![0u8; 24];
    external[0..16].copy_from_slice(&[
        0x33, 0x22, 0x11, 0x00, 0x55, 0x44, 0x77, 0x66, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE,
        0xFF,
    ]);
    external[16..24].copy_from_slice(&116_444_736_000_000_000u64.to_le_bytes());
    if with_description {
        external.extend_from_slice(&description_property("ExternalKey"));
    }
    external.extend_from_slice(&key_property());

    let startup = metadata_entry(0x0006, 0x0009, &external);
    let metadata_size = 48u32 + u32::try_from(startup.len()).expect("metadata size");
    let mut header = vec![0u8; 48];
    header[0..4].copy_from_slice(&metadata_size.to_le_bytes());
    header[4..8].copy_from_slice(&1u32.to_le_bytes());
    header[8..12].copy_from_slice(&48u32.to_le_bytes());
    header[12..16].copy_from_slice(&metadata_size.to_le_bytes());
    [header, startup].concat()
}

fn malformed_bek_missing_key() -> Vec<u8> {
    let mut external = vec![0u8; 24];
    external.extend_from_slice(&description_property("ExternalKey"));
    let startup = metadata_entry(0x0006, 0x0009, &external);
    let metadata_size = 48u32 + u32::try_from(startup.len()).expect("metadata size");
    let mut header = vec![0u8; 48];
    header[0..4].copy_from_slice(&metadata_size.to_le_bytes());
    header[4..8].copy_from_slice(&1u32.to_le_bytes());
    header[8..12].copy_from_slice(&48u32.to_le_bytes());
    header[12..16].copy_from_slice(&metadata_size.to_le_bytes());
    [header, startup].concat()
}

fn run_bek_pipeline(
    image_bytes: &[u8],
    chunk_size: u64,
    overlap: u64,
) -> (
    tempfile::TempDir,
    Vec<serde_json::Value>,
    Vec<serde_json::Value>,
) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let raw_path = tmp.path().join("image.raw");
    std::fs::write(&raw_path, image_bytes).expect("write raw image");

    let loaded = config::load_config(None).expect("config");
    let mut cfg = loaded.config;
    cfg.run_id = "carver_test_bek".to_string();
    cfg.file_types.retain(|ft| ft.id == "bek");

    let evidence = RawFileSource::open(&raw_path).expect("open raw");
    let evidence: Arc<dyn swiftbeaver::evidence::EvidenceSource> = Arc::new(evidence);
    let run_output_dir = tmp.path().join(&cfg.run_id);
    std::fs::create_dir_all(&run_output_dir).expect("output dir");

    let meta_sink = metadata::build_sink(
        MetadataBackendKind::Jsonl,
        &cfg,
        &cfg.run_id,
        env!("CARGO_PKG_VERSION"),
        &loaded.config_hash,
        &raw_path,
        "evidence_hash",
        &run_output_dir,
    )
    .expect("metadata sink");

    let sig_scanner = scanner::build_signature_scanner(&cfg, false).expect("scanner");
    let sig_scanner: Arc<dyn swiftbeaver::scanner::SignatureScanner> = Arc::from(sig_scanner);
    let carve_registry = Arc::new(util::build_carve_registry(&cfg, false).expect("registry"));

    pipeline::run_pipeline(
        &cfg,
        evidence,
        sig_scanner,
        None,
        vec![meta_sink],
        &run_output_dir,
        1,
        1,
        chunk_size,
        overlap,
        None,
        None,
        carve_registry,
    )
    .expect("pipeline");

    let carved_path = run_output_dir.join("metadata").join("carved_files.jsonl");
    let bek_path = run_output_dir.join("metadata").join("bitlocker_bek.jsonl");
    let carved = read_jsonl(&carved_path);
    let bek = read_jsonl(&bek_path);
    (tmp, carved, bek)
}

fn read_jsonl(path: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .expect("read jsonl metadata")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("json row"))
        .collect()
}

fn embed_at(payload: &[u8], offset: usize) -> Vec<u8> {
    let mut image = vec![0xA5u8; offset];
    image.extend_from_slice(payload);
    image.extend_from_slice(&[0x5Au8; 64]);
    image
}

#[test]
fn carves_valid_bek_and_records_metadata() {
    let bek = synthetic_bek(true);
    let offset = 0x200usize;
    let image = embed_at(&bek, offset);

    let (tmp, carved, bek_rows) = run_bek_pipeline(&image, 4096, 128);
    assert_eq!(carved.len(), 1);
    assert_eq!(bek_rows.len(), 1);

    let row = &carved[0];
    assert_eq!(row["file_type"], "bek");
    assert_eq!(row["extension"], "bek");
    assert_eq!(row["global_start"], offset as u64);
    assert_eq!(row["global_end"], (offset + bek.len() - 1) as u64);
    assert_eq!(row["size"], bek.len() as u64);
    assert_eq!(row["validated"], true);
    assert_eq!(row["truncated"], false);
    assert_eq!(row["pattern_id"], "bek_v1_header_fields");
    assert_eq!(row["tool_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(row["evidence_sha256"], "evidence_hash");

    let rel_path = row["path"].as_str().expect("path");
    assert!(rel_path.starts_with("bek/"));
    assert!(!rel_path.contains(".."));
    let carved_file = tmp
        .path()
        .join("carver_test_bek")
        .join("carved")
        .join(rel_path);
    assert_eq!(std::fs::read(carved_file).expect("carved bytes"), bek);

    let bek_row = &bek_rows[0];
    assert_eq!(bek_row["global_start"], offset as u64);
    assert_eq!(bek_row["size"], bek.len() as u64);
    assert_eq!(
        bek_row["key_identifier_guid"],
        "00112233-4455-6677-8899-aabbccddeeff"
    );
    assert_eq!(bek_row["description"], "ExternalKey");
    assert_eq!(bek_row["key_data_length"], 32);
    assert_eq!(bek_row["key_encryption_method"], 0);
    assert_eq!(bek_row["evidence_sha256"], "evidence_hash");
}

#[test]
fn rejects_malformed_bek_without_key_property() {
    let malformed = malformed_bek_missing_key();
    let image = embed_at(&malformed, 0x100);
    let (_tmp, carved, bek_rows) = run_bek_pipeline(&image, 4096, 128);
    assert!(carved.is_empty());
    assert!(bek_rows.is_empty());
}

#[test]
fn detects_bek_across_chunk_boundary() {
    let bek = synthetic_bek(false);
    let offset = 60usize;
    let image = embed_at(&bek, offset);
    let (_tmp, carved, bek_rows) = run_bek_pipeline(&image, 64, 16);
    assert_eq!(carved.len(), 1);
    assert_eq!(bek_rows.len(), 1);
    assert_eq!(carved[0]["global_start"], offset as u64);
    assert!(bek_rows[0]["description"].is_null());
}
