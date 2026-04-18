/// Deduplication tracker based on SHA256 hashes.
///
/// This tracker is designed to be used from a **single thread** (the metadata
/// recording thread). It stores the first-encountered global offset for each
/// unique SHA256. Later arrivals with the same hash are marked as duplicates.
///
/// Because carve workers run in parallel, the arrival order of files at the
/// metadata thread may vary between runs. This means which offset is recorded
/// as "first" for a given SHA256 can differ. However, the set of unique hashes
/// and the total duplicate count are deterministic.
use std::collections::HashMap;

/// Tracks previously seen SHA256 hashes to detect duplicate carved files.
pub struct DedupTracker {
    seen: HashMap<String, u64>,
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
            seen: HashMap::new(),
        }
    }

    /// Check whether a SHA256 hash has been seen before. If not, register it
    /// with the given offset. Returns whether this is a duplicate and, if so,
    /// the offset of the first occurrence.
    pub fn check_and_register(&mut self, sha256: &str, offset: u64) -> DedupResult {
        match self.seen.entry(sha256.to_string()) {
            std::collections::hash_map::Entry::Occupied(e) => DedupResult {
                is_duplicate: true,
                duplicate_of_offset: Some(*e.get()),
            },
            std::collections::hash_map::Entry::Vacant(e) => {
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
        let mut tracker = DedupTracker::new();
        let result = tracker.check_and_register("aabbccdd", 100);
        assert!(!result.is_duplicate);
        assert_eq!(result.duplicate_of_offset, None);
    }

    #[test]
    fn second_insert_is_duplicate() {
        let mut tracker = DedupTracker::new();
        tracker.check_and_register("aabbccdd", 100);
        let result = tracker.check_and_register("aabbccdd", 200);
        assert!(result.is_duplicate);
        assert_eq!(result.duplicate_of_offset, Some(100));
    }

    #[test]
    fn different_hashes_are_not_duplicates() {
        let mut tracker = DedupTracker::new();
        tracker.check_and_register("aabbccdd", 100);
        let result = tracker.check_and_register("eeff0011", 200);
        assert!(!result.is_duplicate);
        assert_eq!(result.duplicate_of_offset, None);
    }

    #[test]
    fn equal_offset_is_duplicate() {
        let mut tracker = DedupTracker::new();
        tracker.check_and_register("aabbccdd", 100);
        let result = tracker.check_and_register("aabbccdd", 100);
        assert!(result.is_duplicate);
        assert_eq!(result.duplicate_of_offset, Some(100));
    }
}
