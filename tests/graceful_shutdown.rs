use std::fs;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use swiftbeaver::config;
use swiftbeaver::evidence::RawFileSource;
use swiftbeaver::metadata::{self, MetadataBackendKind};
use swiftbeaver::pipeline;
use swiftbeaver::scanner;
use swiftbeaver::util;

#[test]
fn cancel_flag_stops_pipeline_early() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let input_path = temp_dir.path().join("input.bin");
    fs::write(&input_path, vec![0u8; 1024]).expect("write input");

    let loaded = config::load_config(None).expect("config");
    let mut cfg = loaded.config;
    cfg.run_id = "cancel_test".to_string();

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

    let carve_registry = Arc::new(util::build_carve_registry(&cfg).expect("registry"));

    let cancel_flag = Arc::new(AtomicBool::new(true));
    let stats = pipeline::run_pipeline_with_cancel(
        &cfg,
        evidence,
        sig_scanner,
        None,
        meta_sink,
        &run_output_dir,
        1,
        64,
        0,
        None,
        None,
        carve_registry,
        cancel_flag,
        None,
        None,
    )
    .expect("pipeline");

    assert_eq!(stats.bytes_scanned, 0);
    assert_eq!(stats.chunks_processed, 0);
    assert_eq!(stats.hits_found, 0);
    assert_eq!(stats.files_carved, 0);
    assert_eq!(stats.string_spans, 0);
    assert_eq!(stats.artefacts_extracted, 0);
}
