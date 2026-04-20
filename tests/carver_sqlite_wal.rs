//! SQLite WAL carver tests against golden image.

mod common;

use common::{get_expected_files, run_carver_for_types, verify_carved_files};

#[test]
fn finds_all_sqlite_wal_files() {
    skip_without_golden_image!();
    let manifest = load_manifest_or_skip!();

    let expected = get_expected_files(&manifest, &["sqlite-wal"]);
    if expected.is_empty() {
        eprintln!("No sqlite-wal files in manifest");
        return;
    }

    let result = run_carver_for_types(&["sqlite_wal"]);
    let (matched, errors) = verify_carved_files(&result, &expected, "SQLite WAL");

    assert!(
        errors.is_empty(),
        "SQLite WAL carver failed: {} errors, {} matched",
        errors.len(),
        matched
    );
    assert_eq!(
        matched,
        expected.len(),
        "SQLite WAL carver should find all {} files",
        expected.len()
    );
}
