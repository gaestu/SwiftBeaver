//! ZIP carver tests against golden image.
//!
//! The ZIP carver handles:
//! - ZIP archives (.zip)
//! - Office Open XML formats (.docx, .xlsx, .pptx, .potx)
//! - OpenDocument formats (.odt, .ods, .odp)
//! - EPUB ebooks (.epub)
//! - Java archives (.jar)

mod common;

use common::{get_expected_files, run_carver_for_types, verify_carved_files};

#[test]
fn finds_all_zip_files() {
    skip_without_golden_image!();
    let manifest = load_manifest_or_skip!();

    // ZIP carver handles ZIP and all ZIP-based formats
    let expected = get_expected_files(
        &manifest,
        &[
            "zip", "docx", "xlsx", "pptx", "potx", "odt", "ods", "odp", "epub", "jar",
        ],
    );
    if expected.is_empty() {
        eprintln!("No ZIP-based files in manifest");
        return;
    }

    let result = run_carver_for_types(&["zip"]);
    let (matched, errors) = verify_carved_files(&result, &expected, "ZIP");

    assert!(
        errors.is_empty(),
        "ZIP carver failed: {} errors, {} matched",
        errors.len(),
        matched
    );
    assert_eq!(
        matched,
        expected.len(),
        "ZIP carver should find all {} files",
        expected.len()
    );
}
