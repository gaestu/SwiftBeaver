use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::Deserialize;

#[cfg(feature = "ewf")]
use swiftbeaver::cli::{CliOptions, MetadataBackend};
use swiftbeaver::config;
use swiftbeaver::evidence::RawFileSource;
use swiftbeaver::metadata::{self, MetadataBackendKind};
use swiftbeaver::pipeline;
use swiftbeaver::scanner;
use swiftbeaver::util;

#[derive(Debug, Deserialize)]
struct Manifest {
    files: Vec<ManifestFile>,
    summary: ManifestSummary,
    raw_sha256: String,
}

#[derive(Debug, Deserialize)]
struct ManifestFile {
    path: String,
    category: String,
    extension: String,
    offset: u64,
    size: u64,
    sha256: String,
}

#[derive(Debug, Deserialize)]
struct ManifestSummary {
    total_files: u64,
    categories: HashMap<String, ManifestCategory>,
}

#[derive(Debug, Deserialize)]
struct ManifestCategory {
    files: u64,
    bytes: u64,
}

enum ManifestLoad {
    Missing,
    Error(String),
    Loaded(Manifest),
}

fn golden_image_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden_image")
}

fn golden_raw_path() -> PathBuf {
    golden_image_dir().join("golden.raw")
}

#[cfg(feature = "ewf")]
fn golden_e01_path() -> PathBuf {
    golden_image_dir().join("golden.E01")
}

fn load_manifest() -> ManifestLoad {
    let path = golden_image_dir().join("manifest.json");
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return ManifestLoad::Missing;
        }
        Err(err) => {
            return ManifestLoad::Error(format!("read manifest: {}", err));
        }
    };
    match serde_json::from_str(&content) {
        Ok(manifest) => ManifestLoad::Loaded(manifest),
        Err(err) => ManifestLoad::Error(format!("parse manifest: {}", err)),
    }
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(data))
}

fn derive_category(path: &str) -> &str {
    path.split('/').next().unwrap_or("")
}

fn derive_extension(path: &str) -> String {
    let filename = path.rsplit('/').next().unwrap_or(path);
    match filename.rsplit_once('.') {
        Some((_, ext)) => ext.to_ascii_lowercase(),
        None => String::new(),
    }
}

#[cfg(feature = "ewf")]
fn cli_opts_for_input(path: PathBuf) -> CliOptions {
    CliOptions {
        input: path,
        output: PathBuf::from("./output"),
        config_path: None,
        gpu: false,
        workers: 2,
        chunk_size_mib: 64,
        overlap_kib: None,
        metadata_backend: MetadataBackend::Jsonl,
        log_format: swiftbeaver::cli::LogFormat::Text,
        progress_interval_secs: 0,
        scan_strings: false,
        scan_utf16: false,
        scan_urls: false,
        no_scan_urls: false,
        scan_emails: false,
        no_scan_emails: false,
        scan_phones: false,
        no_scan_phones: false,
        scan_bitlocker_recovery: false,
        no_scan_bitlocker_recovery: false,
        string_min_len: None,
        scan_entropy: false,
        entropy_window_bytes: None,
        entropy_threshold: None,
        max_bytes: None,
        max_chunks: None,
        max_files: None,
        max_memory_mib: None,
        max_open_files: None,
        checkpoint_path: None,
        resume_from: None,
        evidence_sha256: None,
        compute_evidence_sha256: false,
        disable_zip: false,
        types: None,
        enable_types: None,
        dry_run: false,
        metadata_only: false,
        validate_carved: false,
        remove_invalid: false,
        hash_algorithms: None,
        dedupe: false,
        skip_duplicates: false,
        write_workers: None,
        scan_workers: None,
        carve_workers: None,
    }
}

fn golden_run_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn read_file_stable(path: &PathBuf, label: &str) -> String {
    let mut last_len = None;
    for _ in 0..5 {
        let content = fs::read_to_string(path).expect(label);
        let len = content.len();
        if Some(len) == last_len {
            return content;
        }
        last_len = Some(len);
        std::thread::sleep(Duration::from_millis(100));
    }
    fs::read_to_string(path).expect(label)
}

fn read_jsonl_values(path: &PathBuf, label: &str) -> Vec<serde_json::Value> {
    let content = read_file_stable(path, label);
    let mut out = Vec::new();
    for line in content.lines().filter(|line| !line.trim().is_empty()) {
        let value = serde_json::from_str::<serde_json::Value>(line)
            .unwrap_or_else(|err| panic!("{label}: {err}"));
        out.push(value);
    }
    out
}

#[test]
fn golden_carves_from_raw() {
    let _guard = golden_run_lock()
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    let raw_path = golden_raw_path();
    if !raw_path.exists() {
        eprintln!("Skipping: golden.raw not found. Run tests/golden_image/generate.sh");
        return;
    }

    let manifest = match load_manifest() {
        ManifestLoad::Loaded(m) => m,
        ManifestLoad::Missing => {
            if raw_path.exists() {
                panic!("manifest.json required when golden.raw exists");
            }
            eprintln!("Skipping: manifest.json not found.");
            return;
        }
        ManifestLoad::Error(err) => panic!("manifest.json error: {}", err),
    };

    let temp_dir = tempfile::tempdir().expect("tempdir");

    let loaded = config::load_config(None).expect("config");
    let mut cfg = loaded.config;
    cfg.run_id = "golden_raw_test".to_string();

    let evidence = RawFileSource::open(&raw_path).expect("open raw");
    let evidence: Arc<dyn swiftbeaver::evidence::EvidenceSource> = Arc::new(evidence);

    let run_output_dir = temp_dir.path().join(&cfg.run_id);
    fs::create_dir_all(&run_output_dir).expect("output dir");

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
        2,
        2,
        64 * 1024,
        4096,
        None,
        None,
        carve_registry,
    )
    .expect("pipeline");

    assert!(stats.hits_found > 0, "expected some hits");
    assert!(stats.files_carved > 0, "expected carved files");

    let manifest_hashes: HashSet<String> = manifest
        .files
        .iter()
        .map(|file| file.sha256.clone())
        .collect();

    let carved_meta = run_output_dir.join("metadata").join("carved_files.jsonl");
    let carved_content = read_file_stable(&carved_meta, "read carved metadata");
    let mut matched = 0usize;
    let deserializer = serde_json::Deserializer::from_str(&carved_content);
    for record in deserializer.into_iter::<serde_json::Value>() {
        match record {
            Ok(record) => {
                if let Some(hash) = record.get("sha256").and_then(|v| v.as_str())
                    && manifest_hashes.contains(hash)
                {
                    matched += 1;
                }
            }
            Err(err) => {
                if err.is_eof() {
                    eprintln!("Skipping truncated JSON record: {}", err);
                    break;
                }
                panic!("parse carved record: {}", err);
            }
        }
    }

    assert!(
        matched > 0,
        "expected carved outputs to match manifest samples"
    );

    // EVTX coverage: every EVTX fixture in the manifest must be recovered
    // from golden.raw. We compare against the declared logical SHA-256 (file
    // header + declared in-use chunks only) because raw signature carving
    // cannot prove that trailing bytes past `chunk_count` belong to the EVTX
    // file instead of adjacent evidence.
    let mut carved_hashes: HashSet<String> = HashSet::new();
    let deserializer = serde_json::Deserializer::from_str(&carved_content);
    for record in deserializer.into_iter::<serde_json::Value>() {
        let record = record.expect("carved_files.jsonl must be valid JSON");
        if let Some(hash) = record.get("sha256").and_then(|v| v.as_str()) {
            carved_hashes.insert(hash.to_string());
        }
    }
    let evtx_dir = std::path::Path::new("tests/golden_image/samples/windows/evtx");
    let mut missing_evtx: Vec<String> = Vec::new();
    let mut evtx_total = 0usize;
    for file in &manifest.files {
        if file.extension != "evtx" {
            continue;
        }
        evtx_total += 1;
        let fixture_path = evtx_dir.join(
            std::path::Path::new(&file.path)
                .file_name()
                .expect("evtx fixture filename"),
        );
        let bytes = fs::read(&fixture_path).unwrap_or_else(|err| {
            panic!(
                "failed to read EVTX fixture {} (manifest entry {}): {err}",
                fixture_path.display(),
                file.path,
            )
        });
        // Compute declared logical extent: 4096 + chunk_count * 65536.
        assert!(
            bytes.len() >= 0x2C,
            "EVTX fixture {} is undersized ({} bytes) — cannot read chunk_count",
            fixture_path.display(),
            bytes.len(),
        );
        let chunk_count = u16::from_le_bytes([bytes[0x2A], bytes[0x2B]]) as usize;
        let logical_size = 4096 + chunk_count * 65536;
        assert!(
            logical_size <= bytes.len(),
            "EVTX fixture {} is shorter ({} bytes) than its declared logical extent ({} bytes)",
            fixture_path.display(),
            bytes.len(),
            logical_size,
        );
        let logical = &bytes[..logical_size];
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(logical);
        let logical_sha = format!("{:x}", hasher.finalize());
        if !carved_hashes.contains(&logical_sha) {
            missing_evtx.push(file.path.clone());
        }
    }
    assert!(evtx_total > 0, "manifest should contain EVTX fixtures");
    assert!(
        missing_evtx.is_empty(),
        "EVTX fixtures missing from carved output (logical sha256 mismatch): {missing_evtx:?}"
    );

    let metadata_dir = run_output_dir.join("metadata");
    let history_path = metadata_dir.join("browser_history.jsonl");
    let cookies_path = metadata_dir.join("browser_cookies.jsonl");
    let downloads_path = metadata_dir.join("browser_downloads.jsonl");
    assert!(history_path.exists(), "missing browser_history.jsonl");
    assert!(cookies_path.exists(), "missing browser_cookies.jsonl");
    assert!(downloads_path.exists(), "missing browser_downloads.jsonl");

    let history = read_jsonl_values(&history_path, "read history metadata");
    let cookies = read_jsonl_values(&cookies_path, "read cookies metadata");
    let downloads = read_jsonl_values(&downloads_path, "read downloads metadata");
    assert!(
        history.is_empty(),
        "browser history parsing should be disabled"
    );
    assert!(
        cookies.is_empty(),
        "browser cookie parsing should be disabled"
    );
    assert!(
        downloads.is_empty(),
        "browser downloads parsing should be disabled"
    );
}

#[cfg(feature = "ewf")]
#[test]
fn golden_carves_from_e01_with_strings() {
    let _guard = golden_run_lock()
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    let e01_path = golden_e01_path();
    if !e01_path.exists() {
        eprintln!("Skipping: golden.E01 not found.");
        return;
    }

    let temp_dir = tempfile::tempdir().expect("tempdir");
    let loaded = config::load_config(None).expect("config");
    let mut cfg = loaded.config;
    cfg.run_id = "golden_e01_test".to_string();
    cfg.enable_string_scan = true;
    cfg.enable_url_scan = true;
    cfg.enable_email_scan = true;
    cfg.string_scan_utf16 = true;

    let opts = cli_opts_for_input(e01_path.clone());
    let evidence = swiftbeaver::evidence::open_source(&opts, 1).expect("open E01");
    let evidence: Arc<dyn swiftbeaver::evidence::EvidenceSource> = Arc::from(evidence);

    let run_output_dir = temp_dir.path().join(&cfg.run_id);
    fs::create_dir_all(&run_output_dir).expect("output dir");

    let meta_sink = metadata::build_sink(
        MetadataBackendKind::Jsonl,
        &cfg,
        &cfg.run_id,
        env!("CARGO_PKG_VERSION"),
        &loaded.config_hash,
        &e01_path,
        "",
        &run_output_dir,
    )
    .expect("metadata sink");

    let sig_scanner = scanner::build_signature_scanner(&cfg, false).expect("scanner");
    let sig_scanner: Arc<dyn swiftbeaver::scanner::SignatureScanner> = Arc::from(sig_scanner);

    let string_scanner = Some(Arc::from(
        swiftbeaver::strings::build_string_scanner(&cfg, false).expect("string scanner"),
    ));

    let carve_registry = Arc::new(util::build_carve_registry(&cfg, false).expect("registry"));

    let stats = pipeline::run_pipeline(
        &cfg,
        evidence,
        sig_scanner,
        string_scanner,
        vec![meta_sink],
        &run_output_dir,
        2,
        2,
        64 * 1024,
        4096,
        None,
        None,
        carve_registry,
    )
    .expect("pipeline");

    assert!(stats.files_carved > 0, "expected carved files from E01");
    assert!(stats.string_spans > 0, "expected string spans from E01");

    assert!(stats.artefacts_extracted > 0, "expected string artefacts");
}

#[cfg(feature = "ewf")]
#[test]
fn golden_e01_size_matches_raw() {
    let raw_path = golden_raw_path();
    let e01_path = golden_e01_path();

    if !raw_path.exists() || !e01_path.exists() {
        eprintln!("Skipping: need both golden.raw and golden.E01");
        return;
    }

    let raw_size = fs::metadata(&raw_path).expect("raw metadata").len();
    let opts = cli_opts_for_input(e01_path);
    let e01 = swiftbeaver::evidence::open_source(&opts, 1).expect("open E01");

    assert_eq!(e01.len(), raw_size, "E01 media size should match raw");
}

/// Verify that PooledEwfSource (multiple handles) returns byte-identical data
/// compared to a single-handle EwfSource at various offsets.
#[cfg(feature = "ewf")]
#[test]
fn golden_e01_pooled_reads_match_single() {
    let e01_path = golden_e01_path();
    if !e01_path.exists() {
        eprintln!("Skipping: golden.E01 not found.");
        return;
    }

    let opts = cli_opts_for_input(e01_path);

    // Open with 1 handle (single EwfSource path)
    let single = swiftbeaver::evidence::open_source(&opts, 1).expect("open single");
    // Open with 3 handles (PooledEwfSource path)
    let pooled = swiftbeaver::evidence::open_source(&opts, 3).expect("open pooled");

    assert_eq!(single.len(), pooled.len(), "media sizes must match");

    let total = single.len();
    // Test offsets: start, various positions, near end
    let offsets: Vec<u64> = vec![
        0,
        512,
        65536,
        1024 * 1024,
        total / 4,
        total / 2,
        total.saturating_sub(4096),
    ];

    for offset in offsets {
        if offset >= total {
            continue;
        }
        let read_len = 4096.min((total - offset) as usize);
        let mut buf_single = vec![0u8; read_len];
        let mut buf_pooled = vec![0u8; read_len];

        let n1 = single
            .read_at(offset, &mut buf_single)
            .expect("single read");
        let n2 = pooled
            .read_at(offset, &mut buf_pooled)
            .expect("pooled read");

        assert_eq!(n1, n2, "read sizes differ at offset {offset}");
        assert_eq!(
            buf_single[..n1],
            buf_pooled[..n2],
            "data mismatch at offset {offset}"
        );
    }
}

/// Verify that concurrent reads from PooledEwfSource are safe and return
/// consistent data (no corruption from handle contention).
#[cfg(feature = "ewf")]
#[test]
fn golden_e01_pooled_concurrent_reads() {
    let e01_path = golden_e01_path();
    if !e01_path.exists() {
        eprintln!("Skipping: golden.E01 not found.");
        return;
    }

    let opts = cli_opts_for_input(e01_path.clone());

    // Single-handle reference reads
    let single = swiftbeaver::evidence::open_source(&opts, 1).expect("open single");
    let total = single.len();

    let offsets: Vec<u64> = vec![0, 65536, 1024 * 1024, total / 2];
    let mut reference_data: Vec<(u64, Vec<u8>)> = Vec::new();
    for &offset in &offsets {
        if offset >= total {
            continue;
        }
        let read_len = 4096.min((total - offset) as usize);
        let mut buf = vec![0u8; read_len];
        let n = single.read_at(offset, &mut buf).expect("ref read");
        buf.truncate(n);
        reference_data.push((offset, buf));
    }
    drop(single);

    // Pooled source for concurrent reads
    let pooled: Arc<dyn swiftbeaver::evidence::EvidenceSource> =
        Arc::from(swiftbeaver::evidence::open_source(&opts, 3).expect("open pooled"));

    let mut handles = Vec::new();
    for (offset, expected) in &reference_data {
        let src = Arc::clone(&pooled);
        let offset = *offset;
        let expected = expected.clone();
        handles.push(std::thread::spawn(move || {
            for _ in 0..100 {
                let mut buf = vec![0u8; expected.len()];
                let n = src.read_at(offset, &mut buf).expect("concurrent read");
                assert_eq!(
                    &buf[..n],
                    &expected[..n],
                    "data mismatch at offset {offset} during concurrent read"
                );
            }
        }));
    }

    for h in handles {
        h.join().expect("thread panicked");
    }
}

#[test]
fn golden_manifest_integrity() {
    let raw_path = golden_raw_path();
    let manifest = match load_manifest() {
        ManifestLoad::Loaded(m) => m,
        ManifestLoad::Missing => {
            if raw_path.exists() {
                panic!("manifest.json required when golden.raw exists");
            }
            eprintln!("Skipping: manifest.json not found");
            return;
        }
        ManifestLoad::Error(err) => panic!("manifest.json error: {}", err),
    };

    if !raw_path.exists() {
        eprintln!("Skipping: golden.raw not found");
        return;
    }

    let raw_data = fs::read(&raw_path).expect("read raw");
    let mut verified = 0;
    let mut failed = Vec::new();

    for file in &manifest.files {
        let expected_category = derive_category(&file.path);
        let expected_extension = derive_extension(&file.path);
        assert_eq!(
            file.category, expected_category,
            "category mismatch for {}",
            file.path
        );
        assert_eq!(
            file.extension, expected_extension,
            "extension mismatch for {}",
            file.path
        );

        let offset = file.offset as usize;
        let size = file.size as usize;
        if offset + size > raw_data.len() {
            failed.push(format!("{}: extends beyond image", file.path));
            continue;
        }
        let slice = &raw_data[offset..offset + size];
        let actual_hash = sha256_hex(slice);
        if actual_hash == file.sha256 {
            verified += 1;
        } else {
            failed.push(format!("{}: hash mismatch", file.path));
        }
    }

    if !failed.is_empty() {
        for f in &failed {
            eprintln!("FAILED: {}", f);
        }
        panic!("{} files failed verification", failed.len());
    }

    assert_eq!(
        verified as u64, manifest.summary.total_files,
        "verified count should match manifest total"
    );
}

#[test]
fn golden_category_coverage() {
    let manifest = match load_manifest() {
        ManifestLoad::Loaded(m) => m,
        ManifestLoad::Missing => {
            if golden_raw_path().exists() {
                panic!("manifest.json required when golden.raw exists");
            }
            eprintln!("Skipping: manifest.json not found");
            return;
        }
        ManifestLoad::Error(err) => panic!("manifest.json error: {}", err),
    };

    for (cat, info) in &manifest.summary.categories {
        assert!(info.files > 0, "category '{}' should have files", cat);
        assert!(info.bytes > 0, "category '{}' should have bytes", cat);
    }

    assert!(
        !manifest.summary.categories.is_empty(),
        "expected categories in manifest summary"
    );
    assert!(
        !manifest.raw_sha256.is_empty(),
        "expected raw_sha256 in manifest"
    );
}
