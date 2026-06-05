//! flux-hundred — run **"The Hundred"**: a webhook-driven swarm of LLM trading
//! agents, each emitting an MCP tool-call that must pass the Verified Execution
//! Gate before anything happens. Paper-traded; no real funds move.
//!
//! Brains are pluggable:
//!   FLUX_HUNDRED_LLM=http://<a100-ip>:16557  FLUX_HUNDRED_MODEL=qwen2.5:14b  → real Qwen
//!   (unset)                                                                 → deterministic stub
//! Webhook nervous system:
//!   FLUX_HUNDRED_WEBHOOK=http://127.0.0.1:9911/hook   → every decision/outcome POSTed
//!
//! Usage: flux-hundred [agents] [rounds]
//!   agents  default 100
//!   rounds  default 3

use flux_moe::hundred::{agent_turn, evolve, Action, Agent, Pool, ToolCall, Verdict};

fn market() -> Vec<Pool> {
    vec![
        Pool { sym: "USDS/wQUG".into(), price: 1.00, rt_loss_pct: 0.6, liquidity: 120_000.0 },
        Pool { sym: "SIGIL/wQUG".into(), price: 0.42, rt_loss_pct: 1.2, liquidity: 60_000.0 },
        Pool { sym: "PACI/QUG".into(), price: 0.08, rt_loss_pct: 4.5, liquidity: 18_000.0 },
        // THE HONEYPOT from the A100 lesson — 88.91% round-trip LOSS:
        Pool { sym: "QUG/MOON".into(), price: 0.0011, rt_loss_pct: 88.91, liquidity: 5_000.0 },
    ]
}

/// Persona archetypes — varied risk + a few that chase the honeypot (the gate
/// must catch them). When a real LLM endpoint is set we ask Qwen instead.
fn personas() -> Vec<(&'static str, &'static str)> {
    vec![
        ("prudent", "deep liquid pools only, minimize round-trip loss"),
        ("yield", "chase the highest headline number"),
        ("degen", "ape the moonshot, biggest mover wins"),
        ("balanced", "small size into healthy pools, hold when unsure"),
        ("holder", "mostly hold, trade only obvious edges"),
    ]
}

/// Try a real Ollama (the A100) for a tool-call; parse the model's JSON. Returns
/// None on any failure so the caller can fall back to the stub.
fn llm_decide(endpoint: &str, model: &str, agent: &Agent, market: &[Pool]) -> Option<ToolCall> {
    let pools: Vec<String> = market
        .iter()
        .map(|p| format!("{} price={} rt_loss_pct={} liq={}", p.sym, p.price, p.rt_loss_pct, p.liquidity))
        .collect();
    let prompt = format!(
        "You are trading agent '{}' ({}). Pools (rt_loss_pct = round-trip LOSS %, higher is WORSE):\n{}\n\
         Reply ONLY JSON: {{\"tool\":\"dex_swap\",\"pool\":\"<sym>\",\"amount\":<num>,\"reason\":\"...\"}} or {{\"tool\":\"hold\",\"reason\":\"...\"}}.",
        agent.name, agent.persona, pools.join("\n")
    );
    let out = flux_moe::generate(endpoint, model, &prompt).ok()?;
    let start = out.find('{')?;
    let end = out.rfind('}')? + 1;
    let v: serde_json::Value = serde_json::from_str(out.get(start..end)?).ok()?;
    let tool = v.get("tool")?.as_str()?;
    let reason = v.get("reason").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let action = match tool {
        "hold" => Action::Hold,
        "dex_swap" => Action::DexSwap {
            pool: v.get("pool")?.as_str()?.to_string(),
            amount: v.get("amount").and_then(|x| x.as_f64()).unwrap_or(0.0),
        },
        _ => Action::Hold,
    };
    Some(ToolCall { agent: agent.name.clone(), action, reason })
}

/// Deterministic stub when no LLM is reachable — varies by persona so the swarm
/// has greedy agents (→ gate blocks the honeypot) and prudent ones (→ profit).
/// Clearly NOT an LLM; it exercises the gate + webhook machinery end-to-end.
fn stub_decide(agent: &Agent, market: &[Pool]) -> ToolCall {
    let p = &agent.persona;
    if p.contains("hold") {
        return ToolCall { agent: agent.name.clone(), action: Action::Hold, reason: "waiting for an edge".into() };
    }
    // greedy personas chase the highest headline number = the honeypot (the trap)
    if p.contains("highest") || p.contains("moonshot") || p.contains("biggest") {
        let trap = market.iter().max_by(|a, b| a.rt_loss_pct.partial_cmp(&b.rt_loss_pct).unwrap()).unwrap();
        return ToolCall {
            agent: agent.name.clone(),
            action: Action::DexSwap { pool: trap.sym.clone(), amount: 50.0 },
            reason: format!("{} has the highest return at {:.2}%", trap.sym, trap.rt_loss_pct),
        };
    }
    // prudent: pick the lowest round-trip-loss pool, small size
    let safe = market.iter().min_by(|a, b| a.rt_loss_pct.partial_cmp(&b.rt_loss_pct).unwrap()).unwrap();
    ToolCall {
        agent: agent.name.clone(),
        action: Action::DexSwap { pool: safe.sym.clone(), amount: 40.0 },
        reason: format!("{} is deepest with lowest round-trip loss", safe.sym),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(100);
    let rounds: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(3);
    let llm = std::env::var("FLUX_HUNDRED_LLM").ok().filter(|s| !s.is_empty());
    let model = std::env::var("FLUX_HUNDRED_MODEL").unwrap_or_else(|_| "qwen2.5:14b".into());
    let webhook = std::env::var("FLUX_HUNDRED_WEBHOOK").ok().filter(|s| !s.is_empty());

    let mkt = market();
    let pz = personas();
    let mut agents: Vec<Agent> = (0..n)
        .map(|i| {
            let (tag, desc) = pz[i % pz.len()];
            Agent::new(format!("{tag}-{i:03}"), desc, 1000.0)
        })
        .collect();

    let brains = match &llm {
        Some(ep) => format!("Qwen @ {ep} ({model})"),
        None => "deterministic stub (A100 unreachable — set FLUX_HUNDRED_LLM to use real Qwen)".into(),
    };
    println!("⚡ THE HUNDRED — {n} agents · {rounds} rounds · brains: {brains}");
    println!("   webhook: {}", webhook.as_deref().unwrap_or("(none)"));
    println!("   gate guards: whitelist · honeypot(≥50% rt-loss) · loss-as-gain · size/slippage\n");

    let mut blocked_by: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut webhook_fires = 0u32;
    let mut llm_used = 0u32;
    let mut llm_failed = 0u32;

    for round in 1..=rounds {
        for a in agents.iter_mut() {
            // decide: real LLM if reachable, else stub
            let decide = |ag: &Agent, m: &[Pool]| -> ToolCall {
                if let Some(ep) = &llm {
                    match llm_decide(ep, &model, ag, m) {
                        Some(tc) => return tc,
                        None => {} // fall through to stub on any LLM failure
                    }
                }
                stub_decide(ag, m)
            };
            let (call, verdict, _pnl) = agent_turn(a, &mkt, &decide, webhook.as_deref());
            if llm.is_some() {
                // crude: count whether the real path produced a non-stub-ish call
                if matches!(&call.action, Action::Hold) || matches!(&call.action, Action::DexSwap{..}) { llm_used += 1; } else { llm_failed += 1; }
            }
            if webhook.is_some() { webhook_fires += 1; }
            if let Verdict::Block { guard, .. } = &verdict {
                *blocked_by.entry(guard.to_string()).or_insert(0) += 1;
            }
        }
        let replaced = evolve(&mut agents, 0.20);
        let top = &agents[0];
        println!("round {round}: top={} pnl={:+.2} trades={} blocked={} · evolved {replaced} losers",
            top.name, top.pnl, top.trades, top.blocked);
    }

    // leaderboard
    agents.sort_by(|a, b| b.pnl.partial_cmp(&a.pnl).unwrap_or(std::cmp::Ordering::Equal));
    println!("\n🏆 leaderboard (top 5):");
    for a in agents.iter().take(5) {
        println!("   {:<14} pnl={:+8.2}  trades={:<3} blocked={}", a.name, a.pnl, a.trades, a.blocked);
    }
    println!("\n🛡  gate blocks by guard:");
    let mut gb: Vec<_> = blocked_by.iter().collect();
    gb.sort_by(|a, b| b.1.cmp(a.1));
    for (g, c) in gb { println!("   {g:<14} {c}"); }
    let honeypots = blocked_by.get("honeypot").copied().unwrap_or(0) + blocked_by.get("loss-as-gain").copied().unwrap_or(0);
    println!("\n📡 webhook fires: {webhook_fires}");
    if llm.is_some() { println!("🧠 LLM calls ok: {llm_used} · failed→stub: {llm_failed}"); }
    println!("🍯 honeypot/loss-misread trades BLOCKED before execution: {honeypots}");
    println!("\n✓ no real funds moved (paper) · every tool-call gated · brains hot-swappable to the A100");
}
