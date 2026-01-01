//! RAR carver tests against golden image.

mod common;

use common::{get_expected_files, run_carver_for_types, verify_carved_files};

#[test]
fn finds_all_rar_files() {
    skip_without_golden_image!();
    let manifest = load_manifest_or_skip!();

    let expected = get_expected_files(&manifest, &["rar"]);
    if expected.is_empty() {
        eprintln!("No RAR files in manifest");
        return;
    }

    let result = run_carver_for_types(&["rar"]);
    let (matched, errors) = verify_carved_files(&result, &expected, "RAR");

    assert!(
        errors.is_empty(),
        "RAR carver failed: {} errors, {} matched",
        errors.len(),
        matched
    );
    assert_eq!(
        matched,
        expected.len(),
        "RAR carver should find all {} files",
        expected.len()
    );
}
