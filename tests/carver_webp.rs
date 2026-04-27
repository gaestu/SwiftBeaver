//! WebP carver tests against golden image.

mod common;

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use common::{get_expected_files, run_carver_for_types, verify_carved_files};
use serde_json::Value;
use swiftbeaver::config;
use swiftbeaver::evidence::RawFileSource;
use swiftbeaver::metadata::{self, MetadataBackendKind};
use swiftbeaver::pipeline::{self, PipelineStats};
use swiftbeaver::scanner;
use swiftbeaver::util;

struct WebpFixtureResult {
    _temp_dir: tempfile::TempDir,
    output_dir: PathBuf,
    stats: PipelineStats,
    records: Vec<Value>,
}

impl WebpFixtureResult {
    fn only_record(&self) -> &Value {
        assert_eq!(self.records.len(), 1, "expected one carved WebP record");
        &self.records[0]
    }

    fn carved_bytes(&self) -> Vec<u8> {
        let rel_path = self
            .only_record()
            .get("path")
            .and_then(Value::as_str)
            .expect("carved path");
        fs::read(self.output_dir.join("carved").join(rel_path)).expect("read carved file")
    }
}

fn make_webp(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
    let riff_size = 4 + chunks
        .iter()
        .map(|(_, payload)| 8 + payload.len() + (payload.len() % 2))
        .sum::<usize>();

    let mut webp = Vec::new();
    webp.extend_from_slice(b"RIFF");
    webp.extend_from_slice(&(riff_size as u32).to_le_bytes());
    webp.extend_from_slice(b"WEBP");

    for (fourcc, payload) in chunks {
        webp.extend_from_slice(*fourcc);
        webp.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        webp.extend_from_slice(payload);
        if payload.len() % 2 == 1 {
            webp.push(0);
        }
    }

    webp
}

fn run_webp_fixture(data: &[u8], max_size: u64) -> WebpFixtureResult {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let input_path = temp_dir.path().join("evidence.bin");
    fs::write(&input_path, data).expect("write evidence");

    let loaded = config::load_config(None).expect("config");
    let mut cfg = loaded.config;
    cfg.run_id = "webp_regression".to_string();
    cfg.file_types.retain(|file_type| file_type.id == "webp");
    for file_type in &mut cfg.file_types {
        file_type.min_size = 0;
        file_type.max_size = max_size;
    }

    let evidence = RawFileSource::open(&input_path).expect("open evidence");
    let evidence: Arc<dyn swiftbeaver::evidence::EvidenceSource> = Arc::new(evidence);

    let run_output_dir = temp_dir.path().join(&cfg.run_id);
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
        1,
        1,
        4096,
        16,
        None,
        None,
        carve_registry,
    )
    .expect("pipeline");

    let metadata_path = run_output_dir.join("metadata").join("carved_files.jsonl");
    let records = fs::read_to_string(&metadata_path)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).expect("metadata json"))
        .collect();

    WebpFixtureResult {
        _temp_dir: temp_dir,
        output_dir: run_output_dir,
        stats,
        records,
    }
}

fn assert_valid_exact_carve(data: Vec<u8>) {
    let result = run_webp_fixture(&data, 1024 * 1024);
    let record = result.only_record();

    assert_eq!(record["size"].as_u64(), Some(data.len() as u64));
    assert_eq!(record["validated"].as_bool(), Some(true));
    assert_eq!(record["truncated"].as_bool(), Some(false));
    assert_eq!(result.carved_bytes(), data);
}

#[test]
fn finds_all_webp_files() {
    skip_without_golden_image!();
    let manifest = load_manifest_or_skip!();

    let expected = get_expected_files(&manifest, &["webp"]);
    if expected.is_empty() {
        eprintln!("No WebP files in manifest");
        return;
    }

    let result = run_carver_for_types(&["webp"]);
    let (matched, errors) = verify_carved_files(&result, &expected, "WebP");

    assert!(
        errors.is_empty(),
        "WebP carver failed: {} errors, {} matched",
        errors.len(),
        matched
    );
    assert_eq!(
        matched,
        expected.len(),
        "WebP carver should find all {} files",
        expected.len()
    );
}

#[test]
fn carves_lossy_vp8_exact_size_from_riff_header() {
    let data = make_webp(&[(b"VP8 ", &[0x9d, 0x01, 0x2a, 0x00])]);
    assert_valid_exact_carve(data);
}

#[test]
fn carves_lossless_vp8l_exact_size_from_riff_header() {
    let data = make_webp(&[(b"VP8L", &[0x2f, 0x00, 0x00, 0x00, 0x00])]);
    assert_valid_exact_carve(data);
}

#[test]
fn carves_extended_animated_webp_with_inner_chunks() {
    let data = make_webp(&[
        (b"VP8X", &[0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        (b"ANIM", &[0xff, 0xff, 0xff, 0xff, 0, 0]),
        (b"ANMF", &[0; 16]),
    ]);
    assert_valid_exact_carve(data);
}

#[test]
fn preserves_bounded_unknown_non_primary_chunks() {
    let data = make_webp(&[(b"VP8 ", &[0x9d, 0x01, 0x2a, 0x00]), (b"abcd", &[1, 2, 3])]);
    assert_valid_exact_carve(data);
}

#[test]
fn exact_max_size_webp_is_not_marked_truncated() {
    let data = make_webp(&[(b"VP8 ", &[0x9d, 0x01, 0x2a, 0x00])]);
    let result = run_webp_fixture(&data, data.len() as u64);
    let record = result.only_record();

    assert_eq!(record["size"].as_u64(), Some(data.len() as u64));
    assert_eq!(record["validated"].as_bool(), Some(true));
    assert_eq!(record["truncated"].as_bool(), Some(false));
    assert!(
        record["errors"]
            .as_array()
            .expect("errors array")
            .is_empty()
    );
}

#[test]
fn rejects_oversize_riff_declaration() {
    let mut data = Vec::new();
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&0xffff_ffffu32.to_le_bytes());
    data.extend_from_slice(b"WEBP");
    data.extend_from_slice(b"VP8 ");
    data.extend_from_slice(&0u32.to_le_bytes());

    let result = run_webp_fixture(&data, 1024 * 1024);
    assert!(result.records.is_empty());
    assert_eq!(result.stats.files_carved, 0);
}

#[test]
fn rejects_missing_webp_marker() {
    let mut data = Vec::new();
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&12u32.to_le_bytes());
    data.extend_from_slice(b"WAVE");
    data.extend_from_slice(b"VP8 ");
    data.extend_from_slice(&0u32.to_le_bytes());

    let result = run_webp_fixture(&data, 1024 * 1024);
    assert!(result.records.is_empty());
    assert_eq!(result.stats.files_carved, 0);
}

#[test]
fn rejects_junk_first_chunk_fourcc() {
    let data = make_webp(&[(b"JUNK", &[])]);
    let result = run_webp_fixture(&data, 1024 * 1024);

    assert!(result.records.is_empty());
    assert_eq!(result.stats.files_carved, 0);
}

#[test]
fn truncated_evidence_writes_remaining_bytes_without_max_size_fallback() {
    let declared_total_size = 1024 * 1024u32;
    let mut data = Vec::new();
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&(declared_total_size - 8).to_le_bytes());
    data.extend_from_slice(b"WEBP");
    data.extend_from_slice(b"VP8 ");
    data.extend_from_slice(&(declared_total_size - 20).to_le_bytes());
    data.extend_from_slice(&[0x55; 128]);

    let result = run_webp_fixture(&data, 2 * 1024 * 1024);
    let record = result.only_record();

    assert_eq!(record["size"].as_u64(), Some(data.len() as u64));
    assert_eq!(record["validated"].as_bool(), Some(false));
    assert_eq!(record["truncated"].as_bool(), Some(true));
    assert!(
        record["errors"]
            .as_array()
            .expect("errors array")
            .iter()
            .any(|error| error.as_str() == Some("eof before WebP RIFF end"))
    );
    assert_ne!(record["size"].as_u64(), Some(2 * 1024 * 1024));
    assert_eq!(result.carved_bytes(), data);
}

#[test]
fn rejects_chunk_layout_that_exceeds_declared_riff_container() {
    let mut data = Vec::new();
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&56u32.to_le_bytes());
    data.extend_from_slice(b"WEBP");
    data.extend_from_slice(b"VP8 ");
    data.extend_from_slice(&1000u32.to_le_bytes());
    data.extend_from_slice(&[0x55; 8]);

    let result = run_webp_fixture(&data, 1024 * 1024);
    assert!(result.records.is_empty());
    assert_eq!(result.stats.files_carved, 0);
}
