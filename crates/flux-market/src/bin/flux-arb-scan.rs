//! flux-arb-scan — one propose-only arbitrage/DCA scan tick. Run on a loop by a
//! systemd timer. Live Binance price + live Polymarket arbs + a Carl-Runefelt
//! DCA verdict. PROPOSES only — appends to a log + a JSON feed, NEVER spends.

use flux_market::{polymarket, ticker_24h};
use std::io::Write;

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn append(path: &str, line: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{line}");
    }
}

fn main() {
    let log = std::env::var("FLUX_ARB_LOG").unwrap_or_else(|_| "/home/orobit/arb-scan.log".into());
    let feed = std::env::var("FLUX_ARB_FEED").unwrap_or_else(|_| "/home/orobit/q-narwhalknight/dist-final/downloads/arb-scan.json".into());
    let symbol = "BTCUSDT";
    let ts = now();

    let t = match ticker_24h(symbol) {
        Ok(t) => t,
        Err(e) => {
            append(&log, &format!("[{ts}] ⚠ binance scan failed: {e}"));
            eprintln!("binance: {e}");
            return;
        }
    };

    // Carl-Runefelt DCA verdict: buy the dip harder; never sell.
    let chg = t.change_pct_24h;
    let mult = if chg <= -30.0 { 3.0 } else if chg <= -20.0 { 2.0 } else if chg <= -10.0 { 1.5 } else { 1.0 };
    let dca = if mult > 1.0 {
        format!("DIP {chg:.1}% → DCA ×{mult} (buy fear)")
    } else {
        "DCA on schedule (accumulate, never sell)".to_string()
    };

    // Live Polymarket arbs (read-only).
    let bets = polymarket::scan_arbs(1.0, 60).unwrap_or_default();
    let top = bets.first().map(|b| format!("{:.1}% \"{}\"", b.margin_pct, b.question.chars().take(48).collect::<String>())).unwrap_or_else(|| "none".into());

    // Live Google-News sentiment (the Runefelt signal).
    let sent = flux_market::news::fetch_news_sentiment("bitcoin").unwrap_or(flux_market::news::Sentiment {
        score: 0.0, label: "n/a".into(), headlines: 0, bull_hits: 0, bear_hits: 0, sample: vec![],
    });

    let line = format!(
        "[{ts}] BTC ${:.0} ({:+.2}% 24h · H{:.0}/L{:.0}) · 🌙 {} · 📰 {} ({:+.2}, {}hl) · polymarket arbs: {} (best {}) · PROPOSE-ONLY",
        t.last, chg, t.high_24h, t.low_24h, dca, sent.label, sent.score, sent.headlines, bets.len(), top
    );
    append(&log, &line);
    println!("{line}");

    // Dashboard feed (atomic-ish).
    let json = format!(
        "{{\"ts\":{ts},\"btc_usd\":{:.2},\"chg_24h\":{:.2},\"dca_multiplier\":{mult},\"dca_verdict\":{:?},\"news_sentiment\":{:.3},\"news_label\":{:?},\"news_headlines\":{},\"polymarket_arb_count\":{},\"top_bet_arb_pct\":{:.2},\"mode\":\"propose-only\"}}",
        t.last, chg, dca, sent.score, sent.label, sent.headlines, bets.len(), bets.first().map(|b| b.margin_pct).unwrap_or(0.0)
    );
    let tmp = format!("{feed}.tmp");
    if std::fs::write(&tmp, &json).is_ok() {
        let _ = std::fs::rename(&tmp, &feed);
    }
}
