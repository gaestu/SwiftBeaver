use std::sync::Arc;

use swiftbeaver::config;
use swiftbeaver::evidence::RawFileSource;
use swiftbeaver::metadata::{self, MetadataBackendKind};
use swiftbeaver::pipeline;
use swiftbeaver::scanner;
use swiftbeaver::util;

#[path = "common/prefetch_fixture.rs"]
mod prefetch_fixture;

use prefetch_fixture::{build_mam_prefetch, build_scca_prefetch};

fn run_prefetch_pipeline(
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
    cfg.run_id = "carver_test_prefetch".to_string();
    cfg.file_types.retain(|ft| ft.id == "prefetch");

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

/// Windows FILETIME for 2020-06-15 10:00:00 UTC
fn sample_filetime() -> u64 {
    // FILETIME = (unix_secs + 11_644_473_600) * 10_000_000
    let unix: i64 = 1_592_215_200; // 2020-06-15 10:00:00 UTC
    ((unix + 11_644_473_600) as u64) * 10_000_000
}

fn write_u32_le(buf: &mut [u8], offset: usize, value: u32) {
    buf[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn utf16le(value: &str) -> Vec<u8> {
    value
        .encode_utf16()
        .flat_map(|unit| unit.to_le_bytes())
        .collect()
}

fn add_v30_volume_paths(pf: &mut Vec<u8>, paths: &[String]) {
    const SCCA_FILE_SIZE_OFFSET: usize = 12;
    const VOLUME_INFO_OFFSET_FIELD_V30: usize = 0x78;
    const VOLUME_INFO_ENTRY_SIZE_V30_V31: usize = 96;

    let vi_offset = pf.len();
    let entry_array_size = VOLUME_INFO_ENTRY_SIZE_V30_V31 * paths.len();
    let encoded_paths: Vec<Vec<u8>> = paths.iter().map(|path| utf16le(path)).collect();
    let path_bytes_len: usize = encoded_paths.iter().map(Vec::len).sum();
    let section_size = entry_array_size + path_bytes_len;
    let total_len = vi_offset + section_size;
    pf.resize(total_len, 0);

    write_u32_le(pf, VOLUME_INFO_OFFSET_FIELD_V30, vi_offset as u32);
    write_u32_le(pf, VOLUME_INFO_OFFSET_FIELD_V30 + 4, paths.len() as u32);
    write_u32_le(pf, VOLUME_INFO_OFFSET_FIELD_V30 + 8, section_size as u32);
    write_u32_le(pf, SCCA_FILE_SIZE_OFFSET, total_len as u32);

    let mut next_path_offset = entry_array_size;
    for (idx, encoded_path) in encoded_paths.iter().enumerate() {
        let entry_offset = vi_offset + idx * VOLUME_INFO_ENTRY_SIZE_V30_V31;
        write_u32_le(pf, entry_offset, next_path_offset as u32);
        write_u32_le(pf, entry_offset + 4, (encoded_path.len() / 2) as u32);

        let path_start = vi_offset + next_path_offset;
        let path_end = path_start + encoded_path.len();
        pf[path_start..path_end].copy_from_slice(encoded_path);
        next_path_offset += encoded_path.len();
    }
}

// ------------------------------------------------------------------
// test_prefetch_win7_uncompressed
// ------------------------------------------------------------------

#[test]
fn test_prefetch_win7_uncompressed() {
    let pf = build_scca_prefetch(
        23, // Vista/7
        "NOTEPAD.EXE",
        0xABCD_1234,
        5,
        sample_filetime(),
    );
    let offset = 0x200usize;
    let mut image = vec![0u8; offset];
    image.extend_from_slice(&pf);
    image.extend_from_slice(&[0xAAu8; 32]);

    let (_tmp, carved_rows, windows_rows) = run_prefetch_pipeline(&image);
    assert_eq!(carved_rows.len(), 1, "expected one carved prefetch file");
    assert_eq!(windows_rows.len(), 1, "expected one windows artefact row");

    let carved = &carved_rows[0];
    let windows = &windows_rows[0];

    assert_eq!(carved["file_type"], "prefetch");
    assert_eq!(carved["global_start"], offset as u64);

    assert_eq!(windows["artefact_type"], "prefetch");
    assert_eq!(windows["version"], 23);
    assert_eq!(windows["run_count"], 5);
    assert_eq!(windows["prefetch_hash"], "ABCD1234");
    assert_eq!(windows["executable_name"], "NOTEPAD.EXE");
    assert_eq!(windows["volume_paths_truncated"], false);
    assert!(windows["referenced_files_json"].is_null());
}

// ------------------------------------------------------------------
// test_prefetch_executable_name
// ------------------------------------------------------------------

#[test]
fn test_prefetch_executable_name() {
    let pf = build_scca_prefetch(
        17, // XP
        "CMD.EXE",
        0x1234_5678,
        3,
        sample_filetime(),
    );
    let mut image = vec![0u8; 0x100];
    image.extend_from_slice(&pf);
    image.extend_from_slice(&[0u8; 16]);

    let (_tmp, _carved_rows, windows_rows) = run_prefetch_pipeline(&image);
    assert_eq!(windows_rows.len(), 1);
    assert_eq!(windows_rows[0]["executable_name"], "CMD.EXE");
}

// ------------------------------------------------------------------
// test_prefetch_run_count
// ------------------------------------------------------------------

#[test]
fn test_prefetch_run_count() {
    let pf = build_scca_prefetch(30, "EXPLORER.EXE", 0xDEAD_BEEF, 42, sample_filetime());
    let mut image = vec![0u8; 0x100];
    image.extend_from_slice(&pf);
    image.extend_from_slice(&[0u8; 16]);

    let (_tmp, _carved_rows, windows_rows) = run_prefetch_pipeline(&image);
    assert_eq!(windows_rows.len(), 1);
    assert_eq!(windows_rows[0]["run_count"], 42);
}

#[test]
fn test_prefetch_volume_paths_truncation_is_exposed_in_metadata() {
    let mut pf = build_scca_prefetch(30, "SVCHOST.EXE", 0xCAFE_BABE, 8, sample_filetime());
    let volume_paths: Vec<String> = (0..33)
        .map(|idx| format!(r"\Device\HarddiskVolume{idx:02}"))
        .collect();
    add_v30_volume_paths(&mut pf, &volume_paths);

    let mut image = vec![0u8; 0x100];
    image.extend_from_slice(&pf);
    image.extend_from_slice(&[0u8; 16]);

    let (_tmp, _carved_rows, windows_rows) = run_prefetch_pipeline(&image);
    assert_eq!(windows_rows.len(), 1);

    let windows = &windows_rows[0];
    assert_eq!(windows["volume_paths_truncated"], true);

    let decoded_paths: Vec<String> = serde_json::from_str(
        windows["volume_paths_json"]
            .as_str()
            .expect("volume paths json"),
    )
    .expect("decode volume paths");
    assert_eq!(decoded_paths.len(), 32);
    assert_eq!(
        decoded_paths.first().map(String::as_str),
        Some(r"\Device\HarddiskVolume00")
    );
}

#[test]
fn test_prefetch_version_31_supported() {
    let pf = build_scca_prefetch(31, "TASKHOSTW.EXE", 0x0BAD_F00D, 9, sample_filetime());
    let mut image = vec![0u8; 0x100];
    image.extend_from_slice(&pf);
    image.extend_from_slice(&[0u8; 16]);

    let (_tmp, carved_rows, windows_rows) = run_prefetch_pipeline(&image);
    assert_eq!(carved_rows.len(), 1);
    assert_eq!(windows_rows.len(), 1);
    assert_eq!(windows_rows[0]["version"], 31);
    assert_eq!(windows_rows[0]["executable_name"], "TASKHOSTW.EXE");
    assert_eq!(windows_rows[0]["run_count"], 9);
}

// ------------------------------------------------------------------
// test_prefetch_truncated
// ------------------------------------------------------------------

#[test]
fn test_prefetch_truncated() {
    let mut pf = build_scca_prefetch(23, "CALC.EXE", 0x0000_0001, 1, sample_filetime());
    // Truncate to less than the minimum header size
    pf.truncate(40);

    let tmp = tempfile::tempdir().expect("tempdir");
    let raw_path = tmp.path().join("image.raw");
    std::fs::write(&raw_path, &pf).expect("write raw image");

    let loaded = config::load_config(None).expect("config");
    let mut cfg = loaded.config;
    cfg.run_id = "carver_test_prefetch_truncated".to_string();
    cfg.file_types.retain(|ft| ft.id == "prefetch");

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
        512,
        None,
        None,
        carve_registry,
    )
    .expect("pipeline");

    // A truncated file that is smaller than min_size=84 should not be carved
    assert_eq!(stats.files_carved, 0);
}

// ------------------------------------------------------------------
// test_prefetch_mam_carves_correctly
// Verifies that the MAM (compressed) path carves correctly and that
// sentinel bytes appended after the MAM record are NOT included.
// ------------------------------------------------------------------

#[test]
fn test_prefetch_mam_carves_correctly() {
    let scca = build_scca_prefetch(30, "POWERSHELL.EXE", 0x1234_ABCD, 7, sample_filetime());
    let mam = build_mam_prefetch(&scca);
    let mam_len = mam.len();
    // Sanity-check fixture size: 4 (magic) + 4 (size) + 256 (table) + 512 (payload) = 776
    assert!(
        mam_len > 512,
        "build_mam_prefetch produced {mam_len} bytes (expected > 512); fixture bug?"
    );

    let offset = 0x200usize;
    let mut image = vec![0u8; offset];
    image.extend_from_slice(&mam);
    // Sentinel bytes that must NOT be included in the carved output
    image.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0xBA, 0xBE]);

    let (_tmp, carved_rows, windows_rows) = run_prefetch_pipeline(&image);

    // The scanner may produce multiple hits (pattern matches inside the payload),
    // but at least one valid carved record must exist at exactly the MAM offset.
    assert!(
        !carved_rows.is_empty(),
        "expected at least one carved MAM prefetch"
    );
    assert!(
        !windows_rows.is_empty(),
        "expected at least one windows artefact row"
    );

    // Find the row for the MAM record at the correct offset
    let carved = carved_rows
        .iter()
        .find(|r| r["global_start"].as_u64() == Some(offset as u64))
        .expect("no carved row at the expected MAM offset");
    let windows = windows_rows
        .iter()
        .find(|r| r["executable_name"].as_str() == Some("POWERSHELL.EXE"))
        .expect("no windows row with expected exe name");

    assert_eq!(carved["file_type"], "prefetch");

    // Carved size must equal the MAM record length exactly — no sentinel bytes
    let carved_size = carved["size"].as_u64().expect("size field");
    assert_eq!(
        carved_size, mam_len as u64,
        "carved size ({carved_size}) must match MAM record length ({mam_len}), \
         sentinel bytes must not be included"
    );

    assert_eq!(windows["artefact_type"], "prefetch");
    assert_eq!(windows["executable_name"], "POWERSHELL.EXE");
    assert_eq!(windows["prefetch_hash"], "1234ABCD");
    assert_eq!(windows["run_count"], 7);
    assert_eq!(windows["volume_paths_truncated"], false);
}

// ------------------------------------------------------------------
// test_prefetch_mam_metadata_size_consistent
// Verifies that carved_files.size and windows_artefacts.size agree for
// a MAM-compressed Prefetch (provenance consistency between metadata sinks).
// ------------------------------------------------------------------

#[test]
fn test_prefetch_mam_metadata_size_consistent() {
    let scca = build_scca_prefetch(30, "CHROME.EXE", 0xFEED_FACE, 12, sample_filetime());
    let mam = build_mam_prefetch(&scca);

    let offset = 0x100usize;
    let mut image = vec![0u8; offset];
    image.extend_from_slice(&mam);
    image.extend_from_slice(&[0u8; 64]);

    let (_tmp, carved_rows, windows_rows) = run_prefetch_pipeline(&image);
    assert_eq!(carved_rows.len(), 1);
    assert_eq!(windows_rows.len(), 1);

    let carved_size = carved_rows[0]["size"].as_u64().expect("carved size");
    let windows_size = windows_rows[0]["size"].as_u64().expect("windows size");

    assert_eq!(
        carved_size, windows_size,
        "carved_files.size ({carved_size}) and windows_artefacts.size ({windows_size}) \
         must agree for provenance consistency"
    );
}

// ------------------------------------------------------------------
// End-to-end tests against the checked-in .pf samples.
//
// These exercise the carver against the actual files under
// tests/golden_image/samples/windows/prefetch/ rather than synthetic
// fixtures, so any drift between the parser and the real (spec-correct)
// SCCA / MAM layouts shows up here.
// ------------------------------------------------------------------

fn carve_real_sample(rel_path: &str) -> Vec<serde_json::Value> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden_image/samples/windows/prefetch")
        .join(rel_path);
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {rel_path}: {e}"));
    let (_tmp, _carved, windows) = run_prefetch_pipeline(&bytes);
    windows
}

#[test]
fn test_real_sample_v17_xp() {
    let rows = carve_real_sample("NOTEPAD.EXE-ABCD1234.pf");
    assert_eq!(rows.len(), 1, "expected exactly one prefetch artefact row");
    let r = &rows[0];
    assert_eq!(r["version"], 17);
    assert_eq!(r["executable_name"], "NOTEPAD.EXE");
    assert_eq!(r["prefetch_hash"], "ABCD1234");
    assert_eq!(r["run_count"], 3);
    assert_eq!(r["volume_paths_truncated"], false);
    let times = r["last_run_times_json"].as_str().expect("times json");
    assert!(
        times.contains("2021-01-01T00:00:00"),
        "v17 sample timestamp missing: {times}"
    );
}

#[test]
fn test_real_sample_v23_vista_7() {
    let rows = carve_real_sample("NOTEPAD.EXE-B1234567.pf");
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(r["version"], 23);
    assert_eq!(r["executable_name"], "NOTEPAD.EXE");
    assert_eq!(r["prefetch_hash"], "B1234567");
    assert_eq!(r["run_count"], 5);
    assert_eq!(r["volume_paths_truncated"], false);
    let times = r["last_run_times_json"].as_str().expect("times json");
    assert!(
        times.contains("2021-01-01T00:00:00"),
        "v23 sample timestamp missing: {times}"
    );
}

#[test]
fn test_real_sample_v26_win8() {
    let rows = carve_real_sample("NOTEPAD.EXE-C2345678.pf");
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(r["version"], 26);
    assert_eq!(r["run_count"], 2);
    assert_eq!(r["volume_paths_truncated"], false);
    let times = r["last_run_times_json"].as_str().expect("times json");
    assert!(times.contains("2021-01-01T00:00:00"));
}

#[test]
fn test_real_sample_v30_win10_uncompressed() {
    let rows = carve_real_sample("NOTEPAD.EXE-D3456789.pf");
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(r["version"], 30);
    assert_eq!(r["run_count"], 7);
    assert_eq!(r["volume_paths_truncated"], false);
    let times = r["last_run_times_json"].as_str().expect("times json");
    // v30 stores up to 8 timestamps; first three are populated in the sample.
    assert!(times.contains("2021-01-01T00:00:00"));
    assert!(times.contains("2020-12-31T00:00:00"));
    assert!(times.contains("2020-12-30T00:00:00"));
}

#[test]
fn test_real_sample_v30_win10_mam_compressed() {
    let rows = carve_real_sample("CMD.EXE-E4567890.pf");
    assert_eq!(
        rows.len(),
        1,
        "expected MAM sample to decompress and yield one artefact row"
    );
    let r = &rows[0];
    assert_eq!(r["version"], 30);
    assert_eq!(r["executable_name"], "CMD.EXE");
    assert_eq!(r["prefetch_hash"], "E4567890");
    assert_eq!(r["run_count"], 12);
    assert_eq!(r["volume_paths_truncated"], false);
    let times = r["last_run_times_json"].as_str().expect("times json");
    assert!(times.contains("2021-01-01T00:00:00"));
    assert!(times.contains("2020-12-31T00:00:00"));
}
