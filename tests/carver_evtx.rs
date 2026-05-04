use std::sync::Arc;

use swiftbeaver::config;
use swiftbeaver::evidence::RawFileSource;
use swiftbeaver::metadata::{self, MetadataBackendKind};
use swiftbeaver::pipeline;
use swiftbeaver::scanner;
use swiftbeaver::util;

#[path = "common/evtx_fixture.rs"]
mod evtx_fixture;

use evtx_fixture::{EvtxBuildSpec, build_evtx};

const HEADER_SIZE: u64 = 4096;
const CHUNK_SIZE: u64 = 65536;

fn run_evtx_pipeline(
    image_bytes: &[u8],
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
    cfg.run_id = "carver_test_evtx".to_string();
    cfg.file_types.retain(|ft| ft.id == "evtx");

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

    let carved_meta_path = run_output_dir.join("metadata").join("carved_files.jsonl");
    let windows_meta_path = run_output_dir
        .join("metadata")
        .join("windows_artefacts.jsonl");
    let carved_rows = std::fs::read_to_string(&carved_meta_path)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("parse carved json"))
        .collect();
    let windows_rows = std::fs::read_to_string(&windows_meta_path)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("parse windows json"))
        .collect();

    (tmp, carved_rows, windows_rows)
}

fn place_at(image: &mut Vec<u8>, offset: usize, payload: &[u8]) {
    if image.len() < offset {
        image.resize(offset, 0);
    }
    if image.len() < offset + payload.len() {
        image.resize(offset + payload.len(), 0);
    }
    image[offset..offset + payload.len()].copy_from_slice(payload);
}

// ------------------------------------------------------------------
// test_evtx_basic_carve
// ------------------------------------------------------------------

#[test]
fn test_evtx_basic_carve() {
    let evtx = build_evtx(&EvtxBuildSpec {
        chunk_count: 1,
        records_per_chunk: vec![3],
        minor_version: 2, // Win11 v3.2
        file_flags: 0,
    });
    let offset = 0x1000usize;
    let mut image = Vec::new();
    place_at(&mut image, offset, &evtx);
    image.extend(vec![0u8; 4096]); // trailing padding

    let (_tmp, carved_rows, windows_rows) = run_evtx_pipeline(&image);
    assert_eq!(carved_rows.len(), 1, "expected one carved EVTX file");
    assert_eq!(windows_rows.len(), 1, "expected one windows artefact row");

    let carved = &carved_rows[0];
    let win = &windows_rows[0];

    assert_eq!(carved["file_type"], "evtx");
    assert_eq!(carved["extension"], "evtx");
    assert_eq!(carved["global_start"], offset as u64);
    assert_eq!(carved["size"], HEADER_SIZE + CHUNK_SIZE);
    assert_eq!(carved["validated"], true);
    assert_eq!(carved["truncated"], false);

    assert_eq!(win["artefact_type"], "evtx");
    assert_eq!(win["first_chunk"], 0);
    assert_eq!(win["last_chunk"], 0);
    assert_eq!(win["record_count_estimate"], 3);
    assert!(win["log_name"].is_null());
}

// ------------------------------------------------------------------
// test_evtx_chunk_count
// ------------------------------------------------------------------

#[test]
fn test_evtx_chunk_count() {
    let evtx = build_evtx(&EvtxBuildSpec {
        chunk_count: 3,
        records_per_chunk: vec![5, 0, 7], // middle chunk empty
        minor_version: 1,
        file_flags: 0,
    });
    let mut image = vec![0u8; 0x800];
    image.extend_from_slice(&evtx);

    let (_tmp, carved_rows, windows_rows) = run_evtx_pipeline(&image);
    assert_eq!(carved_rows.len(), 1);
    assert_eq!(windows_rows.len(), 1);

    assert_eq!(carved_rows[0]["size"], HEADER_SIZE + 3 * CHUNK_SIZE);
    assert_eq!(windows_rows[0]["last_chunk"], 2);
    assert_eq!(windows_rows[0]["record_count_estimate"], 12);
}

// ------------------------------------------------------------------
// test_evtx_truncated
// ------------------------------------------------------------------

#[test]
fn test_evtx_truncated() {
    // Header declares 2 chunks but image only contains the header + first chunk
    // and is then cut off mid-second-chunk.
    let mut evtx = build_evtx(&EvtxBuildSpec {
        chunk_count: 2,
        records_per_chunk: vec![4, 6],
        minor_version: 1,
        file_flags: 0,
    });
    // Truncate the second chunk to 32 KiB.
    let truncated_len = (HEADER_SIZE + CHUNK_SIZE + 32 * 1024) as usize;
    evtx.truncate(truncated_len);

    let (_tmp, carved_rows, windows_rows) = run_evtx_pipeline(&evtx);
    assert_eq!(carved_rows.len(), 1, "truncated carve should still emit");
    assert_eq!(windows_rows.len(), 1);

    let carved = &carved_rows[0];
    assert_eq!(carved["validated"], false);
    assert_eq!(carved["truncated"], true);
    let size = carved["size"].as_u64().expect("size u64");
    assert_eq!(size as usize, truncated_len);
    let errors = carved["errors"].as_array().expect("errors array");
    assert!(
        errors
            .iter()
            .any(|e| e.as_str().unwrap_or("").contains("truncated")),
        "expected truncated error string"
    );
}

// ------------------------------------------------------------------
// test_evtx_size_limit
// ------------------------------------------------------------------

#[test]
fn test_evtx_size_limit() {
    // Build an EVTX whose declared size exceeds the configured max_size.
    // Default max_size is 1 GiB == 16384 chunks. We override max_size to
    // be tiny so a 2-chunk fixture is rejected.
    let evtx = build_evtx(&EvtxBuildSpec {
        chunk_count: 2,
        records_per_chunk: vec![1, 1],
        minor_version: 1,
        file_flags: 0,
    });

    let tmp = tempfile::tempdir().expect("tempdir");
    let raw_path = tmp.path().join("image.raw");
    let mut image = vec![0u8; 0x1000];
    image.extend_from_slice(&evtx);
    std::fs::write(&raw_path, &image).expect("write raw");

    let loaded = config::load_config(None).expect("config");
    let mut cfg = loaded.config;
    cfg.run_id = "carver_test_evtx_size".to_string();
    cfg.file_types.retain(|ft| ft.id == "evtx");
    // Force max_size below `4096 + 2*65536 = 135168` bytes.
    for ft in cfg.file_types.iter_mut() {
        ft.max_size = HEADER_SIZE + CHUNK_SIZE; // only one chunk allowed
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
    .expect("sink");
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

    let carved_meta_path = run_output_dir.join("metadata").join("carved_files.jsonl");
    let content = std::fs::read_to_string(&carved_meta_path).unwrap_or_default();
    assert!(
        !content.lines().any(|l| !l.trim().is_empty()),
        "expected no carved rows when total_size > max_size, got: {content}"
    );
}

// ------------------------------------------------------------------
// test_evtx_corrupt_declared_chunk_stops_extent
// ------------------------------------------------------------------

#[test]
fn test_evtx_corrupt_declared_chunk_stops_extent() {
    let mut evtx = build_evtx(&EvtxBuildSpec {
        chunk_count: 3,
        records_per_chunk: vec![2, 3, 4],
        minor_version: 1,
        file_flags: 0,
    });
    // Break chunk 1. Chunk 2 still has a valid ElfChnk magic, but it is no
    // longer contiguous verified EVTX data and must not extend the carve.
    let corrupt_chunk_offset = (HEADER_SIZE + CHUNK_SIZE) as usize;
    evtx[corrupt_chunk_offset] = b'X';

    let (_tmp, carved_rows, windows_rows) = run_evtx_pipeline(&evtx);
    assert_eq!(carved_rows.len(), 1);
    assert_eq!(windows_rows.len(), 1);

    let carved = &carved_rows[0];
    assert_eq!(carved["size"], HEADER_SIZE + CHUNK_SIZE);
    assert_eq!(carved["validated"], false);
    assert_eq!(carved["truncated"], false);
    let errors = carved["errors"].as_array().expect("errors array");
    assert!(
        errors
            .iter()
            .any(|e| e.as_str().unwrap_or("").contains("bad ElfChnk magic")),
        "expected corrupt chunk error string"
    );
    assert_eq!(windows_rows[0]["record_count_estimate"], 2);
}

// ------------------------------------------------------------------
// test_evtx_dirty_flag — file_flags=1 must still carve cleanly
// ------------------------------------------------------------------

#[test]
fn test_evtx_dirty_flag() {
    let evtx = build_evtx(&EvtxBuildSpec {
        chunk_count: 1,
        records_per_chunk: vec![2],
        minor_version: 1,
        file_flags: 1, // dirty
    });
    let (_tmp, carved_rows, windows_rows) = run_evtx_pipeline(&evtx);
    assert_eq!(carved_rows.len(), 1);
    assert_eq!(windows_rows.len(), 1);
    assert_eq!(carved_rows[0]["validated"], true);
    assert_eq!(windows_rows[0]["record_count_estimate"], 2);
}

// ------------------------------------------------------------------
// test_evtx_invalid_version — major != 3 must be rejected
// ------------------------------------------------------------------

#[test]
fn test_evtx_invalid_version() {
    let mut evtx = build_evtx(&EvtxBuildSpec {
        chunk_count: 1,
        records_per_chunk: vec![1],
        minor_version: 1,
        file_flags: 0,
    });
    // Overwrite major version (offset 0x26) to 4.
    evtx[0x26..0x28].copy_from_slice(&4u16.to_le_bytes());

    let (_tmp, carved_rows, _windows_rows) = run_evtx_pipeline(&evtx);
    assert!(
        carved_rows.is_empty(),
        "EVTX with major != 3 must be rejected"
    );
}

// ------------------------------------------------------------------
// test_evtx_record_count_overflow_preserves_windows_row
//
// Regression for issue #80: an EVTX chunk header that declares an
// implausible record range (last_record == u64::MAX) must NOT cause
// the corresponding `windows_artefacts` row to be dropped by the
// metadata sink. The carved file row and the Windows artefact row
// must both be emitted, with `record_count_estimate` reported as NULL.
// ------------------------------------------------------------------

#[test]
fn test_evtx_record_count_overflow_preserves_windows_row() {
    use evtx_fixture::{build_chunk, build_file_header};

    // Two chunks: the first declares first_record=1, last_record=u64::MAX
    // (implausible / not representable as i64). The second is well-formed.
    // The carver must still produce both metadata rows.
    let mut evtx = Vec::with_capacity((HEADER_SIZE + 2 * CHUNK_SIZE) as usize);
    evtx.extend(build_file_header(0, 1, 2, 1, 0));
    evtx.extend(build_chunk(1, u64::MAX));
    evtx.extend(build_chunk(1, 5));

    let (_tmp, carved_rows, windows_rows) = run_evtx_pipeline(&evtx);
    assert_eq!(carved_rows.len(), 1, "carved file row must be present");
    assert_eq!(
        windows_rows.len(),
        1,
        "windows_artefacts row must NOT be dropped on i64 overflow"
    );

    let carved = &carved_rows[0];
    assert_eq!(carved["file_type"], "evtx");
    assert_eq!(carved["validated"], true);
    assert_eq!(carved["truncated"], false);

    let win = &windows_rows[0];
    assert_eq!(win["artefact_type"], "evtx");
    assert!(
        win["record_count_estimate"].is_null(),
        "record_count_estimate must be NULL when chunk range exceeds i64, got: {}",
        win["record_count_estimate"]
    );
}
