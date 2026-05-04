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
    run_page_carver_with_workers(bytes, sqlite_page_max_hits_per_chunk, 1, 1).0
}

/// Returns `(records, overlap_skipped)` so tests can assert on the
/// pipeline's overlap-suppression counter directly.
fn run_page_carver_with_workers(
    bytes: Vec<u8>,
    sqlite_page_max_hits_per_chunk: Option<usize>,
    scan_workers: usize,
    carve_workers: usize,
) -> (Vec<Value>, u64) {
    run_page_carver_with_workers_and_limit(
        bytes,
        sqlite_page_max_hits_per_chunk,
        scan_workers,
        carve_workers,
        None,
    )
}

fn run_page_carver_with_workers_and_limit(
    bytes: Vec<u8>,
    sqlite_page_max_hits_per_chunk: Option<usize>,
    scan_workers: usize,
    carve_workers: usize,
    max_files: Option<u64>,
) -> (Vec<Value>, u64) {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let input_path = temp_dir.path().join("input.bin");
    fs::write(&input_path, bytes).expect("write input");

    let loaded = config::load_config(None).expect("config");
    let mut cfg = loaded.config;
    cfg.run_id = "sqlite_page_test".to_string();
    cfg.file_types.retain(|ft| ft.id == "sqlite_page");
    cfg.max_files = max_files;
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

    let stats = pipeline::run_pipeline(
        &cfg,
        evidence,
        sig_scanner,
        None,
        vec![meta_sink],
        &run_output_dir,
        scan_workers,
        carve_workers,
        64 * 1024,
        64,
        None,
        None,
        carve_registry,
    )
    .expect("pipeline");

    let meta_path = run_output_dir.join("metadata").join("carved_files.jsonl");
    let content = fs::read_to_string(meta_path).expect("metadata read");
    let records: Vec<Value> = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("json"))
        .collect();
    (records, stats.overlap_skipped)
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

/// Build two structurally-valid 4096-byte SQLite leaf pages whose ranges
/// overlap: page A occupies [4096..8192) and page B occupies [4608..8704),
/// so B starts inside A's content area and both validate independently.
///
/// This reproduces the failure mode in issue #84 where the single-byte
/// `0x0D` / `0x0A` page-type signatures combined with per-worker overlap
/// trackers caused both pages to be carved when processed by different
/// carve workers.
fn build_overlapping_leaf_pages() -> Vec<u8> {
    // Image holds:  [0..4096) filler | [4096..8192) page A | [8192..8704) page B tail.
    let mut image = vec![0xAAu8; 8704];
    // Zero out A's region first; then we'll write A and overlay B's header
    // bytes (which fall inside A's cell-content area).
    for byte in &mut image[4096..8704] {
        *byte = 0x00;
    }

    // --- Page A header at offset 4096 (relative offsets in [..]) ---
    image[4096] = 0x0D; // [0]   table leaf page type
    image[4097..4099].copy_from_slice(&0u16.to_be_bytes()); // [1..3] first_freeblock = 0
    image[4099..4101].copy_from_slice(&1u16.to_be_bytes()); // [3..5] cell_count = 1
    image[4101..4103].copy_from_slice(&512u16.to_be_bytes()); // [5..7] cell_content_area = 512
    image[4103] = 0x00; // [7]   fragmented_free_bytes = 0
    image[4104..4106].copy_from_slice(&512u16.to_be_bytes()); // [8..10] cell pointer = 512

    // --- Page B header at offset 4608 (relative offsets in [..]) ---
    // These bytes lie inside A's cell-content region (A relative 512..),
    // which the validator does not parse, so A still validates while B
    // independently passes the same checks.
    image[4608] = 0x0D; // [0]
    image[4609..4611].copy_from_slice(&0u16.to_be_bytes()); // [1..3] first_freeblock = 0
    image[4611..4613].copy_from_slice(&1u16.to_be_bytes()); // [3..5] cell_count = 1
    image[4613..4615].copy_from_slice(&4084u16.to_be_bytes()); // [5..7] cell_content_area = 4084
    image[4615] = 0x00; // [7]   fragmented_free_bytes = 0
    image[4616..4618].copy_from_slice(&4084u16.to_be_bytes()); // [8..10] cell pointer = 4084

    image
}

/// Regression test for issue #84.
///
/// Without cross-worker overlap suppression both overlapping candidates
/// are carved when handled by different workers. With the fix, the
/// streaming overlap arbiter processes sequenced hits in evidence order and
/// accepts the first non-overlapping range, so the lowest-start candidate
/// (4096) wins deterministically and `overlap_skipped` is incremented.
#[test]
fn overlapping_sqlite_pages_are_deconflicted_across_workers() {
    let image = build_overlapping_leaf_pages();

    let (records, overlap_skipped) = run_page_carver_with_workers(image, None, 1, 4);

    assert_eq!(
        records.len(),
        1,
        "expected exactly one sqlite_page record after overlap deconfliction, got {}: {:?}",
        records.len(),
        records
            .iter()
            .map(|r| r.get("global_start").and_then(|v| v.as_u64()))
            .collect::<Vec<_>>()
    );
    let winner = records[0]
        .get("global_start")
        .and_then(|v| v.as_u64())
        .expect("global_start present");
    assert_eq!(
        winner, 4096,
        "deterministic arbiter must pick the lowest-start candidate (4096), got {winner}"
    );
    assert!(
        overlap_skipped >= 1,
        "expected overlap_skipped >= 1 after suppressing nested candidate, got {overlap_skipped}"
    );
}

/// Recall regression for issue #84: two structurally-valid, *non-overlapping*
/// 4 KiB SQLite leaf pages placed back-to-back must both be carved even when
/// they are dispatched to different carve workers. This guards against the
/// pre-claim window being so wide that legitimate adjacent pages are
/// rejected before the first worker shrinks its claim in `finalize`.
#[test]
fn adjacent_sqlite_pages_are_both_carved_across_workers() {
    let page_a = build_valid_leaf_page(4096);
    let page_b = build_valid_leaf_page(4096);

    // Image: [0..4096) filler | [4096..8192) page A | [8192..12288) page B
    let mut image = vec![0xAAu8; 4096];
    image.extend_from_slice(&page_a);
    image.extend_from_slice(&page_b);

    let (records, _overlap_skipped) = run_page_carver_with_workers(image, None, 1, 4);

    let mut starts: Vec<u64> = records
        .iter()
        .filter_map(|r| r.get("global_start").and_then(|v| v.as_u64()))
        .collect();
    starts.sort_unstable();
    assert_eq!(
        starts,
        vec![4096, 8192],
        "both adjacent pages must be carved; got starts {starts:?}"
    );
}

/// Regression for interaction between overlap arbitration and `max_files`.
/// Overlap-rejected candidates must not consume the strict output budget.
#[test]
fn overlap_rejections_do_not_consume_max_files_budget() {
    let mut image = build_overlapping_leaf_pages();
    image.resize(12288, 0xAA);
    image.extend_from_slice(&build_valid_leaf_page(4096));

    let (records, overlap_skipped) =
        run_page_carver_with_workers_and_limit(image, None, 1, 4, Some(2));

    let mut starts: Vec<u64> = records
        .iter()
        .filter_map(|r| r.get("global_start").and_then(|v| v.as_u64()))
        .collect();
    starts.sort_unstable();
    assert_eq!(
        starts,
        vec![4096, 12288],
        "max_files budget must be spent only on accepted non-overlapping carves; got starts {starts:?}"
    );
    assert!(
        overlap_skipped >= 1,
        "expected at least one overlap rejection, got {overlap_skipped}"
    );
}
