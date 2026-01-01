//! JPEG carver tests against golden image.

mod common;

use common::{get_expected_files, run_carver_for_types, verify_carved_files};

#[test]
fn finds_all_jpeg_files() {
    skip_without_golden_image!();
    let manifest = load_manifest_or_skip!();

    let expected = get_expected_files(&manifest, &["jpg", "jpeg"]);
    if expected.is_empty() {
        eprintln!("No JPEG files in manifest");
        return;
    }

    let result = run_carver_for_types(&["jpeg"]);
    let (matched, errors) = verify_carved_files(&result, &expected, "JPEG");

    assert!(
        errors.is_empty(),
        "JPEG carver failed: {} errors, {} matched",
        errors.len(),
        matched
    );
    assert_eq!(
        matched,
        expected.len(),
        "JPEG carver should find all {} files",
        expected.len()
    );
}

#[test]
fn jpeg_files_have_correct_sizes() {
    skip_without_golden_image!();
    let manifest = load_manifest_or_skip!();

    let expected = get_expected_files(&manifest, &["jpg", "jpeg"]);
    if expected.is_empty() {
        return;
    }

    let result = run_carver_for_types(&["jpeg"]);

    for file in &expected {
        if let Some(carved) = result.found.get(&file.offset) {
            assert_eq!(
                carved.size, file.size,
                "JPEG at 0x{:X} should be {} bytes, got {}",
                file.offset, file.size, carved.size
            );
        }
    }
}
