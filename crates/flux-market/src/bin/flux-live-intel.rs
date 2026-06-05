//! flux-live-intel — the live intel combo: Binance × Fear&Greed × Polymarket.
//!
//! Pulls REAL data from three sources and fuses them into ONE propose-only
//! decision: Binance (live BTC price + 24h range = trend), the Crypto Fear&Greed
//! index (contrarian DCA sizing), and Polymarket "in between" (a risk-free
//! buy-both arb beats everything). Nothing is executed — it PROPOSES the action.

use flux_market::sim::{decide, Decision};
use flux_market::{binance, fear_greed, polymarket};

fn main() {
    println!("⚡ FLUX LIVE INTEL COMBO — Binance × Fear&Greed × Polymarket (propose-only)\n");

    // 1) Binance — live BTC price + 24h range (the trend reference)
    let btc = binance::ticker_24h("BTCUSDT");
    match &btc {
        Ok(t) => println!("  📈 Binance BTCUSDT: ${:.0}  (24h {:.0}–{:.0}, {:+.2}%)",
            t.last, t.low_24h, t.high_24h, t.change_pct_24h),
        Err(e) => println!("  📈 Binance: unreachable ({e})"),
    }

    // 2) Fear & Greed — contrarian sizing
    let fg = fear_greed::fetch();
    match &fg {
        Ok(f) => println!("  🐻 Fear&Greed: {} ({}) → DCA ×{:.2}", f.value, f.sentiment.label(), f.sentiment.dca_multiplier()),
        Err(e) => println!("  🐻 Fear&Greed: unreachable ({e})"),
    }

    // 3) Polymarket — the in-between risk-free arb signal
    let pm = polymarket::scan_arbs(1.0, 5);
    let best = match &pm {
        Ok(v) if !v.is_empty() => {
            let a = &v[0];
            let m = polymarket::arb_margin_pct(a.yes, a.no);
            println!("  🎲 Polymarket top arb: \"{}\" +{:.1}%", a.question, m);
            Some((a.question.as_str(), m))
        }
        Ok(_) => { println!("  🎲 Polymarket: no arb above threshold"); None }
        Err(e) => { println!("  🎲 Polymarket: unreachable ({e})"); None }
    };

    // FUSE → one propose-only decision
    println!("\n── FUSED PROPOSAL (no order placed) ──");
    if let Ok(t) = &btc {
        let trend = (t.high_24h + t.low_24h) / 2.0;
        let base_dca = 150.0;
        let dca = fg.as_ref().map(|f| f.sized_dca(base_dca)).unwrap_or(base_dca);
        let d = decide(t.last, trend, dca, 10_000.0, best, 2.0);
        match d {
            Decision::TakeArb { question, margin_pct } =>
                println!("  → TAKE ARB: \"{}\" (+{:.1}% risk-free) — beats DCA this tick", question, margin_pct),
            Decision::DcaBuy { usds, reason } =>
                println!("  → DCA-BUY ${:.0} of BTC  [{}]", usds, reason),
            Decision::Hold { reason } =>
                println!("  → HOLD  [{}]", reason),
        }
        println!("  (operator confirms before any real order — live+per-order-confirm)");
    } else {
        println!("  → cannot propose without a live price; retry when Binance reachable.");
    }
}
