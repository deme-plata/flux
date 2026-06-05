//! polymarket.rs — read-only Polymarket scanner for arbitrage-betting profit.
//!
//! A binary prediction market's YES + NO should price to 1.00. When the best
//! YES + best NO < 1, buying BOTH locks (near) risk-free profit — the classic
//! Polymarket arb. We scan the public Gamma API (no account, read-only) and
//! return markets whose `1 - (yes + no)` margin clears a threshold. The agent
//! decides whether to act; we never auto-bet.

use serde::Deserialize;
use std::time::Duration;

#[derive(Deserialize)]
struct GammaMarket {
    #[serde(default)]
    question: String,
    /// JSON-encoded string array, e.g. "[\"0.43\",\"0.57\"]".
    #[serde(default, rename = "outcomePrices")]
    outcome_prices: String,
    #[serde(default)]
    closed: bool,
}

/// A market with a detected arbitrage margin.
#[derive(Debug, Clone)]
pub struct ArbMarket {
    pub question: String,
    pub yes: f64,
    pub no: f64,
    /// 1 - (yes + no), in %. Positive ⇒ buy-both arb.
    pub margin_pct: f64,
}

/// Parse Polymarket's `outcomePrices` JSON-string into (yes, no).
fn parse_prices(s: &str) -> Option<(f64, f64)> {
    let v: Vec<String> = serde_json::from_str(s).ok()?;
    if v.len() != 2 {
        return None;
    }
    Some((v[0].parse().ok()?, v[1].parse().ok()?))
}

/// Compute the buy-both arbitrage margin % for a binary market.
pub fn arb_margin_pct(yes: f64, no: f64) -> f64 {
    (1.0 - (yes + no)) * 100.0
}

/// Scan live Polymarket binary markets for arbs ≥ `min_margin_pct`.
pub fn scan_arbs(min_margin_pct: f64, limit: u32) -> Result<Vec<ArbMarket>, String> {
    let url = format!("https://gamma-api.polymarket.com/markets?closed=false&limit={limit}");
    let markets: Vec<GammaMarket> = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent("flux-market/0.1 polymarket")
        .build()
        .map_err(|e| e.to_string())?
        .get(&url)
        .send()
        .map_err(|e| format!("connect: {e}"))?
        .error_for_status()
        .map_err(|e| format!("http: {e}"))?
        .json()
        .map_err(|e| format!("decode: {e}"))?;

    let mut out: Vec<ArbMarket> = markets
        .into_iter()
        .filter(|m| !m.closed)
        .filter_map(|m| {
            let (yes, no) = parse_prices(&m.outcome_prices)?;
            let margin = arb_margin_pct(yes, no);
            (margin >= min_margin_pct).then_some(ArbMarket { question: m.question, yes, no, margin_pct: margin })
        })
        .collect();
    out.sort_by(|a, b| b.margin_pct.partial_cmp(&a.margin_pct).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arb_when_prices_sum_below_one() {
        // YES 0.45 + NO 0.50 = 0.95 → 5% buy-both arb.
        assert!((arb_margin_pct(0.45, 0.50) - 5.0).abs() < 1e-9);
        // efficient market (sum 1.0) → 0% arb.
        assert!(arb_margin_pct(0.40, 0.60).abs() < 1e-9);
    }

    #[test]
    fn parses_gamma_price_string() {
        assert_eq!(parse_prices("[\"0.43\",\"0.57\"]"), Some((0.43, 0.57)));
        assert_eq!(parse_prices("[\"0.5\"]"), None);
    }
}
