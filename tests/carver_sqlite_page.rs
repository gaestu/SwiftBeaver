mod common;

use std::fs;
use std::sync::Arc;

use serde_json::Value;

use swiftbeaver::config;
use swiftbeaver::evidence::RawFileSource;
use swiftbeaver::metadata::{self, MetadataBackendKind};
use swiftbeaver::pipeline;
use swiftbeaver::scanner;
use swiftbeaver::util;

fn build_valid_leaf_page(page_size: usize) -> Vec<u8> {
    let mut page = vec![0u8; page_size];
    page[0] = 0x0D; // table leaf page
    page[1..3].copy_from_slice(&0u16.to_be_bytes()); // first freeblock
    page[3..5].copy_from_slice(&1u16.to_be_bytes()); // cell count
    let cell_start = (page_size - 16) as u16;
    page[5..7].copy_from_slice(&cell_start.to_be_bytes()); // cell content area
    page[7] = 0; // fragmented free bytes
    page[8..10].copy_from_slice(&cell_start.to_be_bytes()); // one cell pointer
    page[cell_start as usize] = 0x01;
    page
}

fn run_page_carver(bytes: Vec<u8>, sqlite_page_max_hits_per_chunk: Option<usize>) -> Vec<Value> {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let input_path = temp_dir.path().join("input.bin");
    fs::write(&input_path, bytes).expect("write input");

    let loaded = config::load_config(None).expect("config");
    let mut cfg = loaded.config;
    cfg.run_id = "sqlite_page_test".to_string();
    cfg.file_types.retain(|ft| ft.id == "sqlite_page");
    if let Some(cap) = sqlite_page_max_hits_per_chunk {
        cfg.sqlite_page_max_hits_per_chunk = cap;
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

    pipeline::run_pipeline(
        &cfg,
        evidence,
        sig_scanner,
        None,
        meta_sink,
        &run_output_dir,
        1,
        1,
        64 * 1024,
        64,
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
fn carves_valid_sqlite_page() {
    let mut image = vec![0xAA; 16_384];
    let page = build_valid_leaf_page(4096);
    let offset = 4096usize;
    image[offset..offset + page.len()].copy_from_slice(&page);

    let records = run_page_carver(image, None);
    assert_eq!(records.len(), 1, "expected exactly one sqlite_page record");

    let rec = &records[0];
    assert_eq!(
        rec.get("file_type").and_then(|v| v.as_str()),
        Some("sqlite_page")
    );
    assert_eq!(
        rec.get("global_start").and_then(|v| v.as_u64()),
        Some(offset as u64)
    );
    assert_eq!(rec.get("size").and_then(|v| v.as_u64()), Some(4096));
}

#[test]
fn rejects_noisy_candidates() {
    let mut image = vec![0u8; 8192];
    for i in (0..image.len()).step_by(127) {
        image[i] = 0x0D; // signature byte but invalid structure (cell_count remains zero)
    }

    let records = run_page_carver(image, None);
    assert!(
        records.is_empty(),
        "expected no sqlite_page records from noisy data"
    );
}

#[test]
fn caps_sqlite_page_hits_per_chunk() {
    let mut image = vec![0xAA; 64 * 1024];
    for i in 0..8usize {
        let offset = 1024 + i * 4096;
        let page = build_valid_leaf_page(4096);
        image[offset..offset + 4096].copy_from_slice(&page);
    }

    let records = run_page_carver(image, Some(2));
    assert!(
        records.len() <= 2,
        "expected sqlite_page hit cap to keep at most 2 records, got {}",
        records.len()
    );
}

#[test]
fn finds_sqlite_orphan_page_from_golden_image() {
    skip_without_golden_image!();
    let manifest = load_manifest_or_skip!();
    let expected: Vec<common::ManifestFile> = manifest
        .files
        .iter()
        .filter(|f| f.path == "databases/sqlite_orphan_page.bin")
        .cloned()
        .collect();
    if expected.is_empty() {
        eprintln!("No sqlite_orphan_page.bin in manifest");
        return;
    }

    let result = common::run_carver_for_types(&["sqlite_page"]);
    let (matched, errors) = common::verify_carved_files(&result, &expected, "SQLite Page");

    assert!(
        errors.is_empty(),
        "SQLite page carver failed: {} errors, {} matched",
        errors.len(),
        matched
    );
    assert_eq!(
        matched,
        expected.len(),
        "SQLite page carver should find all {} expected page fixtures",
        expected.len()
    );
}
