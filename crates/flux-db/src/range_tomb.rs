//! Range tombstones for v0.13 `delete_range(start, end)`.
//!
//! Pre-v0.13 deletions emitted one tombstone per key. `delete_range(0, 1M)`
//! would write a million tombstones. A range tombstone is a single
//! `[start, end)` interval. `get(k)` returns None if any active range
//! tombstone covers `k`; compaction drops keys covered by range tombstones.

/// A range tombstone covers all keys in `[start, end)`, born at `seq`.
#[derive(Debug, Clone)]
pub struct RangeTombstone {
    pub start: Vec<u8>,
    pub end: Vec<u8>,
    pub seq: u64,
}

impl RangeTombstone {
    /// True if this tombstone covers `key` (i.e. start ≤ key < end).
    pub fn covers(&self, key: &[u8]) -> bool {
        key >= self.start.as_slice() && key < self.end.as_slice()
    }
}

/// Returns true if any tombstone in the set covers `key`.
pub fn is_covered(tombs: &[RangeTombstone], key: &[u8]) -> bool {
    tombs.iter().any(|t| t.covers(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_covers_inclusive_start_exclusive_end() {
        let t = RangeTombstone { start: b"a".to_vec(), end: b"c".to_vec(), seq: 1 };
        assert!(t.covers(b"a"));
        assert!(t.covers(b"b"));
        assert!(!t.covers(b"c"));
        assert!(!t.covers(b"d"));
    }

    #[test]
    fn test_is_covered_multiple() {
        let tombs = vec![
            RangeTombstone { start: b"x".to_vec(), end: b"y".to_vec(), seq: 1 },
            RangeTombstone { start: b"a".to_vec(), end: b"c".to_vec(), seq: 2 },
        ];
        assert!(is_covered(&tombs, b"b"));
        assert!(is_covered(&tombs, b"x"));
        assert!(!is_covered(&tombs, b"m"));
    }
}
