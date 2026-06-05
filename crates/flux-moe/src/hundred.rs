//! hundred.rs — **"The Hundred"**: a webhook-driven swarm of LLM trading agents,
//! each given an MCP tool-call *superpower* behind a **Verified Execution Gate**.
//!
//! The invention isn't "run 100 Qwens" — it's the GATE that makes an LLM *safe*
//! to hand real money tools. Born from the 2026-06-01 A100 lesson: a 32b model
//! walked into an 88.91% **rt-loss** honeypot by misreading "rt-loss" as
//! "return". The Hundred can't do that — every tool-call a Qwen emits passes
//! four guards before anything executes, and every decision/outcome is announced
//! over **webhooks** (the swarm's nervous system: events wake agents, outcomes
//! report back to the cockpit feed).
//!
//! LLM is pluggable: agents call an Ollama endpoint (the A100) via a closure, so
//! the gate + webhooks are testable with zero GPU, and the brains plug in with
//! one env var.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// A market pool agents can see + act on.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pool {
    pub sym: String,
    pub price: f64,
    /// Round-trip loss % (slippage + fee + honeypot tax) if you buy+sell now.
    /// HIGH = trap. This is the field a naive LLM misreads as "return".
    pub rt_loss_pct: f64,
    pub liquidity: f64,
}

/// The MCP tool-call an agent wants executed. The typed enum IS the tool
/// whitelist — an LLM can't conjure an arbitrary call.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "tool", rename_all = "snake_case")]
pub enum Action {
    Hold,
    DexSwap { pool: String, amount: f64 },
}

/// A tool-call as emitted by an agent, carrying its stated reason (which the
/// gate inspects for the loss-as-gain misread).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCall {
    pub agent: String,
    pub action: Action,
    pub reason: String,
}

/// Gate verdict.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Verdict {
    Allow,
    Block { guard: &'static str, why: String },
}

impl Verdict {
    pub fn allowed(&self) -> bool {
        matches!(self, Verdict::Allow)
    }
}

/// Pools losing ≥ this round-trip are honeypots — never buy in.
pub const HONEYPOT_RT_LOSS: f64 = 50.0;
/// Never swap more than this fraction of a pool's liquidity (slippage cap).
pub const MAX_POOL_FRACTION: f64 = 0.10;

/// **THE SUPERPOWER** — the Verified Execution Gate. Every agent tool-call passes
/// these guards BEFORE anything executes. This is what lets you hand an LLM real
/// money tools without it rugging itself.
pub fn gate(call: &ToolCall, market: &[Pool], stake: f64) -> Verdict {
    let (pool_sym, amount) = match &call.action {
        Action::Hold => return Verdict::Allow, // holding is always safe
        Action::DexSwap { pool, amount } => (pool, *amount),
    };
    // Guard 0 — whitelist: enforced by the typed Action enum + known pool.
    let pool = match market.iter().find(|p| &p.sym == pool_sym) {
        Some(p) => p,
        None => {
            return Verdict::Block { guard: "whitelist", why: format!("unknown pool '{pool_sym}'") }
        }
    };
    // Guard 1 — HONEYPOT: never buy into a high round-trip-loss pool.
    if pool.rt_loss_pct >= HONEYPOT_RT_LOSS {
        return Verdict::Block {
            guard: "honeypot",
            why: format!(
                "pool {} has {:.2}% ROUND-TRIP LOSS (≥{:.0}% trap floor) — a loss, NOT a return",
                pool.sym, pool.rt_loss_pct, HONEYPOT_RT_LOSS
            ),
        };
    }
    // Guard 1b — LOSS-AS-GAIN misread (the exact A100 failure): if the reason
    // cites the pool's loss number AS upside, block + relabel.
    let cites_number = call.reason.contains(&format!("{:.2}", pool.rt_loss_pct))
        || call.reason.contains(&format!("{:.0}", pool.rt_loss_pct));
    let r = call.reason.to_lowercase();
    let as_gain = r.contains("return") || r.contains("gain") || r.contains("profit") || r.contains("highest");
    if pool.rt_loss_pct > 0.0 && cites_number && as_gain {
        return Verdict::Block {
            guard: "loss-as-gain",
            why: format!(
                "reason reads {:.2}% as upside but it's a LOSS metric (rt-loss) — label losses unambiguously",
                pool.rt_loss_pct
            ),
        };
    }
    // Guard 2 — SIZE/CAP: can't bet more than you hold, can't move the pool.
    if !(amount > 0.0) {
        return Verdict::Block { guard: "size", why: "amount must be > 0".into() };
    }
    if amount > stake {
        return Verdict::Block { guard: "size", why: format!("amount {amount} exceeds stake {stake}") };
    }
    if amount > pool.liquidity * MAX_POOL_FRACTION {
        return Verdict::Block {
            guard: "slippage",
            why: format!("amount {amount} > {:.0}% of pool liquidity {}", MAX_POOL_FRACTION * 100.0, pool.liquidity),
        };
    }
    Verdict::Allow
}

/// Paper-trade an ALLOWED action → realized pnl delta (sim; no real funds move).
/// Healthy pools (low rt-loss) yield a small positive edge; lossy pools bleed.
pub fn paper_pnl(action: &Action, market: &[Pool]) -> f64 {
    match action {
        Action::Hold => 0.0,
        Action::DexSwap { pool, amount } => market
            .iter()
            .find(|x| &x.sym == pool)
            .map(|p| amount * (-p.rt_loss_pct / 100.0 + 0.01)) // -loss + tiny maker edge
            .unwrap_or(0.0),
    }
}

/// One agent in the swarm.
#[derive(Clone, Debug, Serialize)]
pub struct Agent {
    pub name: String,
    pub persona: String,
    pub stake: f64,
    pub pnl: f64,
    pub trades: u32,
    pub blocked: u32,
}

impl Agent {
    pub fn new(name: impl Into<String>, persona: impl Into<String>, stake: f64) -> Self {
        Agent { name: name.into(), persona: persona.into(), stake, pnl: 0.0, trades: 0, blocked: 0 }
    }
}

/// Now in ms — for webhook event stamping.
pub fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

/// Fire a webhook (best-effort POST JSON). Returns the HTTP status or an error.
/// This is the swarm's nervous system — every gate decision + trade outcome is
/// announced so the cockpit / sibling agents can react.
pub fn fire_webhook(url: &str, body: &serde_json::Value) -> Result<u16, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;
    let r = client.post(url).json(body).send().map_err(|e| e.to_string())?;
    Ok(r.status().as_u16())
}

/// Run ONE agent's turn: it emits a tool-call (via the injected LLM closure),
/// the gate verifies it, and on Allow we paper-trade. Returns (ToolCall, Verdict,
/// pnl_delta). Fires a webhook per turn if `webhook` is Some.
pub fn agent_turn<F>(
    agent: &mut Agent,
    market: &[Pool],
    decide: &F,
    webhook: Option<&str>,
) -> (ToolCall, Verdict, f64)
where
    F: Fn(&Agent, &[Pool]) -> ToolCall,
{
    let call = decide(agent, market);
    let verdict = gate(&call, market, agent.stake);
    let mut pnl_delta = 0.0;
    if verdict.allowed() {
        pnl_delta = paper_pnl(&call.action, market);
        agent.pnl += pnl_delta;
        agent.trades += 1;
    } else {
        agent.blocked += 1;
    }
    if let Some(url) = webhook {
        let ev = serde_json::json!({
            "kind": if verdict.allowed() { "decision" } else { "blocked" },
            "agent": agent.name,
            "action": call.action,
            "reason": call.reason,
            "verdict": verdict,
            "pnl_delta": pnl_delta,
            "agent_pnl": agent.pnl,
            "ts_ms": now_ms(),
        });
        let _ = fire_webhook(url, &ev); // best-effort; swarm shouldn't stall on a slow sink
    }
    (call, verdict, pnl_delta)
}

/// Evolutionary step: sort by pnl, replace the bottom `cull` fraction's personas
/// with mutations of the top performers, reset their pnl. "Trading-agent natural
/// selection on (paper) money." Returns the number replaced.
pub fn evolve(agents: &mut [Agent], cull: f64) -> usize {
    if agents.len() < 2 {
        return 0;
    }
    agents.sort_by(|a, b| b.pnl.partial_cmp(&a.pnl).unwrap_or(std::cmp::Ordering::Equal));
    let n = agents.len();
    let k = ((n as f64) * cull).floor() as usize;
    let k = k.max(1).min(n / 2);
    for i in 0..k {
        let top = agents[i % (n - k)].persona.clone(); // a survivor's persona
        let loser = &mut agents[n - 1 - i];
        loser.persona = format!("{top} · mutated");
        loser.pnl = 0.0;
        loser.trades = 0;
        loser.blocked = 0;
    }
    k
}

#[cfg(test)]
mod tests {
    use super::*;

    fn market() -> Vec<Pool> {
        vec![
            Pool { sym: "USDS/wQUG".into(), price: 1.0, rt_loss_pct: 0.6, liquidity: 100_000.0 },
            // the honeypot from the A100 lesson:
            Pool { sym: "QUG/MOON".into(), price: 0.001, rt_loss_pct: 88.91, liquidity: 5_000.0 },
        ]
    }

    #[test]
    fn honeypot_swap_is_blocked() {
        let m = market();
        let call = ToolCall {
            agent: "greedy".into(),
            action: Action::DexSwap { pool: "QUG/MOON".into(), amount: 100.0 },
            reason: "diversify into MOON".into(),
        };
        match gate(&call, &m, 1000.0) {
            Verdict::Block { guard, .. } => assert_eq!(guard, "honeypot"),
            v => panic!("expected honeypot block, got {v:?}"),
        }
    }

    #[test]
    fn loss_read_as_return_is_blocked() {
        // exactly the 32b failure: cites 88.91 as "highest return"
        let m = market();
        let call = ToolCall {
            agent: "qwen32b".into(),
            action: Action::DexSwap { pool: "QUG/MOON".into(), amount: 50.0 },
            reason: "QUG/MOON has the highest return at 88.91%".into(),
        };
        // honeypot guard catches it first (rt_loss >= 50) — both guards agree it's a trap.
        assert!(!gate(&call, &m, 1000.0).allowed());
    }

    #[test]
    fn loss_as_gain_guard_fires_below_honeypot_floor() {
        // a pool under the 50% honeypot floor but the reason still misreads loss as gain
        let m = vec![Pool { sym: "SCAM/QUG".into(), price: 1.0, rt_loss_pct: 12.0, liquidity: 100_000.0 }];
        let call = ToolCall {
            agent: "qwen".into(),
            action: Action::DexSwap { pool: "SCAM/QUG".into(), amount: 10.0 },
            reason: "best return, 12% gain".into(),
        };
        match gate(&call, &m, 1000.0) {
            Verdict::Block { guard, .. } => assert_eq!(guard, "loss-as-gain"),
            v => panic!("expected loss-as-gain block, got {v:?}"),
        }
    }

    #[test]
    fn oversized_swap_is_blocked() {
        let m = market();
        let call = ToolCall {
            agent: "whale".into(),
            action: Action::DexSwap { pool: "USDS/wQUG".into(), amount: 50_000.0 }, // > 10% of 100k
            reason: "go big".into(),
        };
        match gate(&call, &m, 1_000_000.0) {
            Verdict::Block { guard, .. } => assert_eq!(guard, "slippage"),
            v => panic!("expected slippage block, got {v:?}"),
        }
    }

    #[test]
    fn healthy_swap_is_allowed_and_profits() {
        let m = market();
        let call = ToolCall {
            agent: "prudent".into(),
            action: Action::DexSwap { pool: "USDS/wQUG".into(), amount: 100.0 },
            reason: "deep pool, low round-trip loss".into(),
        };
        assert!(gate(&call, &m, 1000.0).allowed());
        // healthy pool (0.6% loss) + 1% maker edge → small positive pnl
        assert!(paper_pnl(&call.action, &m) > 0.0);
    }

    #[test]
    fn hold_always_allowed() {
        let m = market();
        let call = ToolCall { agent: "x".into(), action: Action::Hold, reason: "wait".into() };
        assert!(gate(&call, &m, 0.0).allowed());
    }

    #[test]
    fn evolve_replaces_losers() {
        let mut agents = vec![
            Agent { name: "a".into(), persona: "winner".into(), stake: 100.0, pnl: 50.0, trades: 5, blocked: 0 },
            Agent { name: "b".into(), persona: "mid".into(), stake: 100.0, pnl: 0.0, trades: 1, blocked: 1 },
            Agent { name: "c".into(), persona: "loser".into(), stake: 100.0, pnl: -30.0, trades: 2, blocked: 3 },
            Agent { name: "d".into(), persona: "loser2".into(), stake: 100.0, pnl: -40.0, trades: 1, blocked: 4 },
        ];
        let replaced = evolve(&mut agents, 0.25);
        assert!(replaced >= 1);
        // the worst now carries a mutated survivor persona
        assert!(agents.last().unwrap().persona.contains("mutated"));
    }
}
