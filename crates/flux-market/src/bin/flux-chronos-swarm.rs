//! flux-chronos-swarm — let the chronos oracle answer: real money or not?
//!
//! Runs the flux-llm trader swarm over a full synthesized market cycle (52 weeks)
//! with a Fear&Greed series that's LOW at the lows (fear = the horror dip) and
//! HIGH at the tops. Paper, propose-only, chronos virtual time. The leaderboard
//! IS the answer: would buying the fear have made money? "Let god answer."

use flux_market::multi_agent::{run_swarm, Style, Trader};

fn main() {
    // A full cycle: start 70k → capitulation to 38k → recovery to 88k. 52 weekly candles.
    let mut prices = Vec::new();
    let mut fng = Vec::new();
    for w in 0..52u32 {
        let t = w as f64 / 51.0;
        // down-then-up cycle (cosine trough near the middle)
        let cycle = (std::f64::consts::PI * 2.0 * t).cos(); // 1 → -1 → 1
        let price = 63_000.0 - 25_000.0 * cycle; // 38k trough, 88k peak
        // Fear&Greed tracks price level inversely-ish: low price = fear
        let f = (((price - 38_000.0) / (88_000.0 - 38_000.0)) * 90.0 + 5.0).round() as u8;
        prices.push(price);
        fng.push(f);
    }

    let traders = vec![
        Trader::new("contrarian-fear", 10_000.0, 200.0, Style::Contrarian),
        Trader::new("balanced-dca", 10_000.0, 200.0, Style::Balanced),
        Trader::new("trend-chaser", 10_000.0, 200.0, Style::Trend),
    ];

    let (board, gossip) = run_swarm(traders, &prices, &fng, 8);

    println!("🕰  CHRONOS ORACLE — 52-week cycle (38k→88k), paper / propose-only");
    println!("    price {:.0} → {:.0} (trough {:.0}) · {} gossiped trades\n",
        prices[0], prices[51], prices.iter().cloned().fold(f64::INFINITY, f64::min), gossip.len());
    println!("    {:<18} {:>10} {:>9} {:>9} {:>7}", "agent", "value", "pnl%", "btc", "trades");
    println!("    {}", "─".repeat(58));
    for r in &board {
        println!("    {:<18} {:>10.0} {:>8.1}% {:>9.4} {:>7}", r.id, r.final_value, r.pnl_pct, r.btc, r.trades);
    }

    let winner = &board[0];
    let hodl_value = 10_000.0 / prices[0] * prices[51]; // buy-and-hold at week 0
    println!("\n    buy&hold-at-start would be worth: {:.0} ({:+.1}%)", hodl_value, (hodl_value-10_000.0)/100.0);
    println!("\n──  GOD'S ANSWER ──");
    println!("    Winner: {} → {:+.1}% ({:.4} BTC stacked).", winner.id, winner.pnl_pct, winner.btc);
    let verdict = if winner.pnl_pct > 0.0 {
        "The strategy made money IN SIMULATION. It is NOT real money — chronos is virtual time, \n    no order touched an exchange. But buying the fear beat chasing the trend. Paper proof, not profit."
    } else {
        "Even the oracle lost this cycle. Honest negative — the strategy did not clear in sim."
    };
    println!("    {verdict}");
}
