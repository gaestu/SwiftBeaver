use std::fs::{self, File};
use std::io::{Seek, SeekFrom, Write};
use std::sync::Arc;

use swiftbeaver::config;
use swiftbeaver::evidence::RawFileSource;
use swiftbeaver::metadata::{self, MetadataBackendKind};
use swiftbeaver::pipeline;
use swiftbeaver::scanner;
use swiftbeaver::util;

fn minimal_jpeg() -> Vec<u8> {
    let mut jpeg = vec![0u8; 32];
    jpeg[0..4].copy_from_slice(&[0xFF, 0xD8, 0xFF, 0xE0]);
    jpeg[4..9].copy_from_slice(b"JFIF\0");
    jpeg[30..32].copy_from_slice(&[0xFF, 0xD9]);
    jpeg
}

fn run_pipeline(
    input_path: &std::path::Path,
    max_files: Option<u64>,
    workers: usize,
) -> pipeline::PipelineStats {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let loaded = config::load_config(None).expect("config");
    let mut cfg = loaded.config;
    cfg.run_id = "max_files_test".to_string();
    cfg.max_files = max_files;
    for file_type in &mut cfg.file_types {
        if file_type.id.eq_ignore_ascii_case("jpeg")
            || file_type.validator.trim().eq_ignore_ascii_case("jpeg")
        {
            file_type.min_size = 0;
        }
    }

    let evidence = RawFileSource::open(input_path).expect("evidence");
    let evidence: Arc<dyn swiftbeaver::evidence::EvidenceSource> = Arc::new(evidence);

    let run_output_dir = temp_dir.path().join("run");
    fs::create_dir_all(&run_output_dir).expect("output dir");

    let meta_sink = metadata::build_sink(
        MetadataBackendKind::Jsonl,
        &cfg,
        &cfg.run_id,
        env!("CARGO_PKG_VERSION"),
        &loaded.config_hash,
        input_path,
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
        workers,
        64 * 1024,
        256,
        None,
        None,
        carve_registry,
    )
    .expect("pipeline")
}

#[test]
fn enforces_strict_max_files_under_concurrency() {
    let hits = 32usize;
    let max_files = 5u64;
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let input_path = temp_dir.path().join("dense.bin");
    let mut file = File::create(&input_path).expect("create");
    let jpeg = minimal_jpeg();
    let padding = vec![0u8; 32];

    for _ in 0..hits {
        file.write_all(&jpeg).expect("write jpeg");
        file.write_all(&padding).expect("write padding");
    }
    file.flush().expect("flush");
    file.seek(SeekFrom::Start(0)).expect("seek");

    let stats = run_pipeline(&input_path, Some(max_files), 4);
    assert!(stats.files_carved <= max_files);
    assert!(stats.files_carved > 0);
    assert!(stats.hits_found >= max_files);
}
