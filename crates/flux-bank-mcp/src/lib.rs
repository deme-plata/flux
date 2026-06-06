//! flux-bank-mcp — agent combos (read + propose dry-run + bifrost skeleton).

use flux_bank_bridge::bank_status;
use flux_bank_core::{Ledger, TransferProposal, NATIVE};

pub fn flux_bank_status(endpoint: Option<&str>) -> String {
    let alias = endpoint.unwrap_or("quillon");
    let resp = bank_status(alias);
    serde_json::to_string_pretty(&resp).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
}

pub fn flux_bank_propose_transfer(
    from: &str,
    to: &str,
    amount_uqug: u128,
    token: Option<&str>,
    memo: Option<&str>,
) -> String {
    let p = TransferProposal {
        from: from.into(),
        to: to.into(),
        token: token.unwrap_or(NATIVE).into(),
        amount_uqug,
        memo: memo.map(str::to_string),
        dry_run: true,
    };
    let ledger = Ledger::new();
    let body = match ledger.simulate_transfer(&p) {
        Ok(()) => serde_json::json!({
            "ok": true,
            "mode": "dry_run",
            "from": p.from,
            "to": p.to,
            "token": p.token,
            "amount_uqug": p.amount_uqug,
            "memo": p.memo,
            "note": "proposal only — execute requires SignedIntent + 2-of-2"
        }),
        Err(e) => serde_json::json!({
            "ok": false,
            "mode": "dry_run",
            "from": p.from,
            "to": p.to,
            "token": p.token,
            "amount_uqug": p.amount_uqug,
            "error": e.to_string()
        }),
    };
    body.to_string()
}

fn bifrost_steps_json() -> serde_json::Value {
    serde_json::json!([
        {"step": 1, "tool": "flux_goal_post", "note": "post intent"},
        {"step": 2, "tool": "flux_swarm_claim", "note": "claim crate/files"},
        {"step": 3, "tool": "flux-moe", "note": "route model/lane"},
        {"step": 4, "tool": "flux_vast_recommend", "note": "ask_id only if budget OK"},
        {"step": 5, "tool": "flux_vast_create", "note": "operator confirms + autostop"},
        {"step": 6, "tool": "flux_combo", "note": "build/test gate"},
        {"step": 7, "tool": "verify_on_chain", "note": "money/code proof"},
        {"step": 8, "tool": "flux_swarm_complete", "note": "QUG settle"},
        {"step": 9, "tool": "flux_activity_tail", "note": "learn loop"},
    ])
}

pub fn flux_bifrost_run(goal: &str, agent_id: &str) -> String {
    serde_json::json!({
        "goal": goal,
        "agent_id": agent_id,
        "loop": "bifrost",
        "proposal_only": true,
        "steps": bifrost_steps_json(),
        "bank_combo": "flux_bank_status -> flux_bank_propose_transfer (dry) -> ...",
        "spend_gate": "flux_vast_recommend returns ask_id; flux_vast_create requires operator",
        "next": ["flux_goal_post", "flux_swarm_claim", "flux_bank_status"]
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn propose_dry_run_json() {
        let out = flux_bank_propose_transfer("a", "b", 100, None, Some("test"));
        assert!(out.contains("dry_run"));
    }

    #[test]
    fn bifrost_plan_has_steps() {
        let out = flux_bifrost_run("port bank", "grok-viktor");
        assert!(out.contains("flux_vast_recommend"));
    }
}