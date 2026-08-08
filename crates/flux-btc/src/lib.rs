//! flux-btc — the Bitcoin accumulation brain.
//!
//! Composes two battle-tested crates into one BTC-focused decision:
//!   * [`flux_trade`] — 9 SIMD technical indicators → confluence long/short + Kelly sizing
//!   * [`flux_market`] — the crypto Fear & Greed index, DCA engine, spend governor
//!
//! The output is a **propose-only** recommendation: a `dip_strength` score
//! (0–100, higher = deeper/more-oversold dip = better accumulation) and a
//! suggested DCA size. It NEVER moves real Bitcoin — an agent surfaces the
//! recommendation, a human confirms the spend. This is the Carl-Runefelt
//! playbook made mechanical: **DCA the dip, never sell the core stack.**
//!
//! Scoring is split into PURE functions (`dip_strength`, `size_dca`,
//! `recommendation`) that take plain numbers, so the whole decision is unit-
//! tested offline with zero network — the live wrappers (`analyze`,
//! `dca_plan`) only fetch the inputs and hand them to the pure core.

use serde_json::{json, Value};

/// A single accumulation verdict — everything an agent needs to explain the
/// call, and nothing it needs to execute it (that stays with the human).
#[derive(Debug, Clone)]
pub struct BtcVerdict {
    pub price_usd: f64,
    /// Crypto Fear & Greed index value, 0 (extreme fear) – 100 (extreme greed).
    pub fear_greed: u8,
    pub fear_greed_label: &'static str,
    /// RSI(14) on the analysis interval — <30 oversold, >70 overbought.
    pub rsi: f64,
    /// Price relative to the Bollinger middle band (the MA): <1 = below = dip.
    pub price_vs_ma: f64,
    /// 0–100. Higher = deeper, more-oversold, more-feared dip = stronger buy.
    pub dip_strength: f64,
    /// Suggested DCA multiple of the base ticket (contrarian-sized). 0 = skip.
    pub dca_multiple: f64,
    /// Human-readable provenance of the call — the WHY, in one line.
    pub reason: String,
}

/// PURE. Blend the three independent dip signals into one 0–100 score.
///
/// * Fear (contrarian): extreme fear scores high. `fg` 0→100 maps 100→0.
/// * Oversold: RSI below 70 contributes; 30 and under is maxed.
/// * Below-the-mean: price under the moving average contributes; the further
///   below, the more, saturating around −20%.
///
/// Weights (fear 0.4, oversold 0.35, below-MA 0.25) reflect that sentiment
/// leads at cycle bottoms — but no single signal can peg the score alone.
pub fn dip_strength(fear_greed: u8, rsi: f64, price_vs_ma: f64) -> f64 {
    let fear = (100.0 - fear_greed as f64).clamp(0.0, 100.0);
    let oversold = ((70.0 - rsi) / 40.0 * 100.0).clamp(0.0, 100.0);
    // Price AT the MA is neutral (50), not minimum: below the mean pushes
    // toward 100 (buy the dip), above toward 0, saturating at ±20%.
    let below = (50.0 + (1.0 - price_vs_ma) / 0.20 * 50.0).clamp(0.0, 100.0);
    (fear * 0.40 + oversold * 0.35 + below * 0.25).clamp(0.0, 100.0)
}

/// PURE. Turn a dip_strength score into a DCA multiple of the base ticket.
///
/// A flat DCA buys the same each period; this tilts it — buy MORE when the dip
/// is strong, less (but never zero below the skip line) when it's weak. Capped
/// at `max_multiple` so a single euphoric-fear print can't drain the budget.
/// Below `skip_below`, returns 0.0 — "not a dip worth an extra ticket."
pub fn size_dca(dip_strength: f64, max_multiple: f64, skip_below: f64) -> f64 {
    if dip_strength < skip_below {
        return 0.0;
    }
    // 50 (neutral) → 1.0×; scales linearly to max at 100.
    let m = 1.0 + (dip_strength - 50.0) / 50.0 * (max_multiple - 1.0);
    m.clamp(0.0, max_multiple)
}

/// PURE. Assemble a verdict + human reason from already-fetched inputs.
pub fn recommendation(
    price_usd: f64,
    fear_greed: u8,
    fear_greed_label: &'static str,
    rsi: f64,
    price_vs_ma: f64,
) -> BtcVerdict {
    let strength = dip_strength(fear_greed, rsi, price_vs_ma);
    let multiple = size_dca(strength, 3.0, 20.0);
    let dip_word = match strength as u32 {
        0..=25 => "weak / not a dip",
        26..=50 => "mild dip",
        51..=75 => "strong dip",
        _ => "capitulation-grade dip",
    };
    let reason = format!(
        "{dip_word}: BTC ${price_usd:.0}, {fear_greed_label} ({fear_greed}), RSI {rsi:.0}, {:.1}% {} the MA → DCA {multiple:.2}×",
        (price_vs_ma - 1.0).abs() * 100.0,
        if price_vs_ma < 1.0 { "below" } else { "above" },
    );
    BtcVerdict {
        price_usd,
        fear_greed,
        fear_greed_label,
        rsi,
        price_vs_ma,
        dip_strength: strength,
        dca_multiple: multiple,
        reason,
    }
}

impl BtcVerdict {
    pub fn to_json(&self) -> Value {
        json!({
            "price_usd": self.price_usd,
            "fear_greed": self.fear_greed,
            "fear_greed_label": self.fear_greed_label,
            "rsi": self.rsi,
            "price_vs_ma": self.price_vs_ma,
            "dip_strength": (self.dip_strength * 10.0).round() / 10.0,
            "dca_multiple": (self.dca_multiple * 100.0).round() / 100.0,
            "reason": self.reason,
            "propose_only": true,
        })
    }
}

/// LIVE. Fetch BTC price + fear/greed + indicators and produce a verdict.
/// `interval` is a Binance kline interval (e.g. "1d", "1w"); `limit` candles.
pub fn analyze(interval: &str, limit: u32) -> Result<BtcVerdict, String> {
    // Indicators via flux-trade's decide (already fetches klines + runs the
    // set). Fields are nested under "indicators"; there's no bb_middle, but
    // bb_pct_b (Bollinger %B) locates price between the bands — a cleaner
    // price-vs-mean signal: 0.5 = at the MA, <0.5 = below (a dip).
    let decision = flux_trade::decide("BTCUSDT", interval, limit)?;
    let ind = &decision["indicators"];
    let rsi = ind["rsi"].as_f64().unwrap_or(50.0);
    let pct_b = ind["bb_pct_b"].as_f64();
    let close = decision["price"].as_f64().unwrap_or(0.0);

    // Live spot (may differ slightly from last candle close; prefer it).
    let price = flux_market::binance::spot_price("BTCUSDT").unwrap_or(close);
    // Map %B → a price_vs_ma ratio the pure scorer understands: %B 0.5 → 1.0
    // (at MA), and each band (±2σ) maps to ±20% so it saturates the `below`
    // signal exactly. No %B (cold indicators) → 1.0 neutral.
    let price_vs_ma = match pct_b {
        Some(b) => 1.0 + (b - 0.5) * 0.40,
        None => 1.0,
    };

    let fg = flux_market::fear_greed::fetch()?;
    Ok(recommendation(price, fg.value, fg.sentiment.label(), rsi, price_vs_ma))
}

/// LIVE. A full DCA proposal: the verdict + the concrete (still propose-only)
/// ticket sizing for a given base amount in USD. Returns the numbers a human
/// needs to approve a buy — and an explicit `execute: false` so no downstream
/// tool mistakes this for an order.
pub fn dca_plan(interval: &str, limit: u32, base_usd: f64) -> Result<Value, String> {
    let v = analyze(interval, limit)?;
    let suggested_usd = base_usd * v.dca_multiple;
    let btc_qty = if v.price_usd > 0.0 { suggested_usd / v.price_usd } else { 0.0 };
    Ok(json!({
        "verdict": v.to_json(),
        "base_usd": base_usd,
        "suggested_usd": (suggested_usd * 100.0).round() / 100.0,
        "suggested_btc": format!("{:.8}", btc_qty),
        "execute": false,
        "note": "Propose-only. DCA the dip, never sell the core. A human confirms every spend.",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extreme_fear_deep_oversold_scores_capitulation() {
        // FG 10 (extreme fear), RSI 25 (oversold), 15% below MA → very strong.
        let s = dip_strength(10, 25.0, 0.85);
        assert!(s > 75.0, "capitulation dip should score >75, got {s}");
        // And it should size UP toward the cap, never past it.
        let m = size_dca(s, 3.0, 20.0);
        assert!(m > 1.0 && m <= 3.0, "multiple {m} out of (1,3]");
    }

    #[test]
    fn extreme_greed_overbought_is_a_skip() {
        // FG 90 (greed), RSI 78 (overbought), 10% ABOVE MA → weak / skip.
        let s = dip_strength(90, 78.0, 1.10);
        assert!(s < 20.0, "euphoria should score <20, got {s}");
        assert_eq!(size_dca(s, 3.0, 20.0), 0.0, "below skip line → 0×");
    }

    #[test]
    fn neutral_market_holds_base_ticket() {
        // Neutral everything → ~50 score → ~1.0× (flat DCA, no tilt).
        let s = dip_strength(50, 50.0, 1.00);
        let m = size_dca(s, 3.0, 20.0);
        assert!((0.9..=1.15).contains(&m), "neutral should be ~1.0×, got {m}");
    }

    #[test]
    fn size_is_monotonic_and_capped() {
        let weak = size_dca(55.0, 3.0, 20.0);
        let strong = size_dca(95.0, 3.0, 20.0);
        assert!(strong > weak, "stronger dip must size larger");
        assert!(size_dca(100.0, 3.0, 20.0) <= 3.0, "never exceed the cap");
    }

    #[test]
    fn recommendation_reason_names_the_why() {
        let v = recommendation(65000.0, 12, "Extreme Fear", 28.0, 0.88);
        assert!(v.reason.contains("Extreme Fear"));
        assert!(v.reason.contains("below the MA"));
        assert!(v.dip_strength > 70.0);
        assert!(v.to_json()["propose_only"].as_bool().unwrap());
    }
}
