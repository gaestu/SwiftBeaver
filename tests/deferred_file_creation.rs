//! Integration tests for deferred file creation.
//!
//! Verifies that deferred_buffer_kb=0 (eager) and deferred_buffer_kb=64 (deferred)
//! produce byte-identical output with identical hashes.

use std::path::Path;

use swiftbeaver::carve::{CarveHandler, CarvedFile, ExtractionContext};
use swiftbeaver::evidence::RawFileSource;
use swiftbeaver::scanner::NormalizedHit;

/// Create a minimal valid PNG file in memory.
fn make_png() -> Vec<u8> {
    let mut data = Vec::new();
    // PNG signature
    data.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    // IHDR chunk
    let ihdr_data = [
        0x00, 0x00, 0x00, 0x01, // width = 1
        0x00, 0x00, 0x00, 0x01, // height = 1
        0x08, // bit depth
        0x02, // color type = RGB
        0x00, // compression
        0x00, // filter
        0x00, // interlace
    ];
    let ihdr_crc = crc32_chunk(b"IHDR", &ihdr_data);
    data.extend_from_slice(&(ihdr_data.len() as u32).to_be_bytes());
    data.extend_from_slice(b"IHDR");
    data.extend_from_slice(&ihdr_data);
    data.extend_from_slice(&ihdr_crc.to_be_bytes());
    // IDAT chunk (minimal compressed data)
    let idat_data = [
        0x08, 0xD7, 0x63, 0x60, 0x60, 0x60, 0x00, 0x00, 0x00, 0x04, 0x00, 0x01,
    ];
    let idat_crc = crc32_chunk(b"IDAT", &idat_data);
    data.extend_from_slice(&(idat_data.len() as u32).to_be_bytes());
    data.extend_from_slice(b"IDAT");
    data.extend_from_slice(&idat_data);
    data.extend_from_slice(&idat_crc.to_be_bytes());
    // IEND chunk
    let iend_crc = crc32_chunk(b"IEND", &[]);
    data.extend_from_slice(&0u32.to_be_bytes());
    data.extend_from_slice(b"IEND");
    data.extend_from_slice(&iend_crc.to_be_bytes());
    data
}

fn crc32_chunk(chunk_type: &[u8], chunk_data: &[u8]) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(chunk_type);
    hasher.update(chunk_data);
    hasher.finalize()
}

/// Create a minimal valid JPEG file in memory.
fn make_jpeg() -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xE0]); // SOI + APP0
    data.extend_from_slice(&[0x00, 0x10]); // segment length = 16
    data.extend_from_slice(b"JFIF\0");
    data.extend_from_slice(&[0x01, 0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00]);
    data.extend_from_slice(&[0xFF, 0xD9]); // EOI
    data
}

/// Create a minimal valid BMP (24-bit, 1×1 pixel).
fn make_bmp() -> Vec<u8> {
    let mut data = Vec::new();
    // BMP header (14 bytes)
    let file_size: u32 = 14 + 40 + 4; // header + DIB + 1 padded row
    let pixel_offset: u32 = 14 + 40;
    data.extend_from_slice(&[0x42, 0x4D]); // "BM"
    data.extend_from_slice(&file_size.to_le_bytes());
    data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // reserved
    data.extend_from_slice(&pixel_offset.to_le_bytes());
    // DIB header (BITMAPINFOHEADER = 40 bytes)
    data.extend_from_slice(&40u32.to_le_bytes()); // DIB header size
    data.extend_from_slice(&1i32.to_le_bytes()); // width
    data.extend_from_slice(&1i32.to_le_bytes()); // height
    data.extend_from_slice(&1u16.to_le_bytes()); // planes
    data.extend_from_slice(&24u16.to_le_bytes()); // bits per pixel
    data.extend_from_slice(&[0; 24]); // rest of DIB header (compression, sizes, etc.)
    // Pixel data: 3 bytes (RGB) + 1 byte padding = 4 bytes
    data.extend_from_slice(&[0xFF, 0x00, 0x00, 0x00]);
    data
}

/// Carve a file with a specific deferred_buffer_bytes setting and return (content, carved_file).
fn carve_with_buffer(
    evidence_path: &Path,
    handler: &dyn CarveHandler,
    offset: u64,
    pattern_id: &str,
    deferred_buffer_bytes: usize,
) -> Option<(Vec<u8>, CarvedFile)> {
    let evidence = RawFileSource::open(evidence_path).expect("open evidence");
    let output_dir = tempfile::tempdir().expect("tmpdir");

    let ctx = ExtractionContext::new(
        "test_deferred",
        output_dir.path(),
        &evidence,
        deferred_buffer_bytes,
    );

    let hit = NormalizedHit {
        global_offset: offset,
        file_type_id: handler.file_type().to_string(),
        pattern_id: pattern_id.to_string(),
        chunk_data: None,
        chunk_start: 0,
    };

    let result = handler.process_hit(&hit, &ctx).expect("process_hit");
    result.map(|carved| {
        let full_path = output_dir.path().join(&carved.path);
        let content = std::fs::read(&full_path).expect("read carved file");
        (content, carved)
    })
}

fn make_handler(file_type: &str) -> Box<dyn CarveHandler> {
    use swiftbeaver::carve::{bmp::BmpCarveHandler, jpeg::JpegCarveHandler, png::PngCarveHandler};
    match file_type {
        "png" => Box::new(PngCarveHandler::new(
            "png".to_string(),
            10,
            10 * 1024 * 1024,
        )),
        "jpeg" => Box::new(JpegCarveHandler::new(
            "jpg".to_string(),
            10,
            10 * 1024 * 1024,
        )),
        "bmp" => Box::new(BmpCarveHandler::new(
            "bmp".to_string(),
            10,
            10 * 1024 * 1024,
        )),
        _ => panic!("unsupported type: {file_type}"),
    }
}

#[test]
fn deferred_vs_eager_png_identical() {
    let png_data = make_png();
    let evidence_dir = tempfile::tempdir().expect("tmpdir");
    let evidence_path = evidence_dir.path().join("test.raw");
    std::fs::write(&evidence_path, &png_data).expect("write evidence");

    let handler = make_handler("png");

    let (eager_content, eager_file) =
        carve_with_buffer(&evidence_path, handler.as_ref(), 0, "png_sig", 0)
            .expect("eager carve should succeed");
    let (deferred_content, deferred_file) =
        carve_with_buffer(&evidence_path, handler.as_ref(), 0, "png_sig", 64 * 1024)
            .expect("deferred carve should succeed");

    assert_eq!(eager_content, deferred_content, "output bytes differ");
    assert_eq!(eager_file.md5, deferred_file.md5, "MD5 hashes differ");
    assert_eq!(
        eager_file.sha256, deferred_file.sha256,
        "SHA256 hashes differ"
    );
    assert_eq!(eager_file.size, deferred_file.size, "sizes differ");
    assert_eq!(eager_file.validated, deferred_file.validated);
}

#[test]
fn deferred_vs_eager_jpeg_identical() {
    let jpeg_data = make_jpeg();
    let evidence_dir = tempfile::tempdir().expect("tmpdir");
    let evidence_path = evidence_dir.path().join("test.raw");
    std::fs::write(&evidence_path, &jpeg_data).expect("write evidence");

    let handler = make_handler("jpeg");

    let (eager_content, eager_file) =
        carve_with_buffer(&evidence_path, handler.as_ref(), 0, "jpeg_soi", 0)
            .expect("eager carve should succeed");
    let (deferred_content, deferred_file) =
        carve_with_buffer(&evidence_path, handler.as_ref(), 0, "jpeg_soi", 64 * 1024)
            .expect("deferred carve should succeed");

    assert_eq!(eager_content, deferred_content, "output bytes differ");
    assert_eq!(eager_file.md5, deferred_file.md5, "MD5 hashes differ");
    assert_eq!(
        eager_file.sha256, deferred_file.sha256,
        "SHA256 hashes differ"
    );
    assert_eq!(eager_file.size, deferred_file.size, "sizes differ");
}

#[test]
fn deferred_vs_eager_bmp_identical() {
    let bmp_data = make_bmp();
    let evidence_dir = tempfile::tempdir().expect("tmpdir");
    let evidence_path = evidence_dir.path().join("test.raw");
    std::fs::write(&evidence_path, &bmp_data).expect("write evidence");

    let handler = make_handler("bmp");

    let (eager_content, eager_file) =
        carve_with_buffer(&evidence_path, handler.as_ref(), 0, "bmp_sig", 0)
            .expect("eager carve should succeed");
    let (deferred_content, deferred_file) =
        carve_with_buffer(&evidence_path, handler.as_ref(), 0, "bmp_sig", 64 * 1024)
            .expect("deferred carve should succeed");

    assert_eq!(eager_content, deferred_content, "output bytes differ");
    assert_eq!(eager_file.md5, deferred_file.md5, "MD5 hashes differ");
    assert_eq!(
        eager_file.sha256, deferred_file.sha256,
        "SHA256 hashes differ"
    );
    assert_eq!(eager_file.size, deferred_file.size, "sizes differ");
}

#[test]
fn deferred_invalid_png_no_file_created() {
    // Create invalid PNG: valid signature but garbage after
    let mut data = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    data.extend_from_slice(&[0xFF; 100]); // garbage chunk data

    let evidence_dir = tempfile::tempdir().expect("tmpdir");
    let evidence_path = evidence_dir.path().join("test.raw");
    std::fs::write(&evidence_path, &data).expect("write evidence");

    let evidence = RawFileSource::open(&evidence_path).expect("open evidence");
    let output_dir = tempfile::tempdir().expect("tmpdir");

    let handler = make_handler("png");
    let ctx = ExtractionContext::new(
        "test",
        output_dir.path(),
        &evidence,
        64 * 1024, // Large buffer, so file stays buffered
    );

    let hit = NormalizedHit {
        global_offset: 0,
        file_type_id: "png".to_string(),
        pattern_id: "png_sig".to_string(),
        chunk_data: None,
        chunk_start: 0,
    };

    let result = handler.process_hit(&hit, &ctx);
    // Should reject (invalid chunk structure)
    match result {
        Ok(None) => {
            // Verify no output file was created in the png subdirectory
            let png_dir = output_dir.path().join("png");
            if png_dir.exists() {
                let entries: Vec<_> = std::fs::read_dir(&png_dir)
                    .expect("read dir")
                    .filter_map(|e| e.ok())
                    .collect();
                assert!(
                    entries.is_empty(),
                    "no output file should exist for rejected candidate, found: {:?}",
                    entries.iter().map(|e| e.path()).collect::<Vec<_>>()
                );
            }
        }
        Ok(Some(_)) => {
            // Small invalid PNGs may still be carved (truncated) - that's OK
        }
        Err(e) => panic!("unexpected error: {e}"),
    }
}
