//! flux-market-demo — live Binance price → a DCA step + an arb scan. Proves the
//! price feed against the real Binance API, then runs the pure-logic engines.

use flux_market::dca::{DcaPlan, Interval, USDS_ONE};
use flux_market::{scan_arb, snapshot};

fn main() {
    let symbol = std::env::args().nth(1).unwrap_or_else(|| "BTCUSDT".to_string());
    println!("\n  flux-market — live price · DCA · arb   (symbol: {symbol})\n");

    // ── live Binance price (the headline: "so you know the price") ──
    match flux_market::ticker_24h(&symbol) {
        Ok(t) => {
            println!("  Binance {symbol}: ${:.2}  ({:+.2}% 24h · H ${:.0} / L ${:.0})", t.last, t.change_pct_24h, t.high_24h, t.low_24h);

            // ── DCA: $100 daily, simulate 3 buys at the live price ──
            let mut plan = DcaPlan::new("wBTC", 100 * USDS_ONE, Interval::Daily, 0);
            for d in 0..3 {
                plan.record_buy(t.last, d * Interval::Daily.secs());
            }
            println!("  DCA $100/day ×3 @ live price → acquired {} sat · avg cost ${:.2} · pnl {:+.2}%",
                plan.total_acquired_sat, plan.avg_cost_usd(), plan.pnl_pct(t.last));

            // ── arb: scan vs a sample on-chain price 0.6% below CEX ──
            let onchain = t.last * 0.994;
            let s = scan_arb(t.last, onchain, 0.4);
            println!("  ARB (vs sample on-chain ${:.0}): {}", onchain, s.summary());
            println!("    actionable: {}", if s.actionable { "✓ yes" } else { "no" });

            // the real combo (live fetch + arb in one call)
            if let Ok(snap) = snapshot(&symbol, onchain, 0.4) {
                println!("  combo snapshot(): last ${:.2} · spread {:+.3}%", snap.ticker.last, snap.arb.spread_pct);
            }
        }
        Err(e) => {
            println!("  ⚠ Binance price unavailable: {e}");
            println!("  (engines still work on supplied prices — see tests; the live feed needs egress to api.binance.com)");
        }
    }
    println!();
}
