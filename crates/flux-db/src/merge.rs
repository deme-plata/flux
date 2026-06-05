//! v0.16: user-defined merge operator.
//!
//! Like RocksDB's `MergeOperator`. A merge entry is a *delta*: instead of
//! writing the full value, the user supplies bytes that the operator
//! combines with the existing value at read time.
//!
//! Example: a counter increment.
//!
//! ```text
//! struct AddInt;
//! impl MergeOperator for AddInt {
//!     fn merge(&self, existing: Option<&[u8]>, delta: &[u8]) -> Vec<u8> {
//!         let mut cur = existing
//!             .map(|e| i64::from_le_bytes(e.try_into().unwrap_or([0; 8])))
//!             .unwrap_or(0);
//!         cur += i64::from_le_bytes(delta.try_into().unwrap_or([0; 8]));
//!         cur.to_le_bytes().to_vec()
//!     }
//! }
//! let op: Arc<dyn MergeOperator> = Arc::new(AddInt);
//! db.set_merge_operator(op);
//! db.put(b"counter", &0i64.to_le_bytes())?;
//! db.merge(b"counter", &5i64.to_le_bytes())?;
//! db.merge(b"counter", &3i64.to_le_bytes())?;
//! assert_eq!(db.get(b"counter")?.as_deref(),
//!            Some(&8i64.to_le_bytes()[..]));
//! ```

/// User-defined value merge. Pure function: same inputs always produce
/// the same output. Holds no state (state lives inside the values).
pub trait MergeOperator: Send + Sync {
    /// Combine an existing value (None if the key is new) with a delta and
    /// return the new full value.
    fn merge(&self, existing: Option<&[u8]>, delta: &[u8]) -> Vec<u8>;

    /// Optional human-friendly name for logs / introspection.
    fn name(&self) -> &str { "user_merge" }
}
