//! flux-vite-engine — the visual dev engine for Vite + TS + React projects.
//!
//! Spawns `vite` (or `npx vite`) as a child process, parses HMR + transform
//! events from stdout, and exposes them as a typed event stream + a state
//! snapshot. The `vite-garden.html` surface consumes the snapshot to render
//! a kinetic dashboard (subway-map component tree, HMR pulse ribbon, bundle
//! treemap, transform waterfall, SAP score gauge).
//!
//! ## Quick start
//!
//! ```no_run
//! use flux_vite_engine::{ViteEngine, ViteConfig};
//! # async fn run() -> anyhow::Result<()> {
//! let cfg = ViteConfig::for_project("/path/to/my-vite-app");
//! let engine = ViteEngine::spawn(cfg).await?;
//! let mut events = engine.subscribe();
//! while let Ok(ev) = events.recv().await {
//!     println!("{ev:?}");
//! }
//! # Ok(()) }
//! ```
//!
//! ## What v0.1 ships (this crate)
//!
//! - Spawn Vite + capture stdout/stderr
//! - Parse HMR updates, page reloads, transform timing, errors
//! - In-memory state container with broadcast-channel event stream
//! - SAP-style score derivation
//!
//! ## What v0.1 deliberately does NOT ship (V1.1 follow-up lanes)
//!
//! - WebSocket tap into Vite's `/__vite_hmr` (more reliable than stdout but
//!   requires `tokio-tungstenite` workspace dep)
//! - SSE + REST exposure via fluxc-serve (separate sub-lane to avoid R2 merge)
//! - MCP tools `flux_vite_attach` / `flux_vite_stats` / `flux_vite_score`
//!   (separate sub-lane in fluxc-mcp)
//! - Real React Fiber tree introspection (requires `vite-plugin-flux` shipped on npm)
//! - Bundle composition (build-time only; dev-mode shows modules instead)
//! - tsc --watch type-check integration

pub mod bridges;
pub mod build;
pub mod events;
pub mod eye;
pub mod proc;
pub mod state;
pub mod verify;

pub use bridges::{event_to_search_doc, from_chiron, from_hotswap, from_search_tap, kind_tag};
pub use build::{build_project, AssetInfo, BuildReport, BuildScore};
pub use events::{ChironArm, HmrKind, TransformStage, ViteEvent, ViteEventKind};
pub use eye::{EyeClient, EyeCommand, EyeResult, EyeServer, PanelInfo, PanelSnapshot};
pub use proc::{ViteConfig, ViteEngine};
pub use state::{PathHits, SapScore, ViteSnapshot, ViteState};
pub use verify::{find_chrome, verify, verify_dist, VerifyReport};
