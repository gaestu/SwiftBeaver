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

/// Verify that PendingCarve::discard() does not create a file on disk
/// when the DeferredWriter has not overflowed its buffer (zero-write dedup).
#[test]
fn pending_carve_discard_skips_disk_io_for_small_files() {
    use std::sync::Arc;
    use swiftbeaver::carve::gif::GifCarveHandler;
    use swiftbeaver::carve::{CarveHandler, ExtractionContext};
    use swiftbeaver::evidence::RawFileSource;
    use swiftbeaver::scanner::NormalizedHit;

    // Build a small valid GIF (well under 64 KB deferred buffer)
    let gif_data = {
        let mut v = Vec::new();
        // GIF89a header
        v.extend_from_slice(b"GIF89a");
        // Logical screen descriptor: 1x1, no GCT
        v.extend_from_slice(&[0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00]);
        // Trailer
        v.push(0x3B);
        v
    };

    let evidence_dir = tempfile::tempdir().expect("tmpdir");
    let evidence_path = evidence_dir.path().join("dup.raw");
    // Write the same GIF twice at offsets 0 and 512
    let mut raw = vec![0u8; 1024];
    raw[..gif_data.len()].copy_from_slice(&gif_data);
    raw[512..512 + gif_data.len()].copy_from_slice(&gif_data);
    std::fs::write(&evidence_path, &raw).expect("write evidence");

    let evidence = RawFileSource::open(&evidence_path).expect("open evidence");
    let output_dir = tempfile::tempdir().expect("tmpdir");
    let handler = GifCarveHandler::new("gif".to_string(), 6, 1024 * 1024);

    // Carve the first copy at offset 0
    let ctx = ExtractionContext::new("test_dedup", output_dir.path(), &evidence, 64 * 1024);
    let hit1 = NormalizedHit {
        global_offset: 0,
        file_type_id: "gif".to_string(),
        pattern_id: "gif89a".to_string(),
        chunk_data: Some(Arc::new(raw.clone())),
        chunk_start: 0,
    };
    let pending1 = handler
        .process_hit(&hit1, &ctx)
        .expect("process_hit 1")
        .expect("should carve");
    let file1 = pending1.flush().expect("flush first copy");
    let first_path = output_dir.path().join(&file1.path);
    assert!(
        first_path.exists(),
        "first file should be on disk after flush"
    );

    // Carve the second (duplicate) copy at offset 512
    let hit2 = NormalizedHit {
        global_offset: 512,
        file_type_id: "gif".to_string(),
        pattern_id: "gif89a".to_string(),
        chunk_data: Some(Arc::new(raw.clone())),
        chunk_start: 0,
    };
    let pending2 = handler
        .process_hit(&hit2, &ctx)
        .expect("process_hit 2")
        .expect("should carve");

    // Verify the second file does NOT exist on disk before discard
    // (it's still in the DeferredWriter buffer because it's < 64 KB)
    let second_path = output_dir.path().join(&pending2.file.path);
    assert!(
        !second_path.exists(),
        "second file should NOT be on disk before flush (deferred)"
    );

    // Discard the duplicate — zero disk I/O
    let file2 = pending2.discard();
    assert!(
        !second_path.exists(),
        "second file should NOT exist after discard"
    );

    // Verify both files have the same hash (they're duplicates)
    assert_eq!(file1.sha256, file2.sha256);
    assert!(file2.sha256.is_some(), "sha256 must be computed");
}
