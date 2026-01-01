//! OLE/CFB carver tests against golden image.
//!
//! OLE Compound File Binary format is used by:
//! - Microsoft Word 97-2003 (.doc)
//! - Microsoft Excel 97-2003 (.xls)
//! - Microsoft PowerPoint 97-2003 (.ppt)
//! - Microsoft Outlook (.msg)

mod common;

use common::{get_expected_files, run_carver_for_types, verify_carved_files};

#[test]
fn finds_all_ole_files() {
    skip_without_golden_image!();
    let manifest = load_manifest_or_skip!();

    // OLE carver handles legacy Office formats
    let expected = get_expected_files(&manifest, &["doc", "xls", "ppt", "msg"]);
    if expected.is_empty() {
        eprintln!("No OLE (DOC/XLS/PPT) files in manifest");
        return;
    }

    let result = run_carver_for_types(&["ole"]);
    let (matched, errors) = verify_carved_files(&result, &expected, "OLE");

    assert!(
        errors.is_empty(),
        "OLE carver failed: {} errors, {} matched",
        errors.len(),
        matched
    );
    assert_eq!(
        matched,
        expected.len(),
        "OLE carver should find all {} files",
        expected.len()
    );
}
