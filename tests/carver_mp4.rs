//! MP4/MOV carver tests against golden image.
//!
//! The MP4 carver handles the ISO Base Media File Format family:
//! - MP4 (MPEG-4 Part 14)
//! - MOV (QuickTime)
//! - M4A (MPEG-4 Audio)
//! - M4V (MPEG-4 Video)

mod common;

use common::{get_expected_files, run_carver_for_types, verify_carved_files};

#[test]
fn finds_all_mp4_files() {
    skip_without_golden_image!();
    let manifest = load_manifest_or_skip!();

    // MP4 carver handles multiple extensions in the same format family
    let expected = get_expected_files(&manifest, &["mp4", "mov", "m4a", "m4v"]);
    if expected.is_empty() {
        eprintln!("No MP4/MOV files in manifest");
        return;
    }

    let result = run_carver_for_types(&["mp4"]);
    let (matched, errors) = verify_carved_files(&result, &expected, "MP4");

    assert!(
        errors.is_empty(),
        "MP4 carver failed: {} errors, {} matched",
        errors.len(),
        matched
    );
    assert_eq!(
        matched,
        expected.len(),
        "MP4 carver should find all {} files",
        expected.len()
    );
}
