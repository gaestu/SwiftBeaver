use std::fs::File;
use std::io::Write;
use std::sync::Arc;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

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

fn run_pipeline(input_path: &std::path::Path, max_files: Option<u64>) -> pipeline::PipelineStats {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let loaded = config::load_config(None).expect("config");
    let mut cfg = loaded.config;
    cfg.run_id = "bench".to_string();
    cfg.max_files = max_files;

    let evidence = RawFileSource::open(input_path).expect("evidence");
    let evidence: Arc<dyn swiftbeaver::evidence::EvidenceSource> = Arc::new(evidence);

    let run_output_dir = temp_dir.path().join("run");
    std::fs::create_dir_all(&run_output_dir).expect("output dir");

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
        2,
        4 * 1024 * 1024,
        64 * 1024,
        None,
        None,
        carve_registry,
    )
    .expect("pipeline")
}

fn bench_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("pipeline");
    for size in [8 * 1024 * 1024usize, 32 * 1024 * 1024usize] {
        group.bench_with_input(BenchmarkId::new("scan_only", size), &size, |b, &size| {
            b.iter(|| {
                let temp_dir = tempfile::tempdir().expect("tempdir");
                let input_path = temp_dir.path().join("image.bin");
                let file = File::create(&input_path).expect("create");
                file.set_len(size as u64).expect("set len");
                run_pipeline(&input_path, None);
            });
        });
    }

    group.bench_function("jpeg_dense", |b| {
        b.iter(|| {
            let temp_dir = tempfile::tempdir().expect("tempdir");
            let input_path = temp_dir.path().join("dense.bin");
            let mut file = File::create(&input_path).expect("create");
            let jpeg = minimal_jpeg();
            let padding = vec![0u8; 32];
            for _ in 0..500 {
                file.write_all(&jpeg).expect("write");
                file.write_all(&padding).expect("write");
            }
            file.flush().expect("flush");
            run_pipeline(&input_path, Some(200));
        });
    });

    group.finish();
}

criterion_group!(benches, bench_throughput);
criterion_main!(benches);
