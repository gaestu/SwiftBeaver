//! Integration tests for the Windows Registry hive carver.
//!
//! Covers the acceptance criteria from issue #43:
//!  - basic carving of a valid `regf` hive
//!  - hive-type detection from the embedded filename
//!  - embedded filename extraction
//!  - graceful handling of truncated input
//!  - max_size enforcement

use std::sync::Arc;

use swiftbeaver::config;
use swiftbeaver::evidence::RawFileSource;
use swiftbeaver::metadata::{self, MetadataBackendKind};
use swiftbeaver::pipeline;
use swiftbeaver::scanner;
use swiftbeaver::util;

#[path = "common/registry_fixture.rs"]
mod registry_fixture;

use registry_fixture::{
    build_registry_hive, build_registry_hive_with_seq, build_registry_log_hive_with_seq,
};

fn run_registry_pipeline(
    image_bytes: &[u8],
    max_size_override: Option<u64>,
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
    cfg.run_id = "carver_test_registry".to_string();
    cfg.file_types.retain(|ft| ft.id == "registry");
    if let Some(max) = max_size_override
        && let Some(ft) = cfg.file_types.iter_mut().find(|ft| ft.id == "registry")
    {
        ft.max_size = max;
    }

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
        vec![meta_sink],
        &run_output_dir,
        1,
        1,
        4096,
        512,
        None,
        None,
        carve_registry,
    )
    .expect("pipeline");

    let carved_path = run_output_dir.join("metadata").join("carved_files.jsonl");
    let windows_path = run_output_dir
        .join("metadata")
        .join("windows_artefacts.jsonl");
    let carved = std::fs::read_to_string(&carved_path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("carved json"))
        .collect();
    let windows = std::fs::read_to_string(&windows_path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("windows json"))
        .collect();

    (tmp, carved, windows)
}

fn embed_at_offset(payload: &[u8], offset: usize) -> Vec<u8> {
    let mut image = vec![0u8; offset];
    image.extend_from_slice(payload);
    image.extend_from_slice(&[0xAAu8; 64]);
    image
}

// ----------------------------------------------------------------
// test_registry_basic_carve
// ----------------------------------------------------------------
#[test]
fn test_registry_basic_carve() {
    let hive = build_registry_hive("SYSTEM", "ROOT");
    let offset = 0x1000usize;
    let image = embed_at_offset(&hive, offset);

    let (_tmp, carved, windows) = run_registry_pipeline(&image, None);
    assert_eq!(carved.len(), 1, "expected exactly one carved registry hive");
    assert_eq!(windows.len(), 1);

    let row = &carved[0];
    assert_eq!(row["file_type"], "registry");
    assert_eq!(row["extension"], "hve");
    assert_eq!(row["global_start"], offset as u64);
    assert_eq!(row["size"], hive.len() as u64);
    assert_eq!(row["validated"], true);
    assert_eq!(row["truncated"], false);

    let win = &windows[0];
    assert_eq!(win["artefact_type"], "registry");
    assert_eq!(win["offset"], offset as u64);
    assert_eq!(win["size"], hive.len() as u64);
    assert_eq!(win["root_key_name"], "ROOT");
}

// ----------------------------------------------------------------
// test_registry_hive_type_detection
// ----------------------------------------------------------------
#[test]
fn test_registry_hive_type_detection() {
    for (embedded, expected) in [
        ("SAM", "SAM"),
        ("SYSTEM", "SYSTEM"),
        ("SOFTWARE", "SOFTWARE"),
        ("NTUSER.DAT", "NTUSER.DAT"),
        (r"\config\SECURITY", "SECURITY"),
        (r"Users\bob\ntuser.dat", "NTUSER.DAT"),
    ] {
        let hive = build_registry_hive(embedded, "ROOT");
        let image = embed_at_offset(&hive, 0x1000);
        let (_tmp, _carved, windows) = run_registry_pipeline(&image, None);
        assert_eq!(windows.len(), 1, "hive {embedded} not carved");
        assert_eq!(
            windows[0]["hive_type"], expected,
            "hive_type mismatch for {embedded}"
        );
    }
}

// ----------------------------------------------------------------
// test_registry_embedded_filename
// ----------------------------------------------------------------
#[test]
fn test_registry_embedded_filename() {
    let hive = build_registry_hive("NTUSER.DAT", "ROOT");
    let image = embed_at_offset(&hive, 0x800);
    let (_tmp, _carved, windows) = run_registry_pipeline(&image, None);
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0]["hive_name"], "NTUSER.DAT");
    assert_eq!(windows[0]["hive_type"], "NTUSER.DAT");
}

// ----------------------------------------------------------------
// test_registry_truncated
// ----------------------------------------------------------------
#[test]
fn test_registry_truncated() {
    let mut hive = build_registry_hive("SYSTEM", "ROOT");
    // Drop the last 1 KiB of the hive bin so the carver cannot read the full
    // declared size; the hit must not panic and must either skip or mark
    // the carved artefact as truncated.
    hive.truncate(hive.len() - 1024);
    let offset = 0x400usize;
    // Do NOT pad with sentinel bytes — we want evidence to actually end
    // before the declared hive_bins_data_size so the carver hits Eof.
    let mut image = vec![0u8; offset];
    image.extend_from_slice(&hive);

    let (_tmp, carved, _windows) = run_registry_pipeline(&image, None);
    if let Some(row) = carved.first() {
        // If anything was emitted it must be marked truncated and not larger
        // than what is actually in evidence.
        assert_eq!(row["truncated"], true);
        let size = row["size"].as_u64().expect("size");
        assert!(size <= hive.len() as u64);
    }
}

// ----------------------------------------------------------------
// test_registry_size_limit
// ----------------------------------------------------------------
const BASE_BLOCK_ONLY_LIMIT: usize = 4096;

#[test]
fn test_registry_size_limit() {
    let hive = build_registry_hive("SYSTEM", "ROOT");
    let image = embed_at_offset(&hive, 0x1000);

    // Set max_size below the hive total size — it must be rejected entirely.
    let (_tmp, carved, windows) = run_registry_pipeline(&image, Some(BASE_BLOCK_ONLY_LIMIT as u64));
    assert!(
        carved.is_empty(),
        "registry above max_size must not be carved"
    );
    assert!(windows.is_empty());
}

// ----------------------------------------------------------------
// test_registry_dirty_hive_carved
// ----------------------------------------------------------------
#[test]
fn test_registry_dirty_hive_carved() {
    // A "dirty" hive has primary != secondary sequence. The carver must
    // still emit the bytes — recovery is the analyst's call.
    let hive = build_registry_hive_with_seq("SYSTEM", "ROOT", 7, 6);
    let image = embed_at_offset(&hive, 0x1000);
    let (_tmp, carved, windows) = run_registry_pipeline(&image, None);
    assert_eq!(carved.len(), 1, "dirty hive must still be carved");
    assert_eq!(windows.len(), 1);
}

// ----------------------------------------------------------------
// test_registry_log_with_zero_secondary_sequence
// ----------------------------------------------------------------
#[test]
fn test_registry_log_with_zero_secondary_sequence() {
    let hive = build_registry_log_hive_with_seq("SYSTEM.LOG1", "ROOT", 1, 0);
    let image = embed_at_offset(&hive, 0x1000);
    let (_tmp, carved, windows) = run_registry_pipeline(&image, None);
    assert_eq!(carved.len(), 1, "fresh log hive must still be carved");
    assert_eq!(windows.len(), 1);
}

// ----------------------------------------------------------------
// test_registry_primary_with_zero_secondary_sequence_rejected
// ----------------------------------------------------------------
#[test]
fn test_registry_primary_with_zero_secondary_sequence_rejected() {
    let hive = build_registry_hive_with_seq("SYSTEM", "ROOT", 1, 0);
    let image = embed_at_offset(&hive, 0x1000);
    let (_tmp, carved, windows) = run_registry_pipeline(&image, None);
    assert!(
        carved.is_empty(),
        "primary hive with zero secondary sequence must be rejected"
    );
    assert!(windows.is_empty());
}
