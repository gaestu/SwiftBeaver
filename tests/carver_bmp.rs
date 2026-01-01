//! BMP carver tests against golden image.

mod common;

use common::{get_expected_files, run_carver_for_types, verify_carved_files};

#[test]
fn finds_all_bmp_files() {
    skip_without_golden_image!();
    let manifest = load_manifest_or_skip!();

    let expected = get_expected_files(&manifest, &["bmp"]);
    if expected.is_empty() {
        eprintln!("No BMP files in manifest");
        return;
    }

    let result = run_carver_for_types(&["bmp"]);
    let (matched, errors) = verify_carved_files(&result, &expected, "BMP");

    assert!(
        errors.is_empty(),
        "BMP carver failed: {} errors, {} matched",
        errors.len(),
        matched
    );
    assert_eq!(
        matched,
        expected.len(),
        "BMP carver should find all {} files",
        expected.len()
    );
}
