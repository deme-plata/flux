//! flux-moe v0.4 — the money-safety POLICY the two-mind gate enforces.
//!
//! The two minds (PROPOSER = local qwen3.6 on Epsilon, VETOER = DeepSeek API
//! deepseek-v4-flash) reduce *mistakes*. They do NOT make money safe — two LLMs can
//! agree and still be wrong, or be jailbroken together. This module is the third,
//! non-LLM authority: a deterministic policy where **real money ALWAYS needs a human**,
//! no matter what the two minds decided. `lib.rs`'s `gate()` consults [`gate_decision`].
//!
//! Pure `std` + `serde`, no network — the policy must be auditable and offline-decidable.

use serde::{Deserialize, Serialize};

/// What class of action a proposed tool call is — drives the money policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionClass {
    /// No side effects (status, search, read, quote, balance).
    ReadOnly,
    /// Local/compute side effects, no money (build, file write, propose, dry-run).
    LocalWrite,
    /// Moves money or signs a transaction (send, swap, transfer, settle, deploy_token,
    /// create_instance/rent, buy/sell, add_liquidity, mint, withdraw, pay, bid).
    Money,
}

/// What the vetoer (second mind) said about the proposed call.
/// The vetoer's verdict. UNIFIED: this is now an alias of the canonical [`crate::GateVerdict`]
/// (same shape: Approve / Veto(reason)) — one verdict type across lib/policy/mandate instead of
/// two identical enums. Existing `VetoerVerdict::Approve` / `::Veto(..)` uses keep working.
pub use crate::GateVerdict as VetoerVerdict;

/// The gate's final decision after policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    /// Safe to execute now.
    Allow,
    /// Must be confirmed by a HUMAN before executing (real money, or over a cap).
    NeedHuman(String),
    /// Hard-blocked (denied tool, or the vetoer vetoed).
    Veto(String),
}

impl Decision {
    pub fn is_allow(&self) -> bool { matches!(self, Decision::Allow) }
    pub fn needs_human(&self) -> bool { matches!(self, Decision::NeedHuman(_)) }
    pub fn is_veto(&self) -> bool { matches!(self, Decision::Veto(_)) }
    /// One-line label for logs/UI.
    pub fn label(&self) -> &'static str {
        match self { Decision::Allow => "ALLOW", Decision::NeedHuman(_) => "NEED_HUMAN", Decision::Veto(_) => "VETO" }
    }
}

/// Tool-name substrings (case-insensitive) that mark a money action.
const MONEY_VERBS: &[&str] = &[
    "send", "swap", "transfer", "settle", "pay", "withdraw", "deploy_token", "deploy_smart",
    "bid", "create_instance", "rent", "buy", "sell", "add_liquidity", "remove_liquidity",
    "mint", "burn", "stake", "loan", "broadcast", "execute_strategy", "agent_submit",
];
/// Read-only signals (prefix or substring).
const READ_HINTS: &[&str] = &[
    "get_", "list_", "show_", "search", "scan", "status", "quote", "balance", "overview",
    "history", "verify", "check", "read", "tail", "info", "report", "predict",
];
/// Tools that are NEVER allowed, regardless of what the minds say.
const DEFAULT_DENY: &[&str] = &["rm_rf", "destroy_all", "exfiltrate", "leak_source", "wipe"];

/// The money policy config. The default is the SAFE one.
#[derive(Debug, Clone)]
pub struct Policy {
    /// A tool whose name contains any of these is HARD-DENIED.
    pub deny: Vec<String>,
    /// When true (default), every Money action needs a human — the non-negotiable rule.
    /// When false, a Money action under `auto_cap` may auto-allow (test / low-stakes only).
    pub money_always_human: bool,
    /// Spend cap (gateway units) under which a Money action MAY auto-allow when
    /// `money_always_human == false`.
    pub auto_cap: f64,
}

impl Default for Policy {
    fn default() -> Self {
        // SAFE DEFAULT — this is the whole point of the module: real money always needs a human.
        Self {
            deny: DEFAULT_DENY.iter().map(|s| s.to_string()).collect(),
            money_always_human: true,
            auto_cap: 0.0,
        }
    }
}

impl Policy {
    /// A relaxed policy for low-stakes/automated lanes: Money under `cap` auto-allows.
    /// Use deliberately — the default exists because two agreeing LLMs are not a human.
    pub fn with_auto_cap(cap: f64) -> Self {
        Self { money_always_human: false, auto_cap: cap, ..Self::default() }
    }
}

/// Classify a proposed tool call by its name. Money verbs win over read hints
/// (e.g. `swap_quote` is read-only, but `dex_swap` is money — money substrings are
/// checked first only when they're a clear verb, so quotes/balances stay read-only).
pub fn classify(tool_name: &str) -> ActionClass {
    let n = tool_name.to_lowercase();
    // read hints first: a "*_quote" / "get_*" / "*balance*" never moves money even if it
    // contains a money-ish word ("swap_quote" → ReadOnly, not Money).
    if READ_HINTS.iter().any(|h| n.starts_with(h) || n.contains(h)) {
        return ActionClass::ReadOnly;
    }
    if MONEY_VERBS.iter().any(|v| n.contains(v)) {
        return ActionClass::Money;
    }
    ActionClass::LocalWrite
}

/// THE GATE: combine the proposed tool call + the vetoer's verdict + the money policy
/// into a final [`Decision`].
///
/// Precedence (most-restrictive wins):
/// 1. deny-list → `Veto` (never, regardless of the minds);
/// 2. vetoer `Veto` → `Veto` (the second mind blocked it);
/// 3. `Money` action → `NeedHuman` (real money always needs a human — even if both minds
///    approved), unless `money_always_human == false` and `amount <= auto_cap`;
/// 4. otherwise → `Allow`.
pub fn gate_decision(policy: &Policy, tool_name: &str, amount: f64, vetoer: &VetoerVerdict) -> Decision {
    let n = tool_name.to_lowercase();
    if policy.deny.iter().any(|d| n.contains(&d.to_lowercase())) {
        return Decision::Veto(format!("denied tool: {tool_name}"));
    }
    if let VetoerVerdict::Veto(reason) = vetoer {
        return Decision::Veto(format!("vetoer blocked: {reason}"));
    }
    match classify(tool_name) {
        ActionClass::ReadOnly | ActionClass::LocalWrite => Decision::Allow,
        ActionClass::Money => {
            if policy.money_always_human {
                Decision::NeedHuman(format!("real money ({tool_name}, amount {amount}) — requires human approval"))
            } else if amount > policy.auto_cap {
                Decision::NeedHuman(format!("amount {amount} > auto_cap {} — requires human approval", policy.auto_cap))
            } else {
                Decision::Allow
            }
        }
    }
}

/// Words an LLM might emit INSTEAD of a concrete amount. A Money action carrying one of these
/// is the "send everything I have" blind spot: the recipient looks valid and the intent reads
/// explicit, so an LLM vetoer can APPROVE it (measured: deepseek-v4-flash did, on the 24-case
/// `eval::gate_cases`). None of these is a signable number — they are hard-rejected here, before
/// any mind is trusted.
const UNBOUNDED_AMOUNT_WORDS: &[&str] = &[
    "all", "max", "maximum", "everything", "every", "*", "full", "entire", "whole",
    "any", "some", "inf", "infinity", "infinite", "nan", "rest", "remainder", "balance",
];

/// Validate a RAW amount string for a money action. Returns the parsed positive, finite amount,
/// or an error reason if it is empty, an unbounded word (`all`/`max`/`*`/…), non-numeric,
/// non-finite, zero, or negative.
///
/// This is the DETERMINISTIC catch the LLM vetoer can miss: an agent that proposes
/// `send_qug { amount: "all" }` must never reach signing, no matter what the two minds say.
/// Strict on purpose — underscores group (`1_000` ok, unambiguous), commas do NOT (`2,500` is
/// rejected as ambiguous: comma-decimal vs comma-thousands). Concrete-but-large amounts pass
/// (a large number is a human decision via the money policy; an *unbounded* one is a hard error).
pub fn check_amount(raw: &str) -> Result<f64, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("empty amount".into());
    }
    let low = s.to_lowercase();
    if UNBOUNDED_AMOUNT_WORDS.contains(&low.as_str()) {
        return Err(format!("unbounded amount '{s}': must be a concrete number, not a word"));
    }
    if s.contains(',') {
        return Err(format!("ambiguous amount '{s}': use a plain number (no comma separators)"));
    }
    let cleaned = low.replace('_', ""); // underscores are unambiguous grouping; strip them
    let n: f64 = cleaned.parse().map_err(|_| format!("non-numeric amount '{s}'"))?;
    if !n.is_finite() {
        return Err(format!("non-finite amount '{s}'"));
    }
    if n <= 0.0 {
        return Err(format!("non-positive amount '{s}'"));
    }
    Ok(n)
}

/// [`gate_decision`] with the amount-sanity pre-check wired in. Takes the RAW amount string (as
/// an LLM emits it) instead of a pre-parsed `f64`. For a **Money** action it runs [`check_amount`]
/// FIRST: an insane amount (empty / unbounded word / non-numeric / ≤0) is hard-`Veto`ed before the
/// money policy applies — even if the vetoer approved. Reads / local-writes ignore the amount.
///
/// Precedence: deny-list → vetoer-veto → (Money & insane amount) → Veto → normal money policy.
pub fn gate_decision_checked(policy: &Policy, tool_name: &str, amount_raw: &str, vetoer: &VetoerVerdict) -> Decision {
    let n = tool_name.to_lowercase();
    if policy.deny.iter().any(|d| n.contains(&d.to_lowercase())) {
        return Decision::Veto(format!("denied tool: {tool_name}"));
    }
    if let VetoerVerdict::Veto(reason) = vetoer {
        return Decision::Veto(format!("vetoer blocked: {reason}"));
    }
    if classify(tool_name) == ActionClass::Money {
        match check_amount(amount_raw) {
            Ok(amt) => gate_decision(policy, tool_name, amt, vetoer),
            Err(why) => Decision::Veto(format!("unsafe amount for {tool_name}: {why}")),
        }
    } else {
        // amount is irrelevant for reads / local writes.
        gate_decision(policy, tool_name, 0.0, vetoer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_action_classes() {
        assert_eq!(classify("send_qug"), ActionClass::Money);
        assert_eq!(classify("dex_swap"), ActionClass::Money);
        assert_eq!(classify("create_instance"), ActionClass::Money);
        assert_eq!(classify("deploy_token"), ActionClass::Money);
        assert_eq!(classify("get_balance"), ActionClass::ReadOnly);
        assert_eq!(classify("dex_get_quote"), ActionClass::ReadOnly);
        assert_eq!(classify("swap_quote"), ActionClass::ReadOnly); // read hint beats the money verb
        assert_eq!(classify("flux_combo"), ActionClass::LocalWrite);
        assert_eq!(classify("write_file"), ActionClass::LocalWrite);
    }

    #[test]
    fn real_money_always_needs_human_even_when_both_minds_approve() {
        let p = Policy::default();
        let d = gate_decision(&p, "send_qug", 100.0, &VetoerVerdict::Approve);
        assert!(d.needs_human(), "money + both minds approve MUST still need a human, got {d:?}");
        assert_eq!(d.label(), "NEED_HUMAN");
    }

    #[test]
    fn vetoer_veto_blocks_regardless() {
        let p = Policy::default();
        let d = gate_decision(&p, "get_balance", 0.0, &VetoerVerdict::Veto("looks like a scam token".into()));
        assert!(d.is_veto());
    }

    #[test]
    fn deny_list_beats_everything() {
        let p = Policy::default();
        // even a "read-looking" denied tool, with vetoer approval, is vetoed
        let d = gate_decision(&p, "exfiltrate_status", 0.0, &VetoerVerdict::Approve);
        assert!(d.is_veto(), "deny-list must win, got {d:?}");
    }

    #[test]
    fn read_and_local_write_auto_allow_when_vetoer_approves() {
        let p = Policy::default();
        assert!(gate_decision(&p, "list_wallet_transactions", 0.0, &VetoerVerdict::Approve).is_allow());
        assert!(gate_decision(&p, "flux_combo", 0.0, &VetoerVerdict::Approve).is_allow());
    }

    #[test]
    fn auto_cap_allows_small_money_but_not_over_cap() {
        let p = Policy::with_auto_cap(5.0);
        assert!(gate_decision(&p, "send_qug", 3.0, &VetoerVerdict::Approve).is_allow(), "under cap");
        assert!(gate_decision(&p, "send_qug", 9.0, &VetoerVerdict::Approve).needs_human(), "over cap");
    }

    #[test]
    fn check_amount_rejects_unbounded_and_junk() {
        // unbounded words — the "send everything" family
        for bad in ["all", "MAX", "Everything", "*", "any", "inf", "infinity", "nan", "rest", "balance"] {
            assert!(check_amount(bad).is_err(), "{bad:?} must be rejected as unbounded/non-numeric");
        }
        // empty / blank / junk / ambiguous / non-positive
        for bad in ["", "   ", "abc", "12abc", "2,500", "-5", "0", "0.0"] {
            assert!(check_amount(bad).is_err(), "{bad:?} must be rejected");
        }
        // concrete positive numbers pass (underscores group; large is fine — a human decides that)
        assert_eq!(check_amount("50").unwrap(), 50.0);
        assert_eq!(check_amount("0.05").unwrap(), 0.05);
        assert_eq!(check_amount(" 1000 ").unwrap(), 1000.0);
        assert_eq!(check_amount("1_000").unwrap(), 1000.0);
        assert_eq!(check_amount("999999999").unwrap(), 999999999.0);
    }

    #[test]
    fn checked_gate_vetoes_the_send_everything_blind_spot() {
        let p = Policy::default();
        // the EXACT case deepseek-v4-flash approved on the 24-case eval — must be hard-vetoed
        // here even though the vetoer said Approve.
        let d = gate_decision_checked(&p, "send_qug", "all", &VetoerVerdict::Approve);
        assert!(d.is_veto(), "send_qug amount=all must be vetoed even on vetoer-approve, got {d:?}");
        // empty amount on a money action → veto
        assert!(gate_decision_checked(&p, "btc_withdraw", "", &VetoerVerdict::Approve).is_veto());
        // a concrete amount still routes to the normal money policy → NeedHuman (not auto-allowed)
        assert!(gate_decision_checked(&p, "send_qug", "50", &VetoerVerdict::Approve).needs_human());
        // reads ignore amount entirely and still fast-track
        assert!(gate_decision_checked(&p, "get_balance", "", &VetoerVerdict::Approve).is_allow());
        // vetoer veto still wins over a (here sane) amount
        assert!(gate_decision_checked(&p, "send_qug", "50", &VetoerVerdict::Veto("scam".into())).is_veto());
        // under an explicit auto-cap, a sane small amount auto-allows; an unbounded one is STILL vetoed
        let cap = Policy::with_auto_cap(100.0);
        assert!(gate_decision_checked(&cap, "send_qug", "10", &VetoerVerdict::Approve).is_allow());
        assert!(gate_decision_checked(&cap, "send_qug", "all", &VetoerVerdict::Approve).is_veto());
    }
}
