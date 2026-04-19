/// Deduplication tracker based on SHA256 hashes.
///
/// This tracker is **thread-safe** and designed to be shared across carve
/// workers via `Arc<DedupTracker>`. It stores the first-encountered global
/// offset for each unique SHA256. Later arrivals with the same hash are
/// marked as duplicates.
///
/// Because carve workers run in parallel, the arrival order of files may vary
/// between runs. This means which offset is recorded as "first" for a given
/// SHA256 can differ. However, the set of unique hashes and the total
/// duplicate count are deterministic.
use dashmap::DashMap;

/// Tracks previously seen SHA256 hashes to detect duplicate carved files.
pub struct DedupTracker {
    seen: DashMap<String, u64>,
}

/// Result of a deduplication check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DedupResult {
    pub is_duplicate: bool,
    pub duplicate_of_offset: Option<u64>,
}

impl DedupTracker {
    pub fn new() -> Self {
        Self {
            seen: DashMap::new(),
        }
    }

    /// Check whether a SHA256 hash has been seen before. If not, register it
    /// with the given offset. Returns whether this is a duplicate and, if so,
    /// the offset of the first occurrence.
    pub fn check_and_register(&self, sha256: &str, offset: u64) -> DedupResult {
        use dashmap::mapref::entry::Entry;
        match self.seen.entry(sha256.to_string()) {
            Entry::Occupied(e) => DedupResult {
                is_duplicate: true,
                duplicate_of_offset: Some(*e.get()),
            },
            Entry::Vacant(e) => {
                e.insert(offset);
                DedupResult {
                    is_duplicate: false,
                    duplicate_of_offset: None,
                }
            }
        }
    }
}

impl Default for DedupTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_insert_is_not_duplicate() {
        let tracker = DedupTracker::new();
        let result = tracker.check_and_register("aabbccdd", 100);
        assert!(!result.is_duplicate);
        assert_eq!(result.duplicate_of_offset, None);
    }

    #[test]
    fn second_insert_is_duplicate() {
        let tracker = DedupTracker::new();
        tracker.check_and_register("aabbccdd", 100);
        let result = tracker.check_and_register("aabbccdd", 200);
        assert!(result.is_duplicate);
        assert_eq!(result.duplicate_of_offset, Some(100));
    }

    #[test]
    fn different_hashes_are_not_duplicates() {
        let tracker = DedupTracker::new();
        tracker.check_and_register("aabbccdd", 100);
        let result = tracker.check_and_register("eeff0011", 200);
        assert!(!result.is_duplicate);
        assert_eq!(result.duplicate_of_offset, None);
    }

    #[test]
    fn equal_offset_is_duplicate() {
        let tracker = DedupTracker::new();
        tracker.check_and_register("aabbccdd", 100);
        let result = tracker.check_and_register("aabbccdd", 100);
        assert!(result.is_duplicate);
        assert_eq!(result.duplicate_of_offset, Some(100));
    }

    #[test]
    fn concurrent_access() {
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

        // All 400 unique hashes should now be tracked; re-inserting should be duplicate
        let result = tracker.check_and_register("thread0_hash0", 999999);
        assert!(result.is_duplicate);
        assert_eq!(result.duplicate_of_offset, Some(0));
    }
}
