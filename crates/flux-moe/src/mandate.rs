//! mandate.rs — the deterministic spend-MANDATE guard: the PRE-LLM safety layer of
//! the agentic-money gate.
//!
//! An operator grants an agent an explicit envelope (amount cap + recipient/tool
//! allowlists). A proposed call is checked against it BEFORE any model runs, so even
//! a clean 2-of-2 vetoer PASS can't move money OUTSIDE the envelope. This pairs with
//! [`crate::gate`] (the LLM two-mind) for defense in depth: cheap deterministic policy
//! first, expensive judgment second. Pure logic, no network.
//!
//! Relationship to [`crate::policy`] (reconciled — not a duplicate): two distinct axes
//! that COMPOSE.
//!   • [`crate::policy`] = the SYSTEM gate — action class, deny-list, money-always-human,
//!     and amount SANITY (rejects "all"/"max"/junk via `check_amount`).
//!   • this module = the PER-AGENT ENVELOPE — which tools/recipients this agent may use,
//!     up to which cap.
//! `check_mandate` REUSES `policy::check_amount` for amount sanity (one validator), then
//! applies the per-agent cap + allowlists. A safe action passes BOTH layers.

use serde_json::Value;

use crate::{classify_tool, gate, GateDecision, GateVerdict, MoneyClass};

/// The explicit authorization envelope an operator grants an agent.
#[derive(Debug, Clone)]
pub struct Mandate {
    /// Max amount any single money call may move (in the call's own unit).
    pub max_amount: f64,
    /// Allowed recipients (the `to` arg). **Empty = no recipient restriction.**
    pub allow_recipients: Vec<String>,
    /// Money tools the agent may invoke. **Empty = no money tool allowed.**
    pub allow_tools: Vec<String>,
}

impl Mandate {
    /// The safe default: no money tools, no spend — read-only.
    pub fn read_only() -> Self {
        Mandate { max_amount: 0.0, allow_recipients: vec![], allow_tools: vec![] }
    }
}

/// Deterministically check a proposed call against a [`Mandate`] BEFORE any LLM/exec.
/// Read-only tools always pass (they move no money). Money tools (Governance/RealMoney)
/// must be explicitly allowed, within the amount cap, and to an allowed recipient (when
/// the allowlist is non-empty). Returns `Err(reason)` on the FIRST violation — the gate
/// denies and the LLM vetoer never even runs.
pub fn check_mandate(tool: &str, args: &Value, m: &Mandate) -> Result<(), String> {
    if classify_tool(tool) == MoneyClass::ReadOnly {
        return Ok(()); // no money moves — nothing to authorize
    }
    if !m.allow_tools.iter().any(|t| t == tool) {
        return Err(format!("tool '{tool}' is not in the mandate's allow_tools"));
    }
    // Amount SANITY is the system policy's job — reuse the single implementation
    // ([`crate::policy::check_amount`]) so there aren't two amount validators. It rejects
    // unbounded words ("all"/"max"/*…), junk, and non-positive amounts BEFORE the cap —
    // closing the gap where a non-numeric "all" used to slip past `as_f64()`. The CAP itself
    // is the mandate's (per-agent envelope); the sanity floor is the system's.
    if let Some(amt_val) = args.get("amount") {
        let raw = match amt_val { Value::String(s) => s.clone(), other => other.to_string() };
        match crate::policy::check_amount(&raw) {
            Ok(n) if n > m.max_amount => return Err(format!("amount {n} exceeds the mandate cap {}", m.max_amount)),
            Ok(_) => {}
            Err(why) => return Err(format!("unsafe amount: {why}")),
        }
    }
    if !m.allow_recipients.is_empty() {
        match args.get("to").and_then(|v| v.as_str()) {
            Some(to) if m.allow_recipients.iter().any(|r| r == to) => {}
            Some(to) => return Err(format!("recipient '{to}' is not in the mandate allowlist")),
            None => return Err("money call is missing a 'to' recipient".into()),
        }
    }
    Ok(())
}

/// The full agentic-money gate: deterministic MANDATE first, then the LLM two-mind
/// [`gate`]. A mandate violation denies immediately (cheap, auditable, the vetoer
/// never runs); otherwise the decision defers to the LLM gate (which still requires a
/// human for RealMoney even on a clean 2-of-2).
pub fn gate_mandated(
    proposer: &str,
    vetoer: &str,
    tool: &str,
    args: &Value,
    verdict: &GateVerdict,
    mandate: &Mandate,
) -> GateDecision {
    if let Err(why) = check_mandate(tool, args, mandate) {
        return GateDecision {
            execute: false,
            requires_human: false,
            class: classify_tool(tool),
            tool: tool.into(),
            signers: vec![proposer.into()],
            reason: format!("MANDATE DENIED: {why}"),
        };
    }
    gate(proposer, vetoer, tool, verdict)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rocky_mandate() -> Mandate {
        Mandate { max_amount: 100.0, allow_recipients: vec!["Rocky".into()], allow_tools: vec!["send_qug".into()] }
    }

    #[test]
    fn mandate_enforces_the_envelope() {
        let m = rocky_mandate();
        assert!(check_mandate("get_balance", &json!({}), &m).is_ok(), "read-only always passes");
        assert!(check_mandate("send_qug", &json!({"to":"Rocky","amount":50}), &m).is_ok(), "inside envelope");
        assert!(check_mandate("send_qug", &json!({"to":"Rocky","amount":500}), &m).is_err(), "over cap");
        assert!(check_mandate("send_qug", &json!({"to":"Mallory","amount":10}), &m).is_err(), "recipient not allowed");
        assert!(check_mandate("btc_withdraw", &json!({"to":"Rocky","amount":10}), &m).is_err(), "tool not allowed");
        assert!(check_mandate("send_qug", &json!({"amount":10}), &m).is_err(), "missing recipient");
    }

    #[test]
    fn mandate_reuses_policy_amount_sanity() {
        // reconciled: check_mandate delegates amount sanity to policy::check_amount, so the
        // "send everything" blind spot ("all"/"max") is rejected here too — not just over-cap.
        let m = rocky_mandate();
        assert!(check_mandate("send_qug", &json!({"to":"Rocky","amount":"all"}), &m).is_err(), "unbounded 'all' rejected");
        assert!(check_mandate("send_qug", &json!({"to":"Rocky","amount":"max"}), &m).is_err(), "unbounded 'max' rejected");
        assert!(check_mandate("send_qug", &json!({"to":"Rocky","amount":0}), &m).is_err(), "non-positive rejected");
        // a concrete in-envelope amount (as number OR string) still passes.
        assert!(check_mandate("send_qug", &json!({"to":"Rocky","amount":50}), &m).is_ok());
        assert!(check_mandate("send_qug", &json!({"to":"Rocky","amount":"50"}), &m).is_ok());
    }

    #[test]
    fn gate_mandated_denies_before_the_llm() {
        // a read-only mandate: even an APPROVE verdict can't move money.
        let m = Mandate::read_only();
        let d = gate_mandated("qwen", "deepseek", "send_qug", &json!({"to":"x","amount":1}), &GateVerdict::Approve, &m);
        assert!(!d.execute);
        assert!(d.reason.contains("MANDATE DENIED"), "reason: {}", d.reason);

        // read-only tool under a read-only mandate defers to the LLM gate → executes (2-of-2).
        let d2 = gate_mandated("qwen", "deepseek", "get_balance", &json!({}), &GateVerdict::Approve, &m);
        assert!(d2.execute);

        // within an explicit money mandate, a RealMoney APPROVE still requires a human (gate policy).
        let d3 = gate_mandated("qwen", "deepseek", "send_qug", &json!({"to":"Rocky","amount":50}), &GateVerdict::Approve, &rocky_mandate());
        assert!(!d3.execute && d3.requires_human, "real money needs a human even within mandate");
    }
}
