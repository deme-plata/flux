// flux-groot — wallet-gated control surface for the NVIDIA Isaac GR00T
// reference humanoid (announced 2026-06-01, GTC Taipei: Unitree H2 Plus body +
// two Sharpa five-fingered hands on Jetson Thor; 75 DoF = 31 body + 22 per
// hand; Isaac GR00T open stack for policy/teleop/sim).
//
// What this crate is:
//   spec.rs     — the control interface modeled as `flux_api::ApiEndpoint`s,
//                 so OpenAPI 3.1 + the TS/Python/Go/Rust/Kotlin SDKs (with the
//                 v0.15-B auto-paginators + SSE readers) fall out of one Rust
//                 source of truth.
//   envelope.rs — the QRBT1 signed-command envelope. Every motion command is
//                 signed by a Quillon wallet's Ed25519 key. For client-managed
//                 Quillon wallets THE ADDRESS IS THE PUBKEY (the same fact
//                 q-narwhalknight's wallet_auth exploits), so the daemon can
//                 verify any wallet's signature with no key exchange at all.
//   daemon.rs   — a reference daemon: verifies envelopes, tracks mock 75-DoF
//                 actuation state, streams SSE telemetry, serves a
//                 cursor-paginated command/receipt history, and appends a
//                 pay-per-command QUG receipt per accepted command (settlement
//                 is out-of-band — this crate never moves funds).
//
// Safety asymmetry (deliberate): /v1/robot/estop requires NO signature — any
// party may always stop the robot — while every motion command requires a
// valid signature from an allowlisted operator wallet, a fresh strictly-
// increasing nonce (replay-proof), and a timestamp within the freshness
// window. Auth gates motion, never safety.
//
// No real robot is attached here. The daemon is the verification + settlement
// boundary; swapping mock actuation for ROS 2 / Isaac middleware calls does
// not change the wire contract.

pub mod daemon;
pub mod envelope;
pub mod spec;

pub use daemon::{Daemon, DaemonConfig};
pub use envelope::{
    canonical_bytes, sign_command, verify_envelope, EnvelopeError, ReplayGuard, SignedCommand,
    FRESHNESS_WINDOW_SECS,
};
pub use spec::{groot_endpoints, groot_schemas, BODY_DOF, HAND_DOF, TOTAL_DOF};
