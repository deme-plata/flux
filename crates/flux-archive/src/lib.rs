//! flux-archive — content-addressed, dedup, integrity-verified backup.
//!
//! `snapshot(src, store)` BLAKE3-addresses every file and dedups it into a CID
//! store (identical bytes stored once); `restore(manifest, store, dst)` rebuilds
//! and VERIFIES each file hashes back to its CID — a corrupt/tampered backup
//! cannot silently restore wrong bytes. The backup half of flux-aether's
//! content-addressing; pairs with flux-db + chronos for chain snapshots.
#![warn(missing_docs)]
pub mod archive;
pub use archive::{restore, snapshot, verify, Entry, Manifest};
