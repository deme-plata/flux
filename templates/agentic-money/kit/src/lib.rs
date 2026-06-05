//! agentic-money-kit — the reusable core every agentic-money agent on
//! Flux/SIGIL needs, so you don't re-derive the plumbing each time.
//!
//! Four modules, four hard-won lessons baked in:
//!
//! - [`gate`]   — the **Verified Execution Gate**. The primitive that makes an
//!               LLM (or any autonomous loop) *safe* with money tools: a
//!               decision only reaches the chain after passing a whitelist +
//!               amount clamp + balance check + honeypot block + slippage
//!               ceiling. Origin: the flux-hundred A100 honeypot lesson.
//! - [`rpc`]    — a dependency-light, std-only HTTP client for talking to a
//!               `sigil-rpcd` money daemon (the same std-only transport
//!               sigil-rpcd itself uses). No tokio, no reqwest, no TLS stack.
//! - [`wallet`] — **local-seed wallet bootstrap**. Fixes the `create_wallet`
//!               no-mnemonic trap: generate entropy locally, derive a stable
//!               address, keep the seed so the wallet is actually spendable.
//! - [`llm`]    — open-model (ollama / vLLM) **tool-call decide**, with the
//!               deepseek-r1 / qwen settings lessons baked in (free-form +
//!               high num_predict + lenient `<think>`-stripping parse).
//!
//! The four example bins in this workspace (`safe-trader`, `wallet-onboard`,
//! `webhook-agent`, `llm-trader`) are thin demonstrations — fork one, swap in
//! your strategy, ship.

pub mod gate;
pub mod llm;
pub mod rpc;
pub mod wallet;

pub use gate::{evaluate, GateConfig, Decision, Verdict};
pub use llm::{decide, LlmConfig};
pub use rpc::Rpc;
pub use wallet::Wallet;
