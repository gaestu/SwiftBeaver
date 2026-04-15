//! HEIC/HEIF carver tests against golden image.
//!
//! The HEIC carver handles high-efficiency image formats:
//! - HEIC (High Efficiency Image Container) - default on iOS 11+
//! - HEIF (High Efficiency Image Format)

mod common;

use common::{get_expected_files, run_carver_for_types, verify_carved_files};

#[test]
fn finds_all_heic_files() {
    skip_without_golden_image!();
    let manifest = load_manifest_or_skip!();

    // HEIC carver handles both .heic and .heif extensions
    let expected = get_expected_files(&manifest, &["heic", "heif", "hif"]);
    if expected.is_empty() {
        eprintln!("No HEIC/HEIF files in manifest");
        return;
    }

    let result = run_carver_for_types(&["heic"]);
    let (matched, errors) = verify_carved_files(&result, &expected, "HEIC");

    assert!(
        errors.is_empty(),
        "HEIC carver failed: {} errors, {} matched",
        errors.len(),
        matched
    );
    assert_eq!(
        matched,
        expected.len(),
        "HEIC carver should find all {} files",
        expected.len()
    );
}
