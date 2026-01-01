//! TIFF carver tests against golden image.

mod common;

use common::{get_expected_files, run_carver_for_types, verify_carved_files};

#[test]
fn finds_all_tiff_files() {
    skip_without_golden_image!();
    let manifest = load_manifest_or_skip!();

    let expected = get_expected_files(&manifest, &["tiff", "tif"]);
    if expected.is_empty() {
        eprintln!("No TIFF files in manifest");
        return;
    }

    let result = run_carver_for_types(&["tiff"]);
    let (matched, errors) = verify_carved_files(&result, &expected, "TIFF");

    assert!(
        errors.is_empty(),
        "TIFF carver failed: {} errors, {} matched",
        errors.len(),
        matched
    );
    assert_eq!(
        matched,
        expected.len(),
        "TIFF carver should find all {} files",
        expected.len()
    );
}
