use std::fs;
use std::sync::Arc;

use swiftbeaver::config;
use swiftbeaver::evidence::RawFileSource;
use swiftbeaver::metadata::{self, MetadataBackendKind};
use swiftbeaver::pipeline;
use swiftbeaver::scanner;
use swiftbeaver::util;

fn insert_bytes(target: &mut Vec<u8>, offset: usize, data: &[u8]) {
    let end = offset + data.len();
    if end > target.len() {
        target.resize(end, 0u8);
    }
    target[offset..end].copy_from_slice(data);
}

fn sample_jpeg() -> Vec<u8> {
    // Structurally valid 32-byte JPEG: SOI, APP0(len=4), SOS(len=2), entropy, EOI.
    let mut data = Vec::with_capacity(32);
    data.extend_from_slice(&[0xFF, 0xD8]);
    data.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00]);
    data.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02]);
    data.resize(30, 0u8);
    data.extend_from_slice(&[0xFF, 0xD9]);
    data
}

fn sample_png() -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    data.extend_from_slice(&[0x00, 0x00, 0x00, 0x0D]);
    data.extend_from_slice(b"IHDR");
    data.extend_from_slice(&[
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00,
    ]);
    data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    data.extend_from_slice(b"IEND");
    data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    data
}

fn sample_gif() -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(b"GIF89a");
    data.extend_from_slice(&[0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00]);
    data.extend_from_slice(&[0x21, 0xF9, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00]);
    data.push(0x3B);
    data
}

fn sample_sqlite() -> Vec<u8> {
    let mut data = vec![0u8; 1024];
    data[0..16].copy_from_slice(b"SQLite format 3\0");
    data[16..18].copy_from_slice(&[0x04, 0x00]); // page size 1024
    data[28..32].copy_from_slice(&[0x00, 0x00, 0x00, 0x01]); // page count 1
    data
}

fn sample_pdf() -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(b"%PDF-1.4\n");
    data.extend_from_slice(b"1 0 obj\n<<>>\nendobj\n");
    while data.len() < 64 {
        data.push(b' ');
    }
    data.extend_from_slice(b"%%EOF");
    data
}

fn sample_pdf_with_nested_header() -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(b"%PDF-1.4\n");
    data.extend_from_slice(b"1 0 obj\n<< /Type /Catalog >>\nendobj\n");
    data.extend_from_slice(b"stream\n%PDF-1.4 nested marker\nendstream\n");
    while data.len() < 96 {
        data.push(b' ');
    }
    data.extend_from_slice(b"%%EOF");
    data
}

fn sample_docx_zip() -> Vec<u8> {
    let name = b"word/document.xml";
    let name_len = name.len() as u16;
    let mut out = Vec::new();

    out.extend_from_slice(b"PK\x03\x04");
    out.extend_from_slice(&[0x14, 0x00]);
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    out.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]);
    out.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]);
    out.extend_from_slice(&name_len.to_le_bytes());
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(name);
    out.extend_from_slice(b"x");

    out.extend_from_slice(b"PK\x01\x02");
    out.extend_from_slice(&[0x14, 0x00]);
    out.extend_from_slice(&[0x14, 0x00]);
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    out.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]);
    out.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]);
    out.extend_from_slice(&name_len.to_le_bytes());
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    out.extend_from_slice(name);

    let cd_size = 46 + name.len();
    let cd_offset = 30 + name.len() + 1;
    out.extend_from_slice(b"PK\x05\x06");
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x00]);
    out.extend_from_slice(&[0x01, 0x00]);
    out.extend_from_slice(&[0x01, 0x00]);
    out.extend_from_slice(&(cd_size as u32).to_le_bytes());
    out.extend_from_slice(&(cd_offset as u32).to_le_bytes());
    out.extend_from_slice(&[0x00, 0x00]);

    out
}

fn sample_webp() -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&12u32.to_le_bytes());
    data.extend_from_slice(b"WEBP");
    data.extend_from_slice(b"VP8 ");
    data.extend_from_slice(&0u32.to_le_bytes());
    data
}

fn sample_bmp() -> Vec<u8> {
    let mut data = Vec::new();
    // File size: BMP header (14) + DIB header (40) + 1x1 pixel (4 bytes with padding)
    let file_size = 58u32;
    // BMP file header (14 bytes)
    data.extend_from_slice(b"BM"); // Signature
    data.extend_from_slice(&file_size.to_le_bytes()); // File size
    data.extend_from_slice(&0u16.to_le_bytes()); // Reserved
    data.extend_from_slice(&0u16.to_le_bytes()); // Reserved
    data.extend_from_slice(&54u32.to_le_bytes()); // Pixel data offset (14 + 40)

    // DIB header (BITMAPINFOHEADER - 40 bytes)
    data.extend_from_slice(&40u32.to_le_bytes()); // DIB header size
    data.extend_from_slice(&1i32.to_le_bytes()); // Width = 1
    data.extend_from_slice(&1i32.to_le_bytes()); // Height = 1
    data.extend_from_slice(&1u16.to_le_bytes()); // Planes = 1
    data.extend_from_slice(&24u16.to_le_bytes()); // Bits per pixel = 24
    data.extend_from_slice(&0u32.to_le_bytes()); // Compression = none
    data.extend_from_slice(&4u32.to_le_bytes()); // Image size (1x1 with 4-byte row)
    data.extend_from_slice(&0u32.to_le_bytes()); // X pixels per meter
    data.extend_from_slice(&0u32.to_le_bytes()); // Y pixels per meter
    data.extend_from_slice(&0u32.to_le_bytes()); // Colors used
    data.extend_from_slice(&0u32.to_le_bytes()); // Important colors

    // Pixel data: 1 pixel (3 bytes BGR) + 1 byte padding = 4 bytes
    data.extend_from_slice(&[0xFF, 0x00, 0x00, 0x00]);
    data
}

fn sample_tiff() -> Vec<u8> {
    let mut tiff = Vec::new();
    tiff.extend_from_slice(&[0x49, 0x49, 0x2A, 0x00]);
    tiff.extend_from_slice(&8u32.to_le_bytes());

    let ifd_offset = 8usize;
    let entry_count = 2u16;
    tiff.extend_from_slice(&entry_count.to_le_bytes());

    let strip_offset = (ifd_offset + 2 + 12 * 2 + 4) as u32;
    let strip_len = 256u32;

    tiff.extend_from_slice(&273u16.to_le_bytes());
    tiff.extend_from_slice(&4u16.to_le_bytes());
    tiff.extend_from_slice(&1u32.to_le_bytes());
    tiff.extend_from_slice(&strip_offset.to_le_bytes());

    tiff.extend_from_slice(&279u16.to_le_bytes());
    tiff.extend_from_slice(&4u16.to_le_bytes());
    tiff.extend_from_slice(&1u32.to_le_bytes());
    tiff.extend_from_slice(&strip_len.to_le_bytes());

    tiff.extend_from_slice(&0u32.to_le_bytes());
    tiff.resize(strip_offset as usize + strip_len as usize, 0);
    tiff
}

fn sample_mp4() -> Vec<u8> {
    let mut mp4 = Vec::new();
    mp4.extend_from_slice(&24u32.to_be_bytes());
    mp4.extend_from_slice(b"ftyp");
    mp4.extend_from_slice(b"isom");
    mp4.extend_from_slice(&0u32.to_be_bytes());
    mp4.extend_from_slice(b"isom");
    mp4.extend_from_slice(b"iso2");
    mp4.extend_from_slice(&8u32.to_be_bytes());
    mp4.extend_from_slice(b"moov");
    mp4
}

fn sample_rar4() -> Vec<u8> {
    let mut rar = Vec::new();
    rar.extend_from_slice(&[0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x00]);
    rar.extend_from_slice(&[0u8; 2]);
    rar.push(0x73);
    rar.extend_from_slice(&0u16.to_le_bytes());
    rar.extend_from_slice(&13u16.to_le_bytes());
    rar.extend_from_slice(&[0u8; 6]);
    rar.extend_from_slice(&[0u8; 2]);
    rar.push(0x7B);
    rar.extend_from_slice(&0u16.to_le_bytes());
    rar.extend_from_slice(&12u16.to_le_bytes());
    rar.extend_from_slice(&[0u8; 5]);
    rar
}

fn sample_7z() -> Vec<u8> {
    // Build header bytes 12-31 first (values to be CRCed)
    let next_header_offset: u64 = 0;
    let next_header_size: u64 = 0;
    let next_header_crc: u32 = 0;
    let mut crc_region = Vec::new();
    crc_region.extend_from_slice(&next_header_offset.to_le_bytes());
    crc_region.extend_from_slice(&next_header_size.to_le_bytes());
    crc_region.extend_from_slice(&next_header_crc.to_le_bytes());
    // Compute CRC of bytes 12-31
    let header_crc = crc32fast::hash(&crc_region);

    let mut sevenz = Vec::new();
    sevenz.extend_from_slice(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C]); // bytes 0-5 magic
    sevenz.extend_from_slice(&[0u8, 4u8]); // bytes 6-7 version
    sevenz.extend_from_slice(&header_crc.to_le_bytes()); // bytes 8-11 CRC
    sevenz.extend_from_slice(&crc_region); // bytes 12-31
    sevenz
}

#[test]
fn integration_carves_basic_formats() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let input_path = temp_dir.path().join("image.bin");

    let mut image = vec![0u8; 600_000];
    insert_bytes(&mut image, 1024, &sample_jpeg());
    insert_bytes(&mut image, 65_536, &sample_png());
    insert_bytes(&mut image, 131_072, &sample_gif());
    insert_bytes(&mut image, 150_000, &sample_sqlite());
    insert_bytes(&mut image, 200_000, &sample_pdf());
    insert_bytes(&mut image, 220_000, &sample_docx_zip());
    insert_bytes(&mut image, 260_000, &sample_webp());
    insert_bytes(&mut image, 320_000, &sample_bmp());
    insert_bytes(&mut image, 360_000, &sample_tiff());
    insert_bytes(&mut image, 420_000, &sample_mp4());
    insert_bytes(&mut image, 470_000, &sample_rar4());
    insert_bytes(&mut image, 520_000, &sample_7z());

    fs::write(&input_path, &image).expect("write input");

    let loaded = config::load_config(None).expect("config");
    let mut cfg = loaded.config;
    cfg.run_id = "test_run".to_string();

    // Override min_size for image formats to allow smaller test files
    for ft in cfg.file_types.iter_mut() {
        if ft.id == "jpeg" || ft.id == "gif" || ft.id == "png" || ft.id == "bmp" {
            ft.min_size = 16;
        }
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

    pipeline::run_pipeline(
        &cfg,
        evidence,
        sig_scanner,
        None,
        vec![meta_sink],
        &run_output_dir,
        2,
        2,
        64 * 1024,
        64,
        None,
        None,
        carve_registry,
    )
    .expect("pipeline");

    let carved_root = run_output_dir.join("carved");
    assert!(carved_root.join("jpeg").exists());
    assert!(carved_root.join("png").exists());
    assert!(carved_root.join("gif").exists());
    assert!(carved_root.join("sqlite").exists());
    assert!(carved_root.join("pdf").exists());
    assert!(carved_root.join("docx").exists());
    assert!(carved_root.join("webp").exists());
    assert!(carved_root.join("bmp").exists());
    assert!(carved_root.join("tiff").exists());
    assert!(carved_root.join("mp4").exists());
    assert!(carved_root.join("rar").exists());
    assert!(carved_root.join("7z").exists());

    let meta_path = run_output_dir.join("metadata").join("carved_files.jsonl");
    let contents = fs::read_to_string(meta_path).expect("metadata read");
    let lines: Vec<&str> = contents.lines().collect();
    assert!(lines.len() >= 3, "expected at least 3 records");

    let mut types = Vec::new();
    for line in lines {
        let v: serde_json::Value = serde_json::from_str(line).expect("json");
        if let Some(t) = v.get("file_type").and_then(|v| v.as_str()) {
            types.push(t.to_string());
        }
    }

    assert!(types.contains(&"jpeg".to_string()));
    assert!(types.contains(&"png".to_string()));
    assert!(types.contains(&"gif".to_string()));
    assert!(types.contains(&"sqlite".to_string()));
    assert!(types.contains(&"pdf".to_string()));
    assert!(types.contains(&"docx".to_string()));
    assert!(types.contains(&"webp".to_string()));
    assert!(types.contains(&"bmp".to_string()));
    assert!(types.contains(&"tiff".to_string()));
    assert!(types.contains(&"mp4".to_string()));
    assert!(types.contains(&"rar".to_string()));
    assert!(types.contains(&"7z".to_string()));
}

#[test]
fn pipeline_reports_timing_metrics() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let input_path = temp_dir.path().join("image.bin");

    // Create a simple image with a JPEG to trigger carving
    let mut image = vec![0u8; 64_000];
    insert_bytes(&mut image, 1024, &sample_jpeg());

    fs::write(&input_path, &image).expect("write input");

    let loaded = config::load_config(None).expect("config");
    let mut cfg = loaded.config;
    cfg.run_id = "metrics_test".to_string();

    for ft in cfg.file_types.iter_mut() {
        if ft.id == "jpeg" {
            ft.min_size = 16;
        }
    }

    let evidence = RawFileSource::open(&input_path).expect("evidence");
    let evidence: Arc<dyn swiftbeaver::evidence::EvidenceSource> = Arc::new(evidence);

    let run_output_dir = temp_dir.path().join("run");
    fs::create_dir_all(&run_output_dir).expect("output dir");

    let meta_sink = metadata::build_sink(
        MetadataBackendKind::Jsonl,
        &cfg,
        &cfg.run_id,
        "test",
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
        2,
        2,
        64 * 1024,
        64,
        None,
        None,
        carve_registry,
    )
    .expect("pipeline");

    assert!(stats.scan_time_ms > 0, "scan_time_ms should be > 0");
    // carve_time_ms may be 0 for very fast carves (sub-millisecond on small files)
    // so we only check scan_time_ms is populated
}

#[test]
fn metadata_only_mode_records_metadata_without_writing_files() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let input_path = temp_dir.path().join("image.bin");

    let mut image = vec![0u8; 200_000];
    insert_bytes(&mut image, 1024, &sample_jpeg());
    insert_bytes(&mut image, 65_536, &sample_png());
    insert_bytes(&mut image, 131_072, &sample_gif());

    fs::write(&input_path, &image).expect("write input");

    let loaded = config::load_config(None).expect("config");
    let mut cfg = loaded.config;
    cfg.run_id = "test_metadata_only".to_string();

    for ft in cfg.file_types.iter_mut() {
        if ft.id == "jpeg" || ft.id == "gif" || ft.id == "png" {
            ft.min_size = 16;
        }
    }

    let evidence = RawFileSource::open(&input_path).expect("evidence");
    let evidence: Arc<dyn swiftbeaver::evidence::EvidenceSource> = Arc::new(evidence);

    let run_output_dir = temp_dir.path().join("run");
    fs::create_dir_all(&run_output_dir).expect("output dir");

    let meta_sink = metadata::build_sink(
        MetadataBackendKind::Jsonl,
        &cfg,
        &cfg.run_id,
        "test",
        &loaded.config_hash,
        &input_path,
        "",
        &run_output_dir,
    )
    .expect("metadata sink");

    let sig_scanner = scanner::build_signature_scanner(&cfg, false).expect("scanner");
    let sig_scanner: Arc<dyn swiftbeaver::scanner::SignatureScanner> = Arc::from(sig_scanner);

    let carve_registry = Arc::new(util::build_carve_registry(&cfg, false).expect("registry"));

    let cancel_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));

    pipeline::run_pipeline_with_cancel(
        &cfg,
        evidence,
        sig_scanner,
        None,
        vec![meta_sink],
        &run_output_dir,
        2,
        2,
        64 * 1024,
        64,
        None,
        None,
        carve_registry,
        cancel_flag,
        None,
        None,
        true, // metadata_only = true
    )
    .expect("pipeline");

    // Verify: no carved files on disk
    let carved_root = run_output_dir.join("carved");
    if carved_root.exists() {
        let mut file_count = 0usize;
        fn count_files(dir: &std::path::Path, count: &mut usize) {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        count_files(&path, count);
                    } else if path.is_file() {
                        *count += 1;
                    }
                }
            }
        }
        count_files(&carved_root, &mut file_count);
        assert_eq!(
            file_count, 0,
            "metadata-only mode should not write any carved files"
        );
    }

    // Verify: metadata records exist with hashes
    let meta_path = run_output_dir.join("metadata").join("carved_files.jsonl");
    let contents = fs::read_to_string(&meta_path).expect("metadata read");
    let lines: Vec<&str> = contents.lines().collect();
    assert!(
        lines.len() >= 2,
        "expected at least 2 metadata records, got {}",
        lines.len()
    );

    for line in &lines {
        let v: serde_json::Value = serde_json::from_str(line).expect("json");
        assert!(
            v.get("sha256").and_then(|v| v.as_str()).is_some(),
            "metadata record should have sha256 hash"
        );
        assert!(
            v.get("md5").and_then(|v| v.as_str()).is_some(),
            "metadata record should have md5 hash"
        );
        assert!(
            v.get("run_id").and_then(|v| v.as_str()).is_some(),
            "metadata record should have run_id"
        );
        assert!(
            v.get("tool_version").and_then(|v| v.as_str()).is_some(),
            "metadata record should have tool_version"
        );
        assert!(
            v.get("config_hash").and_then(|v| v.as_str()).is_some(),
            "metadata record should have config_hash"
        );
        assert!(
            v.get("evidence_path").and_then(|v| v.as_str()).is_some(),
            "metadata record should have evidence_path"
        );
    }
}

#[test]
fn overlap_skipped_is_reported_when_nested_hits_share_a_carved_range() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let input_path = temp_dir.path().join("image.bin");

    let mut image = vec![0u8; 8192];
    insert_bytes(&mut image, 1024, &sample_pdf_with_nested_header());
    fs::write(&input_path, &image).expect("write input");

    let loaded = config::load_config(None).expect("config");
    let mut cfg = loaded.config;
    cfg.run_id = "overlap_skipped_test".to_string();

    let evidence = RawFileSource::open(&input_path).expect("evidence");
    let evidence: Arc<dyn swiftbeaver::evidence::EvidenceSource> = Arc::new(evidence);

    let run_output_dir = temp_dir.path().join("run");
    fs::create_dir_all(&run_output_dir).expect("output dir");

    let meta_sink = metadata::build_sink(
        MetadataBackendKind::Jsonl,
        &cfg,
        &cfg.run_id,
        "test",
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
        64 * 1024,
        64,
        None,
        None,
        carve_registry,
    )
    .expect("pipeline");

    assert!(
        stats.files_carved > 0,
        "expected at least one carved file, got {}",
        stats.files_carved
    );
    assert!(
        stats.overlap_skipped > 0,
        "expected overlap_skipped > 0, got {}",
        stats.overlap_skipped
    );

    let run_summary = fs::read_to_string(run_output_dir.join("metadata").join("run_summary.jsonl"))
        .expect("run summary");
    let summary: serde_json::Value =
        serde_json::from_str(run_summary.lines().last().expect("summary line")).expect("json");
    assert_eq!(
        summary
            .get("overlap_skipped")
            .and_then(|value| value.as_u64())
            .expect("overlap_skipped"),
        stats.overlap_skipped
    );
}

/// Verify that the pipeline produces correct output when carve_workers != scan_workers
/// (asymmetric worker configuration from issue #48).
#[test]
fn asymmetric_worker_counts_produce_correct_output() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let input_path = temp_dir.path().join("input.bin");

    let mut data = vec![0u8; 4096];

    // Embed a JPEG
    let jpeg = sample_jpeg();
    insert_bytes(&mut data, 0, &jpeg);

    // Embed a PNG after the JPEG
    let png = sample_png();
    insert_bytes(&mut data, 512, &png);

    fs::write(&input_path, &data).expect("write input");

    let loaded = config::load_config(None).expect("config");
    let mut cfg = loaded.config.clone();
    cfg.run_id = "test_asymmetric_workers".to_string();

    let evidence = RawFileSource::open(&input_path).expect("evidence");
    let evidence: Arc<dyn swiftbeaver::evidence::EvidenceSource> = Arc::new(evidence);

    let run_output_dir = temp_dir.path().join("run");
    fs::create_dir_all(&run_output_dir).expect("output dir");

    let meta_sink = metadata::build_sink(
        MetadataBackendKind::Jsonl,
        &cfg,
        &cfg.run_id,
        "0.1.0",
        &loaded.config_hash,
        &input_path,
        "",
        &run_output_dir,
    )
    .expect("metadata sink");

    let sig_scanner = scanner::build_signature_scanner(&cfg, false).expect("scanner");
    let sig_scanner: Arc<dyn swiftbeaver::scanner::SignatureScanner> = Arc::from(sig_scanner);

    let carve_registry = Arc::new(util::build_carve_registry(&cfg, false).expect("registry"));

    // Asymmetric: 1 scan worker, 4 carve workers
    let stats = pipeline::run_pipeline(
        &cfg,
        evidence,
        sig_scanner,
        None,
        vec![meta_sink],
        &run_output_dir,
        1, // scan_workers
        4, // carve_workers (more than scan)
        64 * 1024,
        64,
        None,
        None,
        carve_registry,
    )
    .expect("pipeline with asymmetric workers");

    assert!(
        stats.hits_found >= 2,
        "expected at least 2 hits, got {}",
        stats.hits_found
    );
}

/// Verify that sharded metadata writing (3 Parquet sinks) produces
/// complete and correct output — all carved files appear in parquet.
#[test]
fn sharded_parquet_metadata_writes_all_events() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let input_path = temp_dir.path().join("input.bin");

    let mut data = vec![0u8; 200_000];
    insert_bytes(&mut data, 1024, &sample_jpeg());
    insert_bytes(&mut data, 65_536, &sample_png());
    insert_bytes(&mut data, 131_072, &sample_gif());

    fs::write(&input_path, &data).expect("write input");

    let loaded = config::load_config(None).expect("config");
    let mut cfg = loaded.config;
    cfg.run_id = "test_sharded_meta".to_string();

    for ft in cfg.file_types.iter_mut() {
        if ft.id == "jpeg" || ft.id == "gif" || ft.id == "png" {
            ft.min_size = 16;
        }
    }

    let evidence = RawFileSource::open(&input_path).expect("evidence");
    let evidence: Arc<dyn swiftbeaver::evidence::EvidenceSource> = Arc::new(evidence);

    let run_output_dir = temp_dir.path().join("run");
    fs::create_dir_all(&run_output_dir).expect("output dir");

    // Build 3 Parquet sinks for sharded metadata writing
    let mut meta_sinks: Vec<Box<dyn swiftbeaver::metadata::MetadataSink>> = Vec::with_capacity(3);
    for _ in 0..3 {
        meta_sinks.push(
            metadata::build_sink(
                MetadataBackendKind::Parquet,
                &cfg,
                &cfg.run_id,
                env!("CARGO_PKG_VERSION"),
                &loaded.config_hash,
                &input_path,
                "",
                &run_output_dir,
            )
            .expect("parquet sink"),
        );
    }

    let sig_scanner = scanner::build_signature_scanner(&cfg, false).expect("scanner");
    let sig_scanner: Arc<dyn swiftbeaver::scanner::SignatureScanner> = Arc::from(sig_scanner);
    let carve_registry = Arc::new(util::build_carve_registry(&cfg, false).expect("registry"));

    let stats = pipeline::run_pipeline(
        &cfg,
        evidence,
        sig_scanner,
        None,
        meta_sinks,
        &run_output_dir,
        2,
        2,
        64 * 1024,
        64,
        None,
        None,
        carve_registry,
    )
    .expect("sharded pipeline");

    assert!(
        stats.files_carved >= 3,
        "expected at least 3 carved files, got {}",
        stats.files_carved
    );

    // Verify parquet files were created by the file shard
    let parquet_dir = run_output_dir.join("parquet");
    assert!(
        parquet_dir.exists(),
        "parquet output directory should exist"
    );

    // At least one files_*.parquet should exist (from file shard)
    let has_file_parquet = fs::read_dir(&parquet_dir)
        .expect("read parquet dir")
        .filter_map(|e| e.ok())
        .any(|e| e.file_name().to_string_lossy().starts_with("files_"));
    assert!(
        has_file_parquet,
        "sharded file shard should produce files_*.parquet"
    );

    // Verify run_summary.parquet was written (also handled by file shard)
    assert!(
        parquet_dir.join("run_summary.parquet").exists(),
        "run_summary.parquet should exist"
    );
}

/// Verify that invalid sink counts are rejected.
#[test]
fn rejects_invalid_sink_counts() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let input_path = temp_dir.path().join("input.bin");
    fs::write(&input_path, [0u8; 1024]).expect("write input");

    let loaded = config::load_config(None).expect("config");
    let mut cfg = loaded.config;
    cfg.run_id = "test_invalid_sinks".to_string();

    let evidence = RawFileSource::open(&input_path).expect("evidence");
    let evidence: Arc<dyn swiftbeaver::evidence::EvidenceSource> = Arc::new(evidence);

    let run_output_dir = temp_dir.path().join("run");
    fs::create_dir_all(&run_output_dir).expect("output dir");

    // Build 2 sinks — should be rejected (only 1 or 3 accepted)
    let mut meta_sinks: Vec<Box<dyn swiftbeaver::metadata::MetadataSink>> = Vec::with_capacity(2);
    for _ in 0..2 {
        meta_sinks.push(
            metadata::build_sink(
                MetadataBackendKind::Parquet,
                &cfg,
                &cfg.run_id,
                env!("CARGO_PKG_VERSION"),
                &loaded.config_hash,
                &input_path,
                "",
                &run_output_dir,
            )
            .expect("parquet sink"),
        );
    }

    let sig_scanner = scanner::build_signature_scanner(&cfg, false).expect("scanner");
    let sig_scanner: Arc<dyn swiftbeaver::scanner::SignatureScanner> = Arc::from(sig_scanner);
    let carve_registry = Arc::new(util::build_carve_registry(&cfg, false).expect("registry"));

    let result = pipeline::run_pipeline(
        &cfg,
        evidence,
        sig_scanner,
        None,
        meta_sinks,
        &run_output_dir,
        1,
        1,
        64 * 1024,
        64,
        None,
        None,
        carve_registry,
    );

    assert!(
        result.is_err(),
        "pipeline should reject 2 sinks (only 1 or 3 valid)"
    );
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("expected 1 or 3"),
        "error should mention valid counts, got: {err}"
    );
}
