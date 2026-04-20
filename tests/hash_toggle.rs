//! Integration tests for hash algorithm toggle feature.
//!
//! Verifies that the HashConfig correctly controls which hash algorithms
//! are computed by CarveStream-based carvers.

use std::sync::Arc;

use swiftbeaver::carve::{CarveHandler, ExtractionContext};
use swiftbeaver::evidence::RawFileSource;
use swiftbeaver::hash::HashConfig;
use swiftbeaver::scanner::NormalizedHit;

/// Build a minimal valid GIF89a image for testing.
fn sample_gif() -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(b"GIF89a");
    data.extend_from_slice(&[0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00]);
    data.extend_from_slice(&[0x21, 0xF9, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00]);
    data.push(0x3B);
    data
}

/// Write evidence to a temp file and return the path.
fn write_evidence(data: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let evidence_path = dir.path().join("evidence.bin");
    std::fs::write(&evidence_path, data).expect("write evidence");
    (dir, evidence_path)
}

/// Carve a GIF using the given HashConfig and return the CarvedFile result.
fn carve_gif_with_config(hash_config: HashConfig) -> Option<swiftbeaver::carve::CarvedFile> {
    let gif_data = sample_gif();
    let (_evidence_dir, evidence_path) = write_evidence(&gif_data);
    let evidence = RawFileSource::open(&evidence_path).expect("open evidence");
    let output_dir = tempfile::tempdir().expect("output tmpdir");

    let mut ctx = ExtractionContext::new("hash_toggle_test", output_dir.path(), &evidence, 0);
    ctx.hash_config = hash_config;

    let handler = swiftbeaver::carve::gif::GifCarveHandler::new("gif".to_string(), 6, 1024 * 1024);
    let hit = NormalizedHit {
        global_offset: 0,
        file_type_id: "gif".to_string(),
        pattern_id: "gif89a".to_string(),
        chunk_data: Some(Arc::new(gif_data)),
        chunk_start: 0,
    };

    handler
        .process_hit(&hit, &ctx)
        .expect("process_hit")
        .map(|p| p.flush().expect("flush"))
}

#[test]
fn default_config_computes_both_hashes() {
    let config = HashConfig::default();
    let carved = carve_gif_with_config(config).expect("should carve file");
    assert!(carved.md5.is_some(), "md5 should be computed");
    assert!(carved.sha256.is_some(), "sha256 should be computed");
}

#[test]
fn md5_only_skips_sha256() {
    let config = HashConfig::from_names(&["md5".to_string()]);
    let carved = carve_gif_with_config(config).expect("should carve file");
    assert!(carved.md5.is_some(), "md5 should be computed");
    assert!(carved.sha256.is_none(), "sha256 should not be computed");
}

#[test]
fn sha256_only_skips_md5() {
    let config = HashConfig::from_names(&["sha256".to_string()]);
    let carved = carve_gif_with_config(config).expect("should carve file");
    assert!(carved.md5.is_none(), "md5 should not be computed");
    assert!(carved.sha256.is_some(), "sha256 should be computed");
}

#[test]
fn empty_config_skips_both_hashes() {
    let config = HashConfig::from_names(&[]);
    let carved = carve_gif_with_config(config).expect("should carve file");
    assert!(carved.md5.is_none(), "md5 should not be computed");
    assert!(carved.sha256.is_none(), "sha256 should not be computed");
}

#[test]
fn hash_config_from_names_is_case_insensitive() {
    let config = HashConfig::from_names(&["MD5".to_string(), "SHA256".to_string()]);
    assert!(config.has_md5());
    assert!(config.has_sha256());
}

#[test]
fn hash_config_ignores_unknown_algorithms() {
    let config = HashConfig::from_names(&[
        "sha256".to_string(),
        "sha1".to_string(),
        "blake3".to_string(),
    ]);
    assert!(!config.has_md5());
    assert!(config.has_sha256());
}

/// Build a minimal valid BMP image for testing.
fn sample_bmp() -> Vec<u8> {
    // Minimal 1x1 24-bit BMP (no palette)
    let mut data = Vec::new();
    // BMP header (14 bytes)
    data.extend_from_slice(b"BM");
    let file_size: u32 = 58; // 14 + 40 + 4 (one pixel row padded to 4 bytes)
    data.extend_from_slice(&file_size.to_le_bytes());
    data.extend_from_slice(&[0u8; 4]); // reserved
    let pixel_offset: u32 = 54; // 14 + 40
    data.extend_from_slice(&pixel_offset.to_le_bytes());
    // DIB header (BITMAPINFOHEADER, 40 bytes)
    data.extend_from_slice(&40u32.to_le_bytes()); // header size
    data.extend_from_slice(&1i32.to_le_bytes()); // width
    data.extend_from_slice(&1i32.to_le_bytes()); // height
    data.extend_from_slice(&1u16.to_le_bytes()); // color planes
    data.extend_from_slice(&24u16.to_le_bytes()); // bits per pixel
    data.extend_from_slice(&0u32.to_le_bytes()); // compression
    data.extend_from_slice(&4u32.to_le_bytes()); // image size (padded row)
    data.extend_from_slice(&2835u32.to_le_bytes()); // h resolution
    data.extend_from_slice(&2835u32.to_le_bytes()); // v resolution
    data.extend_from_slice(&0u32.to_le_bytes()); // colors in palette
    data.extend_from_slice(&0u32.to_le_bytes()); // important colors
    // Pixel data: 3 bytes + 1 padding byte = 4 bytes
    data.extend_from_slice(&[0xFF, 0x00, 0x00, 0x00]); // blue pixel + padding
    data
}

/// Carve a BMP using the given HashConfig and return the CarvedFile result.
fn carve_bmp_with_config(hash_config: HashConfig) -> Option<swiftbeaver::carve::CarvedFile> {
    let bmp_data = sample_bmp();
    let (_evidence_dir, evidence_path) = write_evidence(&bmp_data);
    let evidence = RawFileSource::open(&evidence_path).expect("open evidence");
    let output_dir = tempfile::tempdir().expect("output tmpdir");

    let mut ctx = ExtractionContext::new("hash_toggle_bmp_test", output_dir.path(), &evidence, 0);
    ctx.hash_config = hash_config;

    let handler = swiftbeaver::carve::bmp::BmpCarveHandler::new("bmp".to_string(), 0, 1024 * 1024);
    let hit = NormalizedHit {
        global_offset: 0,
        file_type_id: "bmp".to_string(),
        pattern_id: "bmp_header".to_string(),
        chunk_data: Some(Arc::new(bmp_data)),
        chunk_start: 0,
    };

    handler
        .process_hit(&hit, &ctx)
        .expect("process_hit")
        .map(|p| p.flush().expect("flush"))
}

#[test]
fn bmp_default_config_computes_both_hashes() {
    let config = HashConfig::default();
    let carved = carve_bmp_with_config(config).expect("should carve file");
    assert!(carved.md5.is_some(), "md5 should be computed");
    assert!(carved.sha256.is_some(), "sha256 should be computed");
}

#[test]
fn bmp_md5_only_skips_sha256() {
    let config = HashConfig::from_names(&["md5".to_string()]);
    let carved = carve_bmp_with_config(config).expect("should carve file");
    assert!(carved.md5.is_some(), "md5 should be computed");
    assert!(carved.sha256.is_none(), "sha256 should not be computed");
}

#[test]
fn bmp_sha256_only_skips_md5() {
    let config = HashConfig::from_names(&["sha256".to_string()]);
    let carved = carve_bmp_with_config(config).expect("should carve file");
    assert!(carved.md5.is_none(), "md5 should not be computed");
    assert!(carved.sha256.is_some(), "sha256 should be computed");
}

#[test]
fn bmp_empty_config_skips_both_hashes() {
    let config = HashConfig::from_names(&[]);
    let carved = carve_bmp_with_config(config).expect("should carve file");
    assert!(carved.md5.is_none(), "md5 should not be computed");
    assert!(carved.sha256.is_none(), "sha256 should not be computed");
}
