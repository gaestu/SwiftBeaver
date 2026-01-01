//! 7-Zip carver tests against golden image.

mod common;

use common::{get_expected_files, run_carver_for_types, verify_carved_files};

#[test]
fn finds_all_7z_files() {
    skip_without_golden_image!();
    let manifest = load_manifest_or_skip!();

    let expected = get_expected_files(&manifest, &["7z"]);
    if expected.is_empty() {
        eprintln!("No 7z files in manifest");
        return;
    }

    let result = run_carver_for_types(&["7z"]);
    let (matched, errors) = verify_carved_files(&result, &expected, "7z");

    assert!(
        errors.is_empty(),
        "7z carver failed: {} errors, {} matched",
        errors.len(),
        matched
    );
    assert_eq!(
        matched,
        expected.len(),
        "7z carver should find all {} files",
        expected.len()
    );
}
