//! flux-chronos — deterministic multiverse simulation for Flux chains.
//!
//! This scaffold (CHRONOS-A) lands the core abstractions: virtual clock,
//! seeded RNG, in-memory message bus, and a [`SimNode`] trait that a chain
//! crate (e.g. sigil-node) implements to plug in. A demo at the bottom
//! shows two simulated nodes exchanging a message under virtual time —
//! a 1-hour gossip round trip resolves in microseconds because the clock
//! is whatever the test says it is.
//!
//! See `flux/docs/flux-chronos-spec.md` for the full architecture, the
//! multiverse-fork / time-travel / property-fuzz / MCP / browser-viz phases
//! that follow this scaffold, and the rationale for every design choice.

#![warn(missing_docs)]

mod clock;
mod net;
mod node;
pub mod tourbillon;
mod universe;

pub use clock::{TickId, VirtualClock};
pub use net::{Envelope, NetEdge, NodeId, ScheduledDelivery};
pub use node::{NodeStepResult, SimNode};
pub use tourbillon::{Injection, PermutationOutcome, TourbillonReport};
pub use universe::{ScenarioSeed, Universe};

/// Convenience: simulated duration in microseconds. We keep micros instead
/// of nanos so a u64 fits ~580 years of simulated time — enough for any
/// realistic chain scenario without integer overflow.
pub type SimDuration = u64;

/// Convert seconds → SimDuration.
pub const fn secs(n: u64) -> SimDuration {
    n * 1_000_000
}

/// Convert milliseconds → SimDuration.
pub const fn millis(n: u64) -> SimDuration {
    n * 1_000
}

/// Convert minutes → SimDuration.
pub const fn mins(n: u64) -> SimDuration {
    n * 60 * 1_000_000
}

/// Convert hours → SimDuration.
pub const fn hours(n: u64) -> SimDuration {
    n * 60 * 60 * 1_000_000
}
