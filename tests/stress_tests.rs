use std::fs::{self, File};
use std::io::{Seek, SeekFrom, Write};
use std::sync::Arc;

use swiftbeaver::chunk::build_chunks;
use swiftbeaver::config;
use swiftbeaver::evidence::RawFileSource;
use swiftbeaver::metadata::{self, MetadataBackendKind};
use swiftbeaver::pipeline;
use swiftbeaver::scanner;
use swiftbeaver::util;

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

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
    chunk_size: u64,
    overlap: u64,
    workers: usize,
) -> pipeline::PipelineStats {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let loaded = config::load_config(None).expect("config");
    let mut cfg = loaded.config;
    cfg.run_id = "stress_test".to_string();
    cfg.max_files = max_files;

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

    let carve_registry = Arc::new(util::build_carve_registry(&cfg).expect("registry"));

    pipeline::run_pipeline(
        &cfg,
        evidence,
        sig_scanner,
        None,
        meta_sink,
        &run_output_dir,
        workers,
        chunk_size,
        overlap,
        None,
        None,
        carve_registry,
    )
    .expect("pipeline")
}

#[test]
#[ignore = "stress test"]
fn stress_large_image_scan() {
    let size = env_u64("SWIFTBEAVER_STRESS_BYTES", 64 * 1024 * 1024);
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let input_path = temp_dir.path().join("large.bin");
    let file = File::create(&input_path).expect("create");
    file.set_len(size).expect("set_len");

    let chunk_size = 4 * 1024 * 1024;
    let overlap = 64 * 1024;
    let stats = run_pipeline(&input_path, None, chunk_size, overlap, 2);
    let expected_bytes: u64 = build_chunks(size, chunk_size, overlap)
        .iter()
        .map(|chunk| chunk.length)
        .sum();
    assert_eq!(stats.bytes_scanned, expected_bytes);
    assert_eq!(stats.hits_found, 0);
}

#[test]
#[ignore = "stress test"]
fn stress_high_hit_density() {
    let hits = env_u64("SWIFTBEAVER_STRESS_HITS", 1_000);
    let max_files = env_u64("SWIFTBEAVER_STRESS_MAX_FILES", 200);
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

    let stats = run_pipeline(&input_path, Some(max_files), 64 * 1024, 256, 1);
    assert!(stats.files_carved <= max_files);
    assert!(stats.files_carved > 0);
    assert!(stats.hits_found >= max_files);
}
