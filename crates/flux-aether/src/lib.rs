//! flux-aether — a live distributed "mixer for files", where **each file is an
//! atomic block**.
//!
//! A file is chunked, erasure-coded (K-of-N), per-shard encrypted, content-
//! addressed, and MIXED into one indistinguishable shard pool scattered over
//! flux-p2p. The coherent file exists **HERE** (you hold the [`FileBlock`]),
//! **THERE** (shards live on peers), and **NOWHERE** (it is assembled nowhere —
//! it materializes only transiently on read, from any K shards). A peer holding
//! a shard cannot tell its file, its content, or even that it belongs to a
//! whole file.
//!
//! The key design choice (Viktor): a file's manifest has the **same atomic
//! structure as a SIGIL block header** — a *content root* (≈ a state root) and a
//! *shard merkle root* (≈ `txs_merkle_root`), plus a producer + (wired later) a
//! provenance `.proof` + SQIsign sig. The same primitives that verify a chain
//! block verify a stored file. Files become blocks.
//!
//! First target: WAV / MP3 (raw bytes here; format-aware chunking is a later lane).

#![warn(missing_docs)]

pub mod aether;
pub mod rs;
pub mod sov;
pub mod v2;

pub use aether::{reassemble, shard_file, AetherError, FileBlock, Hash, Shard};
pub use rs::{rs_reassemble, rs_shard};
pub use sov::{
    apply_download, divergence, gossip_until_converged, mesh_status_json, plan, sync_pair,
    AetherSyncPlan, Manifest, NodeIdentity, Ver, VersionEntry,
};
pub use v2::{merkle_root, pir_answer, pir_query, pir_reconstruct, verify_storage, TimeLock};
