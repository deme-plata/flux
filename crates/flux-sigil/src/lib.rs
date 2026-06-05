//! # flux-sigil — BLAKE3 × SQIsign
//!
//! A sigil is a sign or seal — a single atomic mark that's both a hash and a signature.
//! flux-sigil composes BLAKE3's tree-hash with SQIsign's 177-byte post-quantum signatures.
//!
//! Two modes:
//!
//! 1. [`keyed`] — BLAKE3 keyed-hash where the 32-byte key is derived from a SQIsign signature
//!    (the Signature-as-Key pattern from CODWHALE_HANDOFF.md). Useful for tamper-evident
//!    content addressing bound to a PQ identity.
//!
//! 2. [`streaming`] — A streaming hasher that emits `(32B BLAKE3 digest, 177B SQIsign signature)`
//!    in one `finalize()` over the input. Single-pass PQ-authenticated hashing.

pub mod keyed;
pub mod streaming;
