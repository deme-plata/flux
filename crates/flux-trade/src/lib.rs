//! flux-trade — the DECIDE+SIZE brain of the agentic-trading stack, lifted from q-narwhalknight's
//! `q-trading-bot`. It composes three reused, battle-tested cores:
//!   • `indicators` — 9 SIMD technical indicators → an `AnalyticsSnapshot` with confluence
//!     `long_signal`/`short_signal` (a free, self-hosted DECIDE layer — no paid signal API needed)
//!   • `kelly` — drift/volatility from price history → Kelly-criterion position sizing
//!   • `binance` — a public market-data feed (klines + mark price), no key required
//!
//! `decide()` ties them together: fetch candles → run indicators → emit a `{action, reason,
//! confidence, kelly_fraction}` decision an agent can act on. This feeds the execution layer
//! ([[flux-0x]] quote → Coinbase CDP settle), gated by the Verified Execution Gate. Pure data in,
//! a decision with a HUMAN-READABLE `reason` out (that string is the provenance of the trade).

pub mod indicators;
pub mod kelly;
pub mod binance;

use crate::binance::Binance;
use crate::indicators::IndicatorSet;
use crate::kelly::{kelly_fraction, PriceHistory};
use serde_json::{json, Value};

/// What the brain decides. `Hold` when the bull/bear signals disagree or neither fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action { Buy, Sell, Hold }

pub fn action_str(a: &Action) -> &'static str { match a { Action::Buy => "BUY", Action::Sell => "SELL", Action::Hold => "HOLD" } }

/// The confluence verdict — pure & testable. A trade only when one side fires cleanly.
pub fn verdict(long_signal: bool, short_signal: bool) -> Action {
    if long_signal && !short_signal { Action::Buy }
    else if short_signal && !long_signal { Action::Sell }
    else { Action::Hold }
}

/// Binance interval string → seconds (for annualizing drift/vol).
pub fn interval_secs(iv: &str) -> u64 {
    match iv {
        "1m" => 60, "3m" => 180, "5m" => 300, "15m" => 900, "30m" => 1800,
        "1h" => 3600, "2h" => 7200, "4h" => 14400, "6h" => 21600, "12h" => 43200, "1d" => 86400,
        _ => 3600,
    }
}

/// THE DECIDE+SIZE CALL: live market → indicators → a sized, explained decision (no execution).
pub fn decide(symbol: &str, interval: &str, limit: u32) -> Result<Value, String> {
    if symbol.trim().is_empty() { return Err("symbol is required, e.g. BTCUSDT".into()); }
    let b = Binance::new();
    let candles = b.klines(symbol, interval, limit.clamp(60, 1500))?;
    if candles.len() < 30 { return Err(format!("not enough candles ({}) to warm the indicators", candles.len())); }

    let mut ind = IndicatorSet::new();
    let ohlcv: Vec<(f64, f64, f64, f64)> = candles.iter().map(|c| (c.high, c.low, c.close, c.volume)).collect();
    let snap = ind.warmup(&ohlcv).ok_or("indicators produced no snapshot")?;

    let act = verdict(snap.long_signal, snap.short_signal);

    let mut ph = PriceHistory::new(candles.len().max(2));
    for c in &candles { ph.push(c.close); }
    let secs = interval_secs(interval);
    let mu = ph.drift_annualized(secs);
    let sigma = ph.volatility_annualized(secs);
    let kf = kelly_fraction(mu, sigma, 1.0e9); // deep CEX liquidity ⇒ sizing is drift/vol-driven

    let reason = format!(
        "RSI {:.0} · MACD {} · ADX {:.0} (+DI {:.0}/-DI {:.0}) · %B {:.2} ⇒ {}",
        snap.rsi, if snap.macd_hist >= 0.0 { "↑" } else { "↓" }, snap.adx, snap.plus_di, snap.minus_di, snap.bb_pct_b, action_str(&act),
    );

    Ok(json!({
        "symbol": symbol.to_uppercase(), "interval": interval, "candles": candles.len(),
        "price": snap.close,
        "action": action_str(&act),
        "confidence": (snap.adx / 50.0).clamp(0.0, 1.0), // ADX = trend strength
        "reason": reason,
        "sizing": { "kelly_fraction": kf, "drift_annual": mu, "vol_annual": sigma },
        "indicators": {
            "rsi": snap.rsi, "macd_hist": snap.macd_hist, "adx": snap.adx,
            "plus_di": snap.plus_di, "minus_di": snap.minus_di, "bb_pct_b": snap.bb_pct_b,
            "atr": snap.atr, "vwap": snap.vwap, "tenkan": snap.tenkan, "kijun": snap.kijun,
        },
        "long_signal": snap.long_signal, "short_signal": snap.short_signal,
        "note": "DECIDE+SIZE only — no order placed. Feeds flux-0x/CDP through the Verified Execution Gate.",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indicators::IndicatorSet;

    #[test]
    fn verdict_only_trades_on_clean_confluence() {
        assert_eq!(verdict(true, false), Action::Buy);
        assert_eq!(verdict(false, true), Action::Sell);
        assert_eq!(verdict(false, false), Action::Hold);
        assert_eq!(verdict(true, true), Action::Hold); // contradiction ⇒ stand aside
    }

    #[test]
    fn interval_maps_to_seconds() {
        assert_eq!(interval_secs("1h"), 3600);
        assert_eq!(interval_secs("1d"), 86400);
        assert_eq!(interval_secs("weird"), 3600); // sane default
    }

    #[test]
    fn lifted_indicators_run_and_produce_a_snapshot() {
        // a synthetic rising series — proves the lifted SIMD indicators compile + run end-to-end
        let mut ind = IndicatorSet::new();
        let candles: Vec<(f64, f64, f64, f64)> = (0..80).map(|i| {
            let c = 100.0 + i as f64; (c + 0.5, c - 0.5, c, 1000.0)
        }).collect();
        let snap = ind.warmup(&candles).expect("snapshot");
        assert!((snap.close - 179.0).abs() < 1e-6, "last close flows through");
        assert!(snap.rsi >= 0.0 && snap.rsi <= 100.0, "RSI bounded");
        assert!(snap.rsi > 50.0, "a monotonic rise ⇒ RSI in the upper half (got {})", snap.rsi);
    }

    #[test]
    fn kelly_prefers_positive_drift() {
        let up = kelly_fraction(0.5, 0.4, 1.0e9);
        let down = kelly_fraction(-0.5, 0.4, 1.0e9);
        assert!(up > down, "positive drift ⇒ larger Kelly fraction ({up} > {down})");
    }
}
