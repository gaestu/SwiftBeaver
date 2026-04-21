use std::sync::Arc;

use swiftbeaver::config;
use swiftbeaver::evidence::RawFileSource;
use swiftbeaver::metadata::{self, MetadataBackendKind};
use swiftbeaver::pipeline;
use swiftbeaver::scanner;
use swiftbeaver::util;

#[path = "common/lnk_fixture.rs"]
mod lnk_fixture;

use lnk_fixture::build_lnk;

fn run_lnk_pipeline_with_cfg(
    image_bytes: &[u8],
    configure: impl FnOnce(&mut config::Config),
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
    cfg.run_id = "carver_test_lnk".to_string();
    cfg.file_types.retain(|ft| ft.id == "lnk");
    configure(&mut cfg);

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

fn run_lnk_pipeline(
    image_bytes: &[u8],
) -> (tempfile::TempDir, serde_json::Value, serde_json::Value) {
    let (tmp, carved_rows, windows_rows) = run_lnk_pipeline_with_cfg(image_bytes, |_| {});
    let carved = carved_rows.into_iter().next().expect("one carved file row");
    let windows = windows_rows.into_iter().next().expect("one windows row");
    (tmp, carved, windows)
}

#[test]
fn test_lnk_basic_carve() {
    let lnk = build_lnk(
        Some(r"C:\Windows\System32"),
        None,
        Some("notepad.exe"),
        Some(r"..\..\Windows\System32\notepad.exe"),
        Some(r"C:\Windows"),
    );
    let offset = 0x100usize;
    let mut image = vec![0u8; offset];
    image.extend_from_slice(&lnk);
    image.extend_from_slice(&[0xAA; 32]);

    let (tmp, carved, windows) = run_lnk_pipeline(&image);

    assert_eq!(carved["file_type"], "lnk");
    assert_eq!(carved["global_start"], offset as u64);
    assert_eq!(carved["size"], lnk.len() as u64);

    let rel_path = carved["path"].as_str().expect("relative path");
    let carved_path = tmp
        .path()
        .join("carver_test_lnk")
        .join("carved")
        .join(rel_path);
    assert!(
        carved_path.exists(),
        "carved file missing: {}",
        carved_path.display()
    );

    assert_eq!(windows["artefact_type"], "lnk");
    assert_eq!(
        windows["target_path"],
        serde_json::Value::String(r"C:\Windows\System32\notepad.exe".to_string())
    );
    assert_eq!(
        windows["working_dir"],
        serde_json::Value::String(r"C:\Windows".to_string())
    );
    assert_eq!(
        windows["volume_serial"],
        serde_json::Value::String("ABCD-1234".to_string())
    );
}

#[test]
fn test_lnk_truncated() {
    let mut lnk = build_lnk(
        Some(r"C:\Temp"),
        None,
        Some("report.txt"),
        Some(r".\report.txt"),
        Some(r"C:\Temp"),
    );
    lnk.truncate(lnk.len() - 2);

    let tmp = tempfile::tempdir().expect("tempdir");
    let raw_path = tmp.path().join("image.raw");
    std::fs::write(&raw_path, &lnk).expect("write raw image");

    let loaded = config::load_config(None).expect("config");
    let mut cfg = loaded.config;
    cfg.run_id = "carver_test_lnk_truncated".to_string();
    cfg.file_types.retain(|ft| ft.id == "lnk");

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

    assert_eq!(stats.files_carved, 0);
    let carved_meta_path = run_output_dir.join("metadata").join("carved_files.jsonl");
    let windows_meta_path = run_output_dir
        .join("metadata")
        .join("windows_artefacts.jsonl");
    if carved_meta_path.exists() {
        let content = std::fs::read_to_string(&carved_meta_path).expect("read carved metadata");
        assert!(content.trim().is_empty());
    }
    if windows_meta_path.exists() {
        let content = std::fs::read_to_string(&windows_meta_path).expect("read windows metadata");
        assert!(content.trim().is_empty());
    }
}

#[test]
fn test_lnk_duplicate_skip_does_not_emit_windows_sidecar() {
    let lnk = build_lnk(
        Some(r"C:\Temp"),
        None,
        Some("report.txt"),
        Some(r".\report.txt"),
        Some(r"C:\Temp"),
    );
    let mut image = vec![0u8; 0x80];
    image.extend_from_slice(&lnk);
    image.extend_from_slice(&[0u8; 32]);
    image.extend_from_slice(&lnk);

    let (_tmp, carved_rows, windows_rows) = run_lnk_pipeline_with_cfg(&image, |cfg| {
        cfg.enable_deduplication = true;
        cfg.skip_duplicate_files = true;
    });

    assert_eq!(
        carved_rows.len(),
        2,
        "file metadata still records duplicates"
    );
    assert_eq!(
        windows_rows.len(),
        1,
        "duplicate-discarded LNK should not emit a second windows artefact row"
    );
}

#[test]
fn test_lnk_exact_max_size_boundary() {
    let base_len = build_lnk(None, None, Some(""), None, None).len();
    let suffix = "A".repeat(65_536 - base_len);
    let lnk = build_lnk(None, None, Some(&suffix), None, None);

    assert_eq!(lnk.len(), 65_536);
    let (_tmp, carved, windows) = run_lnk_pipeline(&lnk);
    assert_eq!(carved["size"], 65_536);
    assert_eq!(windows["size"], 65_536);
}
