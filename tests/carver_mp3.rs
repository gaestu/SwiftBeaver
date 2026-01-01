//! MP3 carver tests against golden image.
//!
//! Note: MP3 carving can produce false positives due to the sync word
//! pattern (0xFF 0xFB) appearing in other data.

mod common;

use common::{get_expected_files, run_carver_for_types, verify_carved_files};

#[test]
fn finds_all_mp3_files() {
    skip_without_golden_image!();
    let manifest = load_manifest_or_skip!();

    let expected = get_expected_files(&manifest, &["mp3"]);
    if expected.is_empty() {
        eprintln!("No MP3 files in manifest");
        return;
    }

    let result = run_carver_for_types(&["mp3"]);
    let (matched, errors) = verify_carved_files(&result, &expected, "MP3");

    assert!(
        errors.is_empty(),
        "MP3 carver failed: {} errors, {} matched",
        errors.len(),
        matched
    );
    assert_eq!(
        matched,
        expected.len(),
        "MP3 carver should find all {} files",
        expected.len()
    );
}
