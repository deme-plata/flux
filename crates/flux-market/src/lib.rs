//! # flux-market — trading intelligence for the SIGIL bridge.
//!
//! Three pieces, built to be wrapped by `fluxc-mcp` as `flux_market_*` combos:
//!   - [`binance`] — live CEX price ("so you know the price").
//!   - [`dca`] — dollar-cost-averaging engine (Quillon q-trading-bot, Flux-native),
//!     quoted in USDS (the SIGIL stablecoin).
//!   - [`arb`] — CEX↔on-chain arbitrage-spread detector.
//!
//! The headline combo, [`snapshot`], does it in one call: fetch the live Binance
//! price + compute the arb signal vs an on-chain wBTC/USDS price — the
//! "arbitrage/DCA opportunity" scan an agent runs before trading.
//!
//! Follow-on modules (designed, not yet built): a Polymarket scanner
//! (`polymarket.rs`), on-chain price wiring (read the bridge's wBTC/USDS DEX
//! pool), and the USDS stablecoin contract itself.

pub mod arb;
pub mod binance;
pub mod bitrefill;
pub mod cost_model;
pub mod dca;
pub mod fear_greed;
pub mod governor;
pub mod invoice;
pub mod multi_agent;
pub mod news;
pub mod onboarding;
pub mod polymarket;
pub mod pricing;
pub mod sim;
pub mod storage_pool;
pub mod strategy;
pub mod treasury;

pub use arb::{scan as scan_arb, ArbDir, ArbSignal};
pub use binance::{spot_price, ticker_24h, MarketTicker};
pub use dca::{DcaPlan, Interval};

/// One-call market snapshot: the live CEX ticker + the arb signal vs a known
/// on-chain price. This is the `flux_market_scan` combo's payload.
#[derive(Debug, Clone)]
pub struct MarketSnapshot {
    pub ticker: MarketTicker,
    pub arb: ArbSignal,
}

/// Fetch the live Binance 24h ticker for `symbol` and scan for arb vs
/// `onchain_usd` (the on-chain wBTC/USDS price). `min_arb_pct` filters
/// sub-threshold spreads.
pub fn snapshot(symbol: &str, onchain_usd: f64, min_arb_pct: f64) -> Result<MarketSnapshot, String> {
    let ticker = binance::ticker_24h(symbol)?;
    let arb = arb::scan(ticker.last, onchain_usd, min_arb_pct);
    Ok(MarketSnapshot { ticker, arb })
}
