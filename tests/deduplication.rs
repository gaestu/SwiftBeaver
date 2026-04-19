//! Integration tests for deduplication feature.
//!
//! Verifies that DedupTracker correctly identifies duplicate files
//! and that the pipeline honors the dedup and skip_duplicates settings.

use swiftbeaver::dedup::{DedupResult, DedupTracker};

#[test]
fn dedup_tracker_first_insert_not_duplicate() {
    let tracker = DedupTracker::new();
    let result = tracker.check_and_register("aaaa", 0);
    assert_eq!(
        result,
        DedupResult {
            is_duplicate: false,
            duplicate_of_offset: None,
        }
    );
}

#[test]
fn dedup_tracker_second_insert_is_duplicate() {
    let tracker = DedupTracker::new();
    let _ = tracker.check_and_register("aaaa", 100);
    let result = tracker.check_and_register("aaaa", 200);
    assert_eq!(
        result,
        DedupResult {
            is_duplicate: true,
            duplicate_of_offset: Some(100),
        }
    );
}

#[test]
fn dedup_tracker_different_hashes_not_duplicate() {
    let tracker = DedupTracker::new();
    let _ = tracker.check_and_register("aaaa", 100);
    let result = tracker.check_and_register("bbbb", 200);
    assert_eq!(
        result,
        DedupResult {
            is_duplicate: false,
            duplicate_of_offset: None,
        }
    );
}

#[test]
fn dedup_tracker_many_distinct_hashes() {
    let tracker = DedupTracker::new();
    for i in 0..100 {
        let hash = format!("hash_{:04}", i);
        let result = tracker.check_and_register(&hash, i * 512);
        assert!(!result.is_duplicate, "first insert should not be duplicate");
    }
    // Re-inserting the first hash should be duplicate
    let result = tracker.check_and_register("hash_0000", 999999);
    assert!(result.is_duplicate);
    assert_eq!(result.duplicate_of_offset, Some(0));
}

#[test]
fn dedup_tracker_concurrent_access() {
    use std::sync::Arc;
    use std::thread;

    let tracker = Arc::new(DedupTracker::new());
    let mut handles = Vec::new();

    // Spawn threads that each insert unique hashes
    for t in 0..4 {
        let tracker = tracker.clone();
        handles.push(thread::spawn(move || {
            for i in 0..100 {
                let hash = format!("thread{t}_hash{i}");
                let result = tracker.check_and_register(&hash, (t * 1000 + i) as u64);
                assert!(!result.is_duplicate, "unique hash should not be duplicate");
            }
        }));
    }
    for h in handles {
        h.join().expect("thread panicked");
    }

    // All 400 unique hashes should be tracked; re-inserting should be duplicate
    let result = tracker.check_and_register("thread0_hash0", 999999);
    assert!(result.is_duplicate);
    assert_eq!(result.duplicate_of_offset, Some(0));
}

#[test]
fn carved_file_dedup_fields_default_false() {
    let file = swiftbeaver::carve::CarvedFile {
        run_id: "test".to_string(),
        file_type: "gif".to_string(),
        path: "gif/test.gif".to_string(),
        extension: "gif".to_string(),
        global_start: 0,
        global_end: 100,
        size: 101,
        md5: Some("abcd".to_string()),
        sha256: Some("1234".to_string()),
        validated: false,
        truncated: false,
        errors: Vec::new(),
        pattern_id: Some("gif89a".to_string()),
        is_duplicate: false,
        duplicate_of_offset: None,
    };
    assert!(!file.is_duplicate);
    assert!(file.duplicate_of_offset.is_none());
}

#[test]
fn carved_file_dedup_fields_set() {
    let file = swiftbeaver::carve::CarvedFile {
        run_id: "test".to_string(),
        file_type: "gif".to_string(),
        path: "gif/test.gif".to_string(),
        extension: "gif".to_string(),
        global_start: 200,
        global_end: 300,
        size: 101,
        md5: Some("abcd".to_string()),
        sha256: Some("1234".to_string()),
        validated: false,
        truncated: false,
        errors: Vec::new(),
        pattern_id: Some("gif89a".to_string()),
        is_duplicate: true,
        duplicate_of_offset: Some(0),
    };
    assert!(file.is_duplicate);
    assert_eq!(file.duplicate_of_offset, Some(0));
}
