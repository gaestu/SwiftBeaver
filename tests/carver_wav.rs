//! WAV carver tests against golden image.

mod common;

use common::{get_expected_files, run_carver_for_types, verify_carved_files};

#[test]
fn finds_all_wav_files() {
    skip_without_golden_image!();
    let manifest = load_manifest_or_skip!();

    let expected = get_expected_files(&manifest, &["wav"]);
    if expected.is_empty() {
        eprintln!("No WAV files in manifest");
        return;
    }

    let result = run_carver_for_types(&["wav"]);
    let (matched, errors) = verify_carved_files(&result, &expected, "WAV");

    assert!(
        errors.is_empty(),
        "WAV carver failed: {} errors, {} matched",
        errors.len(),
        matched
    );
    assert_eq!(
        matched,
        expected.len(),
        "WAV carver should find all {} files",
        expected.len()
    );
}
