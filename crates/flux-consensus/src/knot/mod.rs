//! Transaction knot diagrams + invariants.
//!
//! Lifted from `/opt/orobit/shared/QTFT/qtft-api/src/blockchain/knot.rs`
//! on Beta (2026-05-29). The lift is API-faithful but decouples from the
//! upstream `QuantumTransaction` type via the [`TransactionLike`] trait so
//! any Flux/SIGIL transaction type can be analysed without bringing the
//! QTFT-api crate along.
//!
//! ## Layout
//!
//! - [`crossing`] — `CrossingType`, `Crossing`, `KnotStrand`
//! - [`jones`] — `JonesPolynomial`
//! - [`transaction_knot`] — `TransactionKnot`, `KnotInvariants`,
//!   `KnotSecurityProof`, plus the `TransactionLike` trait family
//!
//! All three submodules are flat re-exported from this `mod.rs` so callers
//! can write `use flux_consensus::knot::TransactionKnot;` without nesting.

pub mod crossing;
pub mod jones;
pub mod transaction_knot;

pub use crossing::{Crossing, CrossingType, KnotStrand};
pub use jones::JonesPolynomial;
pub use transaction_knot::{
    KnotInvariants, KnotSecurityProof, TransactionInputLike, TransactionKnot,
    TransactionLike, TransactionOutputLike,
};
