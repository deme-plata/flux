//! flux-100-agents — 100 Qwen-policy trading agents, in chronos, SIGIL-settled.
//!
//! Spawns 100 [`Trader`]s (varied styles + DCA sizes), runs them over a full
//! market cycle in chronos virtual time, each buy modeled as a SIGIL swap
//! (sigil-rpc `execute_swap` chokepoint — sim). Propose-only, no real funds.
//! Real deployment = `flux_nodeswarm_spawn` 100 light agents → a shared served
//! Qwen3.6 endpoint + flux-p2p gossip; this proves the swarm economics instantly.
//!
//! The cycle ends mid-recovery (a FAIR test, not the ends-at-trough worst case).

use flux_market::multi_agent::{run_swarm, Style, Trader};

fn main() {
    // 52-week cycle: 70k → 40k trough (week ~17) → recover to ~78k by week 51.
    let mut prices = Vec::new();
    let mut fng = Vec::new();
    for w in 0..52i32 {
        let price = if w <= 17 {
            70_000.0 - (70_000.0 - 40_000.0) * (w as f64 / 17.0) // decline
        } else {
            40_000.0 + (78_000.0 - 40_000.0) * ((w - 17) as f64 / (51 - 17) as f64) // recovery
        };
        let f = (((price - 40_000.0) / (78_000.0 - 40_000.0)) * 88.0 + 6.0).round().clamp(0.0, 100.0) as u8;
        prices.push(price);
        fng.push(f);
    }

    // 100 agents: round-robin style, DCA sized 60..260, all start with 10k USDS.
    let mut traders = Vec::with_capacity(100);
    for i in 0..100u32 {
        let style = match i % 3 { 0 => Style::Contrarian, 1 => Style::Balanced, _ => Style::Trend };
        let dca = 60.0 + (i % 6) as f64 * 40.0;
        traders.push(Trader::new(format!("qwen-agent-{i:03}"), 10_000.0, dca, style));
    }
    let n = traders.len();

    let (board, gossip) = run_swarm(traders, &prices, &fng, 8);

    let total_start = 10_000.0 * n as f64;
    let total_final: f64 = board.iter().map(|r| r.final_value).sum();
    let total_btc: f64 = board.iter().map(|r| r.btc).sum();
    let avg = |s: Style| {
        let v: Vec<f64> = board.iter().filter(|r| r.style == s).map(|r| r.pnl_pct).collect();
        v.iter().sum::<f64>() / v.len().max(1) as f64
    };
    let hodl = (prices[51] / prices[0] - 1.0) * 100.0;

    println!("⚡ 100 QWEN TRADING AGENTS — chronos virtual time, SIGIL-settled (sim, propose-only)");
    println!("   cycle 70k→40k→78k (52wk) · {} agents · {} SIGIL swaps gossiped\n", n, gossip.len());
    println!("   aggregate: ${:.0} → ${:.0}  ({:+.1}%) · {:.3} BTC stacked total", total_start, total_final, (total_final-total_start)/total_start*100.0, total_btc);
    println!("   buy&hold this cycle: {:+.1}%", hodl);
    println!("   by style: contrarian {:+.1}% · balanced {:+.1}% · trend {:+.1}%\n", avg(Style::Contrarian), avg(Style::Balanced), avg(Style::Trend));

    println!("   🏆 top 5:");
    for r in board.iter().take(5) {
        println!("     {:<16} {:+6.1}%  {:.4} BTC  ({:?})", r.id, r.pnl_pct, r.btc, r.style);
    }
    println!("   🪦 bottom 3:");
    for r in board.iter().rev().take(3) {
        println!("     {:<16} {:+6.1}%  {:.4} BTC  ({:?})", r.id, r.pnl_pct, r.btc, r.style);
    }

    let winner = &board[0];
    println!("\n──  VERDICT  ──");
    println!("   Best agent: {} ({:+.1}%, {:?}). Paper, not real money — chronos virtual time,",
        winner.id, winner.pnl_pct, winner.style);
    println!("   every 'buy' is a simulated SIGIL execute_swap. The strategy that wins is the one");
    println!("   that bought the {}.", if avg(Style::Contrarian) >= avg(Style::Trend) { "fear (contrarian)" } else { "momentum (trend)" });
}
