//! Shared test infrastructure for carver tests.
//!
//! This module provides utilities for testing individual carvers against
//! the golden image. Each carver has its own test file that imports this module.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;

use swiftbeaver::config;
use swiftbeaver::evidence::RawFileSource;
use swiftbeaver::metadata::{self, MetadataBackendKind};
use swiftbeaver::pipeline;
use swiftbeaver::scanner;
use swiftbeaver::util;

// ============================================================================
// Data Structures
// ============================================================================

#[derive(Debug, Deserialize, Clone)]
pub struct Manifest {
    pub files: Vec<ManifestFile>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ManifestFile {
    pub path: String,
    pub extension: String,
    pub offset: u64,
    pub size: u64,
    pub sha256: String,
}

/// Result of carving operation
#[derive(Debug)]
pub struct CarveResult {
    /// Files found, keyed by offset
    pub found: HashMap<u64, CarvedFileInfo>,
}

#[derive(Debug)]
pub struct CarvedFileInfo {
    pub size: u64,
    pub sha256: String,
}

// ============================================================================
// Path Helpers
// ============================================================================

pub fn golden_image_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden_image")
}

pub fn golden_raw_path() -> PathBuf {
    golden_image_dir().join("golden.raw")
}

pub fn load_manifest() -> Option<Manifest> {
    let path = golden_image_dir().join("manifest.json");
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Check if golden image is available
pub fn golden_image_available() -> bool {
    golden_raw_path().exists()
}

/// Check if manifest is available
#[allow(dead_code)]
pub fn manifest_available() -> bool {
    golden_image_dir().join("manifest.json").exists()
}

// ============================================================================
// Test Helpers
// ============================================================================

/// Get expected files from manifest filtered by extension(s)
pub fn get_expected_files(manifest: &Manifest, extensions: &[&str]) -> Vec<ManifestFile> {
    manifest
        .files
        .iter()
        .filter(|f| extensions.contains(&f.extension.as_str()))
        .cloned()
        .collect()
}

/// Run the carver pipeline with only specific file types enabled
pub fn run_carver_for_types(types: &[&str]) -> CarveResult {
    let raw_path = golden_raw_path();
    let temp_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tmp")
        .join("test_runs");
    fs::create_dir_all(&temp_root).expect("temp root");
    let temp_dir = tempfile::tempdir_in(&temp_root).expect("tempdir");

    let loaded = config::load_config(None).expect("config");
    let mut cfg = loaded.config;
    cfg.run_id = format!("carver_test_{}", types.join("_"));

    // Filter to only keep the file types we want to test
    cfg.file_types.retain(|ft| types.contains(&ft.id.as_str()));

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

    let _stats = pipeline::run_pipeline(
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

    // Parse carved files metadata
    let carved_meta = run_output_dir.join("metadata").join("carved_files.jsonl");
    let mut found = HashMap::new();

    if let Ok(content) = fs::read_to_string(&carved_meta) {
        for line in content.lines().filter(|l| !l.trim().is_empty()) {
            if let Ok(record) = serde_json::from_str::<serde_json::Value>(line) {
                // Use global_start as the offset (matches manifest)
                let offset = record
                    .get("global_start")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let size = record.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
                let sha256 = record
                    .get("sha256")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                found.insert(offset, CarvedFileInfo { size, sha256 });
            }
        }
    }

    CarveResult { found }
}

/// Verify that all expected files were carved correctly
/// Returns (matched_count, error_messages)
pub fn verify_carved_files(
    result: &CarveResult,
    expected: &[ManifestFile],
    carver_name: &str,
) -> (usize, Vec<String>) {
    let mut matched = 0;
    let mut errors = Vec::new();

    for file in expected {
        match result.found.get(&file.offset) {
            Some(carved) => {
                if carved.sha256 != file.sha256 {
                    errors.push(format!(
                        "{}: SHA256 mismatch at offset 0x{:X}\n  expected: {}\n  got:      {}",
                        file.path, file.offset, file.sha256, carved.sha256
                    ));
                } else if carved.size != file.size {
                    errors.push(format!(
                        "{}: size mismatch at offset 0x{:X}\n  expected: {}\n  got:      {}",
                        file.path, file.offset, file.size, carved.size
                    ));
                } else {
                    matched += 1;
                }
            }
            None => {
                errors.push(format!(
                    "{}: not found at offset 0x{:X}",
                    file.path, file.offset
                ));
            }
        }
    }

    if !errors.is_empty() {
        eprintln!("\n=== {} Carver Errors ===", carver_name);
        for err in &errors {
            eprintln!("  {}", err);
        }
    }

    (matched, errors)
}

// ============================================================================
// Macros for Test Boilerplate
// ============================================================================

/// Skip test if golden image not available
#[macro_export]
macro_rules! skip_without_golden_image {
    () => {
        if !common::golden_image_available() {
            eprintln!("Skipping: golden.raw not found. Run tests/golden_image/generate.sh");
            return;
        }
    };
}

/// Load manifest or skip test
#[macro_export]
macro_rules! load_manifest_or_skip {
    () => {
        match common::load_manifest() {
            Some(m) => m,
            None => {
                eprintln!("Skipping: manifest.json not found");
                return;
            }
        }
    };
}
