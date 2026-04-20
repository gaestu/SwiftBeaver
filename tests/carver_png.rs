//! PNG carver tests against golden image.

mod common;

use common::{get_expected_files, run_carver_for_types, verify_carved_files};

#[test]
fn finds_all_png_files() {
    skip_without_golden_image!();
    let manifest = load_manifest_or_skip!();

    let expected = get_expected_files(&manifest, &["png"]);
    if expected.is_empty() {
        eprintln!("No PNG files in manifest");
        return;
    }

    let result = run_carver_for_types(&["png"]);
    let (matched, errors) = verify_carved_files(&result, &expected, "PNG");

    assert!(
        errors.is_empty(),
        "PNG carver failed: {} errors, {} matched",
        errors.len(),
        matched
    );
    assert_eq!(
        matched,
        expected.len(),
        "PNG carver should find all {} files",
        expected.len()
    );
}

// Unit tests for PNG validation logic
#[cfg(test)]
mod png_validation_tests {
    use std::io::Write;
    use std::sync::Arc;
    use swiftbeaver::carve::png::PngCarveHandler;
    use swiftbeaver::carve::{CarveHandler, PreValidation};
    use swiftbeaver::evidence::RawFileSource;
    use tempfile::NamedTempFile;

    /// Helper: write bytes to a temp file, open as RawFileSource
    fn evidence_from_bytes(data: &[u8]) -> (NamedTempFile, Arc<RawFileSource>) {
        let mut tmp = NamedTempFile::new().expect("create tempfile");
        tmp.write_all(data).expect("write tempfile");
        tmp.flush().expect("flush");
        let source = RawFileSource::open(tmp.path()).expect("open evidence");
        (tmp, Arc::new(source))
    }

    fn handler() -> PngCarveHandler {
        PngCarveHandler::new("png".to_string(), 500, 104_857_600)
    }

    /// Build a minimal valid PNG header (33 bytes: sig + IHDR chunk start)
    fn valid_png_header() -> Vec<u8> {
        let mut buf = Vec::new();
        // PNG signature
        buf.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        // IHDR length: 13
        buf.extend_from_slice(&13u32.to_be_bytes());
        // IHDR chunk type
        buf.extend_from_slice(b"IHDR");
        // Width: 100
        buf.extend_from_slice(&100u32.to_be_bytes());
        // Height: 100
        buf.extend_from_slice(&100u32.to_be_bytes());
        // Bit depth: 8
        buf.push(8);
        // Color type: 2 (truecolor)
        buf.push(2);
        // Compression, filter, interlace
        buf.push(0);
        buf.push(0);
        buf.push(0);
        buf
    }

    #[test]
    fn test_pre_validate_accepts_valid_png() {
        let data = valid_png_header();
        let (_tmp, source) = evidence_from_bytes(&data);
        let h = handler();
        let result = h.pre_validate(source.as_ref(), 0).unwrap();
        assert_eq!(result, PreValidation::Proceed);
    }

    #[test]
    fn test_pre_validate_rejects_truncated_header() {
        // Only 10 bytes — not enough for sig + IHDR
        let data = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00];
        let (_tmp, source) = evidence_from_bytes(&data);
        let h = handler();
        let result = h.pre_validate(source.as_ref(), 0).unwrap();
        assert!(matches!(result, PreValidation::Reject(_)));
    }

    #[test]
    fn test_pre_validate_rejects_bad_signature() {
        let mut data = valid_png_header();
        data[0] = 0x00; // corrupt signature
        let (_tmp, source) = evidence_from_bytes(&data);
        let h = handler();
        let result = h.pre_validate(source.as_ref(), 0).unwrap();
        assert!(matches!(result, PreValidation::Reject(_)));
    }

    #[test]
    fn test_pre_validate_rejects_missing_ihdr() {
        let mut data = valid_png_header();
        // Replace "IHDR" with "IDAT"
        data[12] = b'I';
        data[13] = b'D';
        data[14] = b'A';
        data[15] = b'T';
        let (_tmp, source) = evidence_from_bytes(&data);
        let h = handler();
        let result = h.pre_validate(source.as_ref(), 0).unwrap();
        assert!(matches!(result, PreValidation::Reject(_)));
    }

    #[test]
    fn test_pre_validate_rejects_wrong_ihdr_length() {
        let mut data = valid_png_header();
        // Set IHDR length to 10 instead of 13
        data[8..12].copy_from_slice(&10u32.to_be_bytes());
        let (_tmp, source) = evidence_from_bytes(&data);
        let h = handler();
        let result = h.pre_validate(source.as_ref(), 0).unwrap();
        assert!(matches!(result, PreValidation::Reject(_)));
    }

    #[test]
    fn test_pre_validate_rejects_zero_width() {
        let mut data = valid_png_header();
        // Set width to 0
        data[16..20].copy_from_slice(&0u32.to_be_bytes());
        let (_tmp, source) = evidence_from_bytes(&data);
        let h = handler();
        let result = h.pre_validate(source.as_ref(), 0).unwrap();
        assert!(matches!(result, PreValidation::Reject(_)));
    }

    #[test]
    fn test_pre_validate_rejects_zero_height() {
        let mut data = valid_png_header();
        // Set height to 0
        data[20..24].copy_from_slice(&0u32.to_be_bytes());
        let (_tmp, source) = evidence_from_bytes(&data);
        let h = handler();
        let result = h.pre_validate(source.as_ref(), 0).unwrap();
        assert!(matches!(result, PreValidation::Reject(_)));
    }

    #[test]
    fn test_pre_validate_rejects_invalid_color_type() {
        let mut data = valid_png_header();
        // Color type 5 is invalid
        data[25] = 5;
        let (_tmp, source) = evidence_from_bytes(&data);
        let h = handler();
        let result = h.pre_validate(source.as_ref(), 0).unwrap();
        assert!(matches!(result, PreValidation::Reject(_)));
    }

    #[test]
    fn test_pre_validate_rejects_invalid_bit_depth_for_color_type() {
        let mut data = valid_png_header();
        // Color type 2 (truecolor) only allows bit depth 8 or 16
        data[24] = 4; // bit depth 4 invalid for truecolor
        data[25] = 2; // color type 2
        let (_tmp, source) = evidence_from_bytes(&data);
        let h = handler();
        let result = h.pre_validate(source.as_ref(), 0).unwrap();
        assert!(matches!(result, PreValidation::Reject(_)));
    }

    #[test]
    fn test_pre_validate_at_nonzero_offset() {
        let mut data = vec![0u8; 100]; // padding
        let header = valid_png_header();
        data.extend_from_slice(&header);
        let (_tmp, source) = evidence_from_bytes(&data);
        let h = handler();
        let result = h.pre_validate(source.as_ref(), 100).unwrap();
        assert_eq!(result, PreValidation::Proceed);
    }
}
