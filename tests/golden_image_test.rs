use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;

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
        string_min_len: None,
        scan_entropy: false,
        entropy_window_bytes: None,
        entropy_threshold: None,
        scan_sqlite_pages: false,
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
    let carve_registry = Arc::new(util::build_carve_registry(&cfg).expect("registry"));

    let stats = pipeline::run_pipeline(
        &cfg,
        evidence,
        sig_scanner,
        None,
        meta_sink,
        &run_output_dir,
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
                if let Some(hash) = record.get("sha256").and_then(|v| v.as_str()) {
                    if manifest_hashes.contains(hash) {
                        matched += 1;
                    }
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
    let evidence = swiftbeaver::evidence::open_source(&opts).expect("open E01");
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

    let carve_registry = Arc::new(util::build_carve_registry(&cfg).expect("registry"));

    let stats = pipeline::run_pipeline(
        &cfg,
        evidence,
        sig_scanner,
        string_scanner,
        meta_sink,
        &run_output_dir,
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
    let e01 = swiftbeaver::evidence::open_source(&opts).expect("open E01");

    assert_eq!(e01.len(), raw_size, "E01 media size should match raw");
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
