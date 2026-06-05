//! flux-keel — supercluster stability & robustness.
//!
//! Three primitives, usable identically in the **frontend** (a UI calling a
//! backend) and the **backend** (a node calling its peers):
//!   - [`circuit::CircuitBreaker`] — stop calling a service that's failing,
//!     auto-probe for recovery (Closed → Open → HalfOpen → Closed).
//!   - [`retry`] — exponential backoff + jitter so a recovering service isn't
//!     thundering-herded.
//!   - [`health`] — quorum aggregation: the cluster is Up iff ≥ quorum nodes Up.
//!
//! Everything takes time as a parameter (`now_ms`) — no hidden clock — so it's
//! deterministic + testable, and works on wasm (frontend) and native (backend).
#![warn(missing_docs)]
pub mod circuit;
pub mod health;
pub mod retry;
pub use circuit::{CircuitBreaker, CircuitState};
pub use health::{aggregate, Health};
pub use retry::Backoff;
