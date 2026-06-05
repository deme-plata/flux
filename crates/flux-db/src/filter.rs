//! v0.16: compaction filter.
//!
//! User logic that runs on every key/value during `compact()`. Lets you
//! drop expired records, transform legacy formats forward, or evict
//! tombstones early.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterDecision {
    /// Keep the value as-is.
    Keep,
    /// Drop the key entirely from the output SST.
    Drop,
    /// Replace the value with these bytes.
    Transform(Vec<u8>),
}

pub trait CompactionFilter: Send + Sync {
    fn filter(&self, key: &[u8], value: &[u8]) -> FilterDecision;
    fn name(&self) -> &str { "user_filter" }
}
