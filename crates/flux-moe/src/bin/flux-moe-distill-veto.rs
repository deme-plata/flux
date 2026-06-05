//! flux-moe-distill-veto — generate the VETO-STUDENT training corpus.
//!
//! Loops synthetic money scenarios (distill_goals() × a proposed tool-call, plus
//! adversarial wrong-tool variants) → the deepseek-v4-flash TEACHER labels each
//! APPROVE/VETO (teacher_label_gate) → to_veto_jsonl writes supervised pairs.
//! Fine-tune a 1-3B local veto-student on the output so the two_mind VETOER runs
//! offline, no API.
//!
//! PROPOSE-ONLY: with NO key it DRY-RUNS (prints scenarios + sample prompts, makes
//! zero network calls). It only contacts the teacher when DEEPSEEK_API_KEY (or
//! FLUX_MOE_API_KEY) is set — labelling without a key would fold every row to a
//! bogus Veto, so we refuse to write a corrupt corpus.
//!
//!   flux-moe-distill-veto [out.jsonl] [limit]
//!   DEEPSEEK_API_KEY=… flux-moe-distill-veto crates/flux-moe/veto-corpus.jsonl 30
use serde_json::{json, Value};
use flux_moe::distill::{distill_goals, teacher_label_gate, to_veto_jsonl, veto_prompt, VetoRow};
use flux_moe::GateVerdict;

const TOKENS: &[&str] = &["CLAI", "PACI", "SCALPEL", "USDS", "QUGUSD"];
const SYMBOLS: &[&str] = &["BTC", "ETH", "ETC", "SOL"];

fn first_amount(s: &str) -> u64 {
    s.split(|c: char| !c.is_ascii_digit()).find(|t| !t.is_empty()).and_then(|t| t.parse().ok()).unwrap_or(0)
}
fn recipient(s: &str) -> String {
    s.rsplit(" to ").next().unwrap_or("").split_whitespace().last().unwrap_or("chest").to_string()
}
fn find_in<'a>(s: &str, opts: &'a [&'a str], dflt: &'a str) -> &'a str {
    let u = s.to_uppercase();
    opts.iter().find(|o| u.contains(*o)).copied().unwrap_or(dflt)
}
fn read_tool(g: &str) -> &'static str {
    if g.contains("balance") { "get_balance" }
    else if g.contains("portfolio") { "portfolio_overview" }
    else if g.contains("pools") { "dex_list_pools" }
    else if g.contains("arbitrage") { "arb_scan" }
    else if g.contains("bridge") { "btc_bridge_status" }
    else if g.contains("deposit address") { "btc_generate_deposit_address" }
    else if g.contains("network") { "network_status" }
    else { "get_balance" }
}

/// The "natural" proposed (tool, args) a proposer might emit for a goal.
fn proposed_call(goal: &str) -> (String, Value) {
    let g = goal.to_lowercase();
    let amt = first_amount(goal);
    if g.starts_with("send") {
        ("send_qug".into(), json!({"to": recipient(goal), "amount": amt}))
    } else if g.starts_with("transfer") {
        ("send_token".into(), json!({"to": recipient(goal), "amount": amt, "token": find_in(goal, TOKENS, "CLAI")}))
    } else if g.starts_with("swap") {
        ("dex_swap".into(), json!({"from": find_in(goal, TOKENS, "QUG"), "to": "PACI", "amount": amt}))
    } else if g.starts_with("quote") {
        ("dex_get_quote".into(), json!({"from": find_in(goal, TOKENS, "QUG"), "to": "PACI", "amount": amt}))
    } else if g.starts_with("dca") || g.starts_with("route") {
        ("dex_swap".into(), json!({"from": "USDS", "to": "BTC", "amount": amt}))
    } else if g.contains("price") || g.contains("arb") {
        ("arb_scan".into(), json!({"symbol": find_in(goal, SYMBOLS, "BTC")}))
    } else if g.starts_with("show") {
        (read_tool(&g).into(), json!({}))
    } else {
        ("get_balance".into(), json!({}))
    }
}

/// An adversarial WRONG/unsafe proposal for some goals — proposing a real-money
/// send for a read-only or under-specified goal should draw a VETO. Gives the
/// student both labels instead of an all-APPROVE corpus.
fn adversarial(goal: &str) -> (String, Value) {
    // send real money with NO recipient + no mandate — should be vetoed.
    ("send_qug".into(), json!({"amount": first_amount(goal).max(1)}))
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "crates/flux-moe/veto-corpus.jsonl".into());
    let limit: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(30);

    // Build scenarios: each goal's natural call + every 3rd goal an adversarial twin.
    let goals = distill_goals();
    let mut scen: Vec<(String, String, Value)> = Vec::new();
    for (i, g) in goals.iter().enumerate() {
        let (tool, args) = proposed_call(g);
        scen.push((g.clone(), tool, args));
        if i % 3 == 0 {
            let (atool, aargs) = adversarial(g);
            scen.push((g.clone(), atool, aargs));
        }
    }
    eprintln!("[distill-veto] {} scenarios built from {} goals", scen.len(), goals.len());

    let have_key = std::env::var("DEEPSEEK_API_KEY").is_ok() || std::env::var("FLUX_MOE_API_KEY").is_ok();
    if !have_key {
        eprintln!("[distill-veto] DRY RUN — no DEEPSEEK_API_KEY (zero network calls). Sample prompts:");
        for (req, tool, args) in scen.iter().take(3) {
            println!("--- {tool} ---\n{}\n", veto_prompt(req, tool, args));
        }
        eprintln!("[distill-veto] set DEEPSEEK_API_KEY (+ optional limit arg) to label + write {out}");
        return;
    }

    // LIVE: label up to `limit` scenarios with the teacher.
    let n = scen.len().min(limit);
    eprintln!("[distill-veto] LIVE — labelling {n}/{} scenarios via deepseek-v4-flash teacher…", scen.len());
    let mut rows: Vec<VetoRow> = Vec::with_capacity(n);
    let (mut approve, mut veto) = (0usize, 0usize);
    for (i, (req, tool, args)) in scen.into_iter().take(n).enumerate() {
        let (verdict, _raw) = teacher_label_gate(&req, &tool, &args);
        match &verdict { GateVerdict::Approve => approve += 1, GateVerdict::Veto(_) => veto += 1 }
        eprintln!("  [{:>3}/{n}] {tool:<28} {}", i + 1, match &verdict { GateVerdict::Approve => "APPROVE".into(), GateVerdict::Veto(r) => format!("VETO: {}", r.chars().take(48).collect::<String>()) });
        rows.push(VetoRow { request: req, tool, args, verdict });
    }
    let jsonl = to_veto_jsonl(&rows);
    match std::fs::write(&out, &jsonl) {
        Ok(_) => eprintln!("[distill-veto] WROTE {} rows → {out}  ({approve} APPROVE / {veto} VETO)", rows.len()),
        Err(e) => { eprintln!("[distill-veto] write {out} failed: {e}"); std::process::exit(1); }
    }
}
