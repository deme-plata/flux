//! flux-buzz — Flux-native Buzz: a self-hosted workspace relay where humans
//! and AI agents collaborate as cryptographic peers.
//!
//! Inspired by Block's Buzz (chat + git + agents on one signed event log),
//! rebuilt on the Flux stack: blake3 content-addressing, Ed25519 identities,
//! an append-only JSONL log, and a hand-rolled tokio HTTP/WS relay — no
//! heavyweight framework, dogfood-buildable with `fluxc`.
//!
//! * [`event`] — signed, content-addressed events + participant identities
//! * [`store`] — append-only persisted log with channel/kind/cursor queries
//! * [`relay`] — REST + WebSocket server (plain TCP and TLS), embedded web UI

pub mod event;
pub mod relay;
pub mod store;

pub use event::{BuzzEvent, Identity};
pub use relay::RelayState;
pub use store::EventStore;
