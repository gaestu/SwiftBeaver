//! SQLite carver tests against golden image.

mod common;

use common::{get_expected_files, run_carver_for_types, verify_carved_files};

#[test]
fn finds_all_sqlite_files() {
    skip_without_golden_image!();
    let manifest = load_manifest_or_skip!();

    let expected = get_expected_files(&manifest, &["sqlite", "db", "sqlite3"]);
    if expected.is_empty() {
        eprintln!("No SQLite files in manifest");
        return;
    }

    let result = run_carver_for_types(&["sqlite"]);
    let (matched, errors) = verify_carved_files(&result, &expected, "SQLite");

    assert!(
        errors.is_empty(),
        "SQLite carver failed: {} errors, {} matched",
        errors.len(),
        matched
    );
    assert_eq!(
        matched,
        expected.len(),
        "SQLite carver should find all {} files",
        expected.len()
    );
}
