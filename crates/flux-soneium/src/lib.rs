//! flux-soneium — SIGIL's second EVM leg.
//!
//! Polygon carries wSIGIL3 (`0x3FCED760…`, Uniswap V2 pool `0x7e9C5d73…`, $0.001/SIGIL).
//! This crate puts the SAME contract (byte-identical artifact, same selectors, same
//! `BurnedTo` topic) on **Soneium** — Sony's OP-Stack L2, chain id 1868 — and seeds a
//! Uniswap-V2-style pool there, so SIGIL can be bought on two chains instead of one.
//!
//! # What is measured vs assumed
//! * Chain ids, WETH predeploy, USDC.e, USDT, ASTR, Permit2: from docs.soneium.org, and
//!   re-checked live by [`rpc::connect`] (`eth_chainId`) and [`dex::verify_router`].
//! * The DEX router is NOT trusted from a constant. [`dex::pick_router`] verifies every
//!   candidate on-chain (`factory()`, `WETH()` == predeploy, `allPairsLength()`, and the
//!   depth of its WETH/USDC.e pair) and picks the deepest — the same "canonical factory
//!   guard" that caught the dead Uniswap clone on Polygon, generalised.
//! * ETH/USD is derived from that WETH/USDC.e pair's reserves — no price API, no key.
//! * Gas: every limit is `estimate_gas × 1.12`; the OP-Stack L1 data fee is read from the
//!   GasPriceOracle predeploy. A padded limit is the same as not having the money.
//!
//! # RPC
//! Alchemy first (`https://soneium-mainnet.g.alchemy.com/v2/<key>`, key from
//! `/root/.config/alchemy/api_key` or `ALCHEMY_API_KEY`), public `rpc.soneium.org` as
//! fallback. If the Alchemy app has Soneium disabled the error is surfaced verbatim with
//! the dashboard URL — it is a one-click fix on the operator's side, not a code problem.
//!
//! # What this crate does NOT do
//! It does not start a bridge relayer. `sigil-bridge-relayer` stays masked (unbacked-mint
//! hazard). [`relay`] publishes the *connection descriptor* (chain, token, pair, router,
//! selectors, decimal shift) that a relayer or a game MCP consumes; a pool seeded here is
//! **built**, not **bridged**, until that relayer is un-masked by the operator.

pub mod chain;
pub mod dex;
pub mod plan;
pub mod pool;
pub mod relay;
pub mod rpc;
pub mod wsigil;

pub use chain::{Network, MAINNET_CHAIN_ID, MINATO_CHAIN_ID};
pub use dex::{pick_router, verify_router, RouterVerdict};
pub use plan::FundingPlan;
pub use pool::{SeedPlan, Quote};
pub use relay::RelayDescriptor;
pub use rpc::{connect, Connected, RpcSource};

/// Polygon anchor price the Soneium pool opens at, so both legs quote the same SIGIL.
pub const ANCHOR_USD_PER_SIGIL: f64 = 0.001;
