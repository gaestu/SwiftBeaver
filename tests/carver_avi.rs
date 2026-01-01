//! AVI carver tests against golden image.

mod common;

use common::{get_expected_files, run_carver_for_types, verify_carved_files};

#[test]
fn finds_all_avi_files() {
    skip_without_golden_image!();
    let manifest = load_manifest_or_skip!();

    let expected = get_expected_files(&manifest, &["avi"]);
    if expected.is_empty() {
        eprintln!("No AVI files in manifest");
        return;
    }

    let result = run_carver_for_types(&["avi"]);
    let (matched, errors) = verify_carved_files(&result, &expected, "AVI");

    assert!(
        errors.is_empty(),
        "AVI carver failed: {} errors, {} matched",
        errors.len(),
        matched
    );
    assert_eq!(
        matched,
        expected.len(),
        "AVI carver should find all {} files",
        expected.len()
    );
}
