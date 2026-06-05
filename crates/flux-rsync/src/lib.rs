//! flux-rsync — content-addressed parallel verified batch copy/sync.
//!
//! Better than rsync's default for large trees because:
//!   • **content dedup**: skip a file iff its BLAKE3 already matches at dest
//!     (rsync's quick-check trusts size+mtime; flux-rsync trusts the bytes);
//!   • **verify**: every copied file is re-hashed to confirm integrity;
//!   • **parallel**: files copied across N threads;
//!   • **streaming**: 1 MB chunks, so 10 TB files don't blow memory.
#![warn(missing_docs)]
pub mod sync;
pub use sync::{sync, SyncReport};
