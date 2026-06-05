//! flux-sigil-trade-sim — the first SIGIL AI agentic trade sim (paper, propose-only).
//!
//! Runs the Carl-Runefelt DCA + Polymarket-arb agent over a BTC price path,
//! paper-executes, prints the journal + PnL, and emits the decisions as agentic
//! training data (state → tool-call) for the flux-moe corpus ("then train llm").
//! Also does ONE live MCP-combo step against the real Binance ticker if reachable.
//!
//!   flux-sigil-trade-sim [out.jsonl]

use flux_market::sim::{decide, run_sim, Decision};
use flux_market::{binance, polymarket};

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "trade-sim-journal.jsonl".into());

    // A BTC dip→recover path (reproducible). Injected Polymarket arb at step 4.
    let prices = [73_000.0, 71_500.0, 68_000.0, 65_000.0, 63_500.0, 66_000.0, 69_000.0, 72_000.0, 74_500.0, 76_000.0];
    let arbs: Vec<Option<(&str, f64)>> = (0..prices.len())
        .map(|i| if i == 4 { Some(("Will BTC close above 70k this month?", 3.2)) } else { None })
        .collect();

    let r = run_sim(&prices, 1000.0, 150.0, 3, 2.0, &arbs);

    println!("╭─ SIGIL AI agentic trade sim — paper, propose-only ─────────────────╮");
    println!("│ step  price     SMA      decision                              value");
    for t in &r.journal {
        let d = match &t.decision {
            Decision::DcaBuy { usds, .. } => format!("DCA-buy ${usds:.0} (dip)"),
            Decision::TakeArb { margin_pct, .. } => format!("take arb +{margin_pct:.1}%"),
            Decision::Hold { .. } => "hold core".to_string(),
        };
        println!("│  {:>2}   {:>8.0}  {:>8.0}  {:<38} {:>8.0}", t.step, t.price, t.sma, d, t.portfolio_value);
    }
    println!("╰────────────────────────────────────────────────────────────────────╯");
    println!("start ${:.0} → final ${:.0}  ({:+.1}%)  |  BTC accumulated {:.5}  |  {} dip-buys, {} arbs",
        r.start_value, r.final_value, r.pnl_pct(), r.btc_accumulated, r.buys, r.arbs);

    // emit the journal as agentic training examples (state → tool-call)
    let mut jsonl = String::new();
    for t in &r.journal {
        let (tool, args) = match &t.decision {
            Decision::DcaBuy { usds, .. } => ("btc_dca_buy", format!("{{\"amount\":\"{usds:.0}\"}}")),
            Decision::TakeArb { question, .. } => ("polymarket_scan", format!("{{\"question\":{question:?}}}")),
            Decision::Hold { .. } => ("market_scan", "{\"symbol\":\"BTCUSDT\"}".to_string()),
        };
        let goal = format!("BTC is {:.0} vs trend {:.0}; cash on hand. What do I do?", t.price, t.sma);
        jsonl.push_str(&format!(
            "{{\"messages\":[{{\"role\":\"user\",\"content\":{goal:?}}},{{\"role\":\"assistant\",\"tool_calls\":[{{\"type\":\"function\",\"function\":{{\"name\":\"{tool}\",\"arguments\":{args:?}}}}}]}}]}}\n"
        ));
    }
    match std::fs::write(&out, &jsonl) {
        Ok(_) => println!("✓ emitted {} agentic trade examples → {out} (feeds flux-moe training)", r.journal.len()),
        Err(e) => eprintln!("write {out}: {e}"),
    }

    // one LIVE MCP-combo step against the real Binance ticker (propose-only)
    println!("\n── live MCP combo step (real Binance + Polymarket) ──");
    match binance::ticker_24h("BTCUSDT") {
        Ok(t) => {
            let arbs = polymarket::scan_arbs(2.0, 5).unwrap_or_default();
            let best = arbs.first().map(|a| (a.question.as_str(), polymarket::arb_margin_pct(a.yes, a.no)));
            let trend = (t.high_24h + t.low_24h) / 2.0; // 24h midpoint as trend ref
            let d = decide(t.last, trend, 150.0, 1000.0, best, 2.0);
            println!("live BTC ${:.0} (24h mid ${:.0}) → PROPOSE: {:?}", t.last, trend, d.tool());
            println!("  {d:?}");
        }
        Err(e) => println!("(Binance unreachable from here: {e} — deterministic sim above is the proof)"),
    }
}
