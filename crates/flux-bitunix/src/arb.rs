//! Cross-venue spread scanner — Bitunix vs Bybit vs the Polygon pool.
//!
//! # What two exchanges actually buy you
//! One venue only lets you take a price. Two let you compare, and a difference between two honest
//! quotes for the same asset is the only kind of edge that does not depend on predicting anything.
//! This module finds those differences and — more importantly — tells you when they are **not**
//! real, which is most of the time.
//!
//! # The discipline that makes this useful rather than dangerous
//! A naive scanner subtracts two prices, sees 0.4%, and calls it profit. It is not. A spread is only
//! an opportunity after it survives:
//!
//! * **taker fee on both legs** — you cross two spreads, not one;
//! * **the size you can actually fill**, which on a thin book or a shallow pool is far less than the
//!   size you wanted;
//! * **the transfer**, if the legs are on different venues and the asset has to move.
//!
//! [`edge_bps`] subtracts the first, [`Opportunity::verdict`] refuses to call anything actionable
//! that does not clear a floor, and the SIGIL path below is measured against real pool depth rather
//! than spot price. Nothing here places an order; it produces a ranked reading a human acts on.

use serde_json::{json, Value};

/// Round-trip taker cost in basis points, both legs. Bitunix and Bybit both charge ~6 bps taker on
/// USDT perps, so a two-leg round trip is ~12 bps before anything else goes wrong.
pub const DEFAULT_ROUND_TRIP_BPS: f64 = 12.0;

/// A spread has to beat this after costs before it is worth a human's attention. Below it, the
/// number is inside the noise of the two venues' own quote latency.
pub const ACTIONABLE_FLOOR_BPS: f64 = 15.0;

/// Gross spread between two prices, in basis points, signed from `a` to `b`.
/// Positive means `b` is dearer: buy on `a`, sell on `b`.
pub fn spread_bps(price_a: f64, price_b: f64) -> Option<f64> {
    if !(price_a > 0.0) || !(price_b > 0.0) || !price_a.is_finite() || !price_b.is_finite() {
        return None;
    }
    Some((price_b - price_a) / price_a * 10_000.0)
}

/// What is left of a spread after the round-trip taker fee. This is the only number worth reading.
pub fn edge_bps(price_a: f64, price_b: f64, round_trip_bps: f64) -> Option<f64> {
    spread_bps(price_a, price_b).map(|s| s.abs() - round_trip_bps.max(0.0))
}

#[derive(Debug, Clone)]
pub struct Opportunity {
    pub symbol: String,
    pub venue_cheap: String,
    pub venue_dear: String,
    pub price_cheap: f64,
    pub price_dear: f64,
    pub gross_bps: f64,
    pub edge_bps: f64,
}

impl Opportunity {
    /// Deliberately conservative wording. An "opportunity" that has not cleared costs is a loss
    /// with a nice name, and this is the sentence a human reads before risking money.
    pub fn verdict(&self) -> &'static str {
        if self.edge_bps <= 0.0 { "NO EDGE — the spread does not cover the round-trip fee" }
        else if self.edge_bps < ACTIONABLE_FLOOR_BPS { "MARGINAL — inside quote noise; not actionable" }
        else if self.edge_bps < 50.0 { "THIN — real but small; size is what decides it" }
        else { "WIDE — check both books are live before believing this" }
    }
    pub fn actionable(&self) -> bool { self.edge_bps >= ACTIONABLE_FLOOR_BPS }

    pub fn to_json(&self) -> Value {
        json!({
            "symbol": self.symbol,
            "buy_on": self.venue_cheap, "buy_at": self.price_cheap,
            "sell_on": self.venue_dear, "sell_at": self.price_dear,
            "gross_spread_bps": self.gross_bps,
            "edge_after_fees_bps": self.edge_bps,
            "round_trip_fee_bps": DEFAULT_ROUND_TRIP_BPS,
            "verdict": self.verdict(),
            "actionable": self.actionable(),
        })
    }
}

/// Compare one symbol across two venues. `None` when either price is missing — a scanner that
/// treats a failed fetch as a zero price invents enormous fake spreads.
pub fn compare(symbol: &str, venue_a: &str, price_a: f64, venue_b: &str, price_b: f64,
               round_trip_bps: f64) -> Option<Opportunity> {
    let gross = spread_bps(price_a, price_b)?;
    let edge = edge_bps(price_a, price_b, round_trip_bps)?;
    let (vc, pc, vd, pd) = if price_a <= price_b {
        (venue_a, price_a, venue_b, price_b)
    } else {
        (venue_b, price_b, venue_a, price_a)
    };
    Some(Opportunity {
        symbol: symbol.to_uppercase(),
        venue_cheap: vc.to_string(), venue_dear: vd.to_string(),
        price_cheap: pc, price_dear: pd,
        gross_bps: gross.abs(), edge_bps: edge,
    })
}

/// Scan a symbol list across both exchanges. Failures are reported per-symbol rather than aborting
/// the sweep: one delisted ticker must not blind you to the other nine.
pub fn scan(symbols: &[String]) -> Value {
    let bu = crate::Bitunix::public();
    let by = crate::bybit::Bybit::public();
    let mut found = Vec::new();
    let mut skipped = Vec::new();

    for sym in symbols {
        let pa = bu.as_ref().ok().and_then(|c| c.last_price(sym).ok());
        let pb = by.as_ref().ok().and_then(|c| c.last_price("linear", sym).ok());
        match (pa, pb) {
            (Some(a), Some(b)) => {
                if let Some(o) = compare(sym, "bitunix", a, "bybit", b, DEFAULT_ROUND_TRIP_BPS) {
                    found.push(o);
                }
            }
            (a, b) => skipped.push(json!({
                "symbol": sym,
                "bitunix": a.map(Value::from).unwrap_or(json!("unavailable")),
                "bybit": b.map(Value::from).unwrap_or(json!("unavailable")),
                "why": "need a live price on BOTH venues to compare"
            })),
        }
    }
    found.sort_by(|x, y| y.edge_bps.partial_cmp(&x.edge_bps).unwrap_or(std::cmp::Ordering::Equal));
    let actionable = found.iter().filter(|o| o.actionable()).count();
    json!({
        "ok": true,
        "compared": found.len(), "skipped": skipped.len(), "actionable": actionable,
        "round_trip_fee_bps": DEFAULT_ROUND_TRIP_BPS,
        "actionable_floor_bps": ACTIONABLE_FLOOR_BPS,
        "opportunities": found.iter().map(|o| o.to_json()).collect::<Vec<_>>(),
        "unavailable": skipped,
        "note": "prices are last-traded, not executable quotes. Depth decides whether any of this \
                 survives contact with a real order — check the book before sizing.",
    })
}

/// The SIGIL question, answered honestly.
///
/// Viktor's actual goal is "sell mined SIGIL somewhere real". The scanner above cannot help with
/// that, and saying so plainly is more useful than inventing a route: **wSIGIL is not listed on
/// Bitunix or Bybit**, so there is no cross-exchange leg to arbitrage. The only venue that will take
/// wSIGIL today is the Polygon pool — and the binding constraint there is depth, not price.
///
/// So this reports the largest sale the pool can absorb at a tolerable price impact, which is the
/// number that actually governs how much mined SIGIL can be turned into USDC per day.
pub fn sigil_sale_capacity(reserve_usdc: f64, reserve_wsigil: f64, max_impact_pct: f64) -> Value {
    if !(reserve_usdc > 0.0) || !(reserve_wsigil > 0.0) {
        return json!({"ok": false, "error": "pool is empty on at least one side — nothing can be sold"});
    }
    let spot = reserve_usdc / reserve_wsigil;
    // Binary-search the largest wSIGIL sale whose price impact stays under the tolerance.
    let (mut lo, mut hi) = (0.0f64, reserve_wsigil * 10.0);
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        let out = crate::pool::amm_out(mid, reserve_wsigil, reserve_usdc);
        let eff = if mid > 0.0 { out / mid } else { 0.0 };
        let impact = if eff > 0.0 { (spot / eff - 1.0) * 100.0 } else { f64::INFINITY };
        if impact > max_impact_pct { hi = mid; } else { lo = mid; }
    }
    let sell = lo;
    let proceeds = crate::pool::amm_out(sell, reserve_wsigil, reserve_usdc);
    json!({
        "ok": true,
        "pool_tvl_usd": reserve_usdc * 2.0,
        "spot_usdc_per_wsigil": spot,
        "max_impact_pct": max_impact_pct,
        "max_sellable_wsigil": sell,
        "proceeds_usdc": proceeds,
        "verdict": if proceeds < 1.0 {
            "The pool cannot absorb a meaningful sale. Mined SIGIL has no real exit here until \
             liquidity is added — this is a depth problem, not a price problem."
        } else if proceeds < 100.0 {
            "Only a token-sized sale clears. Adding depth raises this roughly in proportion."
        } else {
            "The pool can take a real sale at this tolerance."
        },
        "note": "wSIGIL is not listed on Bitunix or Bybit, so there is no cross-exchange arbitrage \
                 leg for it. The exchanges are useful for BTC/ETH spreads and for holding quote \
                 currency — not for selling SIGIL.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spread_is_signed_and_scaled_correctly() {
        // 100 -> 101 is +100 bps
        assert!((spread_bps(100.0, 101.0).unwrap() - 100.0).abs() < 1e-9);
        assert!((spread_bps(101.0, 100.0).unwrap() + 99.0099).abs() < 1e-3);
    }

    #[test]
    fn a_bad_price_yields_none_never_a_huge_fake_spread() {
        assert!(spread_bps(0.0, 100.0).is_none());
        assert!(spread_bps(100.0, 0.0).is_none());
        assert!(spread_bps(f64::NAN, 100.0).is_none());
        assert!(spread_bps(-5.0, 100.0).is_none());
    }

    #[test]
    fn edge_subtracts_the_round_trip_fee() {
        // 100 bps gross, 12 bps fees -> 88 bps edge
        assert!((edge_bps(100.0, 101.0, 12.0).unwrap() - 88.0).abs() < 1e-9);
        // a spread smaller than the fee is a LOSS, and must show as negative
        assert!(edge_bps(100.0, 100.05, 12.0).unwrap() < 0.0);
    }

    #[test]
    fn a_spread_inside_the_fee_is_not_an_opportunity() {
        let o = compare("BTCUSDT", "bitunix", 100.0, "bybit", 100.05, 12.0).unwrap();
        assert!(!o.actionable());
        assert!(o.verdict().starts_with("NO EDGE"), "got {}", o.verdict());
    }

    #[test]
    fn marginal_edge_is_named_marginal_not_actionable() {
        // 20 bps gross - 12 fee = 8 bps edge, under the 15 bps floor
        let o = compare("BTCUSDT", "bitunix", 100.0, "bybit", 100.2, 12.0).unwrap();
        assert!(o.edge_bps > 0.0 && !o.actionable());
        assert!(o.verdict().starts_with("MARGINAL"), "got {}", o.verdict());
    }

    #[test]
    fn direction_always_names_the_cheap_venue_as_the_buy() {
        let o = compare("ETHUSDT", "bitunix", 2000.0, "bybit", 2020.0, 12.0).unwrap();
        assert_eq!(o.venue_cheap, "bitunix");
        assert_eq!(o.venue_dear, "bybit");
        let r = compare("ETHUSDT", "bitunix", 2020.0, "bybit", 2000.0, 12.0).unwrap();
        assert_eq!(r.venue_cheap, "bybit");
        assert_eq!(r.venue_dear, "bitunix");
        assert!((r.gross_bps - o.gross_bps).abs() < 20.0, "magnitude is direction-independent");
    }

    #[test]
    fn sigil_capacity_on_the_real_measured_pool_is_effectively_zero() {
        // the live wSIGIL3 pool, measured: 0.097983 USDC / 0.010239 wSIGIL
        let r = sigil_sale_capacity(0.097983, 0.010239, 1.0);
        assert_eq!(r["ok"], true);
        assert!(r["proceeds_usdc"].as_f64().unwrap() < 1.0);
        assert!(r["verdict"].as_str().unwrap().contains("depth problem"));
    }

    #[test]
    fn sigil_capacity_scales_with_depth() {
        let thin = sigil_sale_capacity(1_000.0, 1_000.0, 1.0);
        let deep = sigil_sale_capacity(100_000.0, 100_000.0, 1.0);
        assert!(deep["proceeds_usdc"].as_f64().unwrap() > thin["proceeds_usdc"].as_f64().unwrap() * 50.0,
                "100x the depth should permit far more size");
    }

    #[test]
    fn sigil_capacity_refuses_an_empty_pool_rather_than_dividing_by_zero() {
        assert_eq!(sigil_sale_capacity(0.0, 5.0, 1.0)["ok"], false);
        assert_eq!(sigil_sale_capacity(5.0, 0.0, 1.0)["ok"], false);
    }

    #[test]
    fn capacity_respects_the_impact_tolerance() {
        let tight = sigil_sale_capacity(10_000.0, 10_000.0, 0.5);
        let loose = sigil_sale_capacity(10_000.0, 10_000.0, 5.0);
        assert!(loose["max_sellable_wsigil"].as_f64().unwrap()
              > tight["max_sellable_wsigil"].as_f64().unwrap());
    }
}
