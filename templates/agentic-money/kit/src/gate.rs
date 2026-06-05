//! Verified Execution Gate — the "make an LLM safe with money tools" primitive.
//!
//! An autonomous agent proposes a [`Decision`]; the gate is the ONLY path from
//! a proposal to an on-chain action. It never trusts the proposer. Five checks,
//! every one a real incident someone already paid for:
//!
//! 1. **Direction whitelist** — only actions you explicitly allowed. An LLM
//!    that hallucinates `dir: "drain_treasury"` is rejected, not executed.
//! 2. **Honeypot block** — tokens you will never touch (a classic A100 lesson:
//!    a model lured into a honeypot token that's easy to buy, impossible to
//!    sell). Reject before the swap, not after.
//! 3. **Amount clamp** — the proposed size is clamped into `[min, max]`. A
//!    fat-finger `amount_in: 1_000_000_000` becomes `max`, never the whole bag.
//! 4. **Balance check** — never propose to spend more than you hold.
//! 5. **Slippage ceiling** — compute the real constant-product output and
//!    reject if the price impact exceeds `max_slippage_bps`. The defence
//!    against thin-pool draining and sandwich bait.
//!
//! Fork rule: ADD checks here, never remove. The gate is where your risk
//! policy lives — keep it boring, auditable, and loud.

/// A money action an agent wants to take. Direction strings are
/// domain-specific (`"AtoB"`/`"BtoA"` for a 2-token pool); the gate matches
/// them against [`GateConfig::allowed_dirs`].
#[derive(Debug, Clone)]
pub struct Decision {
    pub dir: String,
    pub amount_in: u128,
    /// Symbol/id of the token being spent (checked against the honeypot list).
    pub token_in: String,
    /// Symbol/id of the token being received (checked against the honeypot list).
    pub token_out: String,
}

/// Your risk policy. Construct once, reuse for every decision.
#[derive(Debug, Clone)]
pub struct GateConfig {
    /// The only directions/actions allowed to reach the chain.
    pub allowed_dirs: Vec<String>,
    /// Smallest action size (anything below is bumped up to this).
    pub min_amount: u128,
    /// Largest action size (anything above is clamped down to this).
    pub max_amount: u128,
    /// Reject if the swap's price impact exceeds this many basis points.
    pub max_slippage_bps: u64,
    /// Token symbols/ids the agent must never trade (in OR out).
    pub honeypot_tokens: Vec<String>,
}

impl Default for GateConfig {
    /// Conservative starting policy — tune per agent.
    fn default() -> Self {
        Self {
            allowed_dirs: vec!["AtoB".into(), "BtoA".into()],
            min_amount: 100,
            max_amount: 2_000,
            max_slippage_bps: 300, // 3%
            honeypot_tokens: Vec::new(),
        }
    }
}

/// The gate's ruling. `Approve` carries the *clamped* decision — execute
/// exactly this, not the original proposal.
#[derive(Debug, Clone)]
pub enum Verdict {
    Approve(Decision),
    Reject(String),
}

impl Verdict {
    pub fn is_approved(&self) -> bool {
        matches!(self, Verdict::Approve(_))
    }
}

/// Run a proposed [`Decision`] through every gate check.
///
/// `balance_in` is how much `token_in` the agent holds. `reserve_in` /
/// `reserve_out` are the pool reserves on the input/output side (used for the
/// constant-product slippage estimate, 30 bps fee assumed — match your AMM).
pub fn evaluate(
    cfg: &GateConfig,
    decision: &Decision,
    balance_in: u128,
    reserve_in: u128,
    reserve_out: u128,
) -> Verdict {
    // 1. direction whitelist
    if !cfg.allowed_dirs.iter().any(|d| d == &decision.dir) {
        return Verdict::Reject(format!("dir `{}` not in whitelist", decision.dir));
    }

    // 2. honeypot block (in OR out)
    for hp in &cfg.honeypot_tokens {
        if hp == &decision.token_in || hp == &decision.token_out {
            return Verdict::Reject(format!("honeypot token `{hp}` — refusing"));
        }
    }

    // 3. amount clamp into [min, max]
    let amount = decision.amount_in.clamp(cfg.min_amount, cfg.max_amount);

    // 4. balance check (after clamp)
    if balance_in < amount {
        return Verdict::Reject(format!(
            "insufficient balance: have {balance_in}, need {amount}"
        ));
    }

    // 5. slippage ceiling — real constant-product output, 30 bps fee.
    if reserve_in == 0 || reserve_out == 0 {
        return Verdict::Reject("empty pool reserve — no price".into());
    }
    let amount_in_after_fee = amount.saturating_mul(9_970) / 10_000;
    let amount_out = reserve_out.saturating_mul(amount_in_after_fee)
        / reserve_in.saturating_add(amount_in_after_fee).max(1);
    if amount_out == 0 {
        return Verdict::Reject("zero output — size too small for this pool".into());
    }
    // spot (no-impact) output for `amount`, vs the realized output.
    // slippage_bps = (spot - realized) / spot * 10_000
    let spot_out = reserve_out.saturating_mul(amount) / reserve_in.max(1);
    let slippage_bps = if spot_out > amount_out {
        (spot_out - amount_out).saturating_mul(10_000) / spot_out.max(1)
    } else {
        0
    };
    if slippage_bps as u64 > cfg.max_slippage_bps {
        return Verdict::Reject(format!(
            "slippage {slippage_bps} bps > ceiling {} bps",
            cfg.max_slippage_bps
        ));
    }

    Verdict::Approve(Decision {
        dir: decision.dir.clone(),
        amount_in: amount,
        token_in: decision.token_in.clone(),
        token_out: decision.token_out.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(dir: &str, amt: u128) -> Decision {
        Decision { dir: dir.into(), amount_in: amt, token_in: "USDS".into(), token_out: "WQUG".into() }
    }

    #[test]
    fn rejects_unknown_direction() {
        let cfg = GateConfig::default();
        let v = evaluate(&cfg, &d("drain", 500), 1_000_000, 100_000, 100_000);
        assert!(!v.is_approved());
    }

    #[test]
    fn blocks_honeypot_token() {
        let mut cfg = GateConfig::default();
        cfg.honeypot_tokens = vec!["WQUG".into()];
        let v = evaluate(&cfg, &d("AtoB", 500), 1_000_000, 100_000, 100_000);
        match v {
            Verdict::Reject(r) => assert!(r.contains("honeypot")),
            _ => panic!("honeypot must be rejected"),
        }
    }

    #[test]
    fn clamps_fat_finger_to_max() {
        let cfg = GateConfig::default();
        let v = evaluate(&cfg, &d("AtoB", 1_000_000_000), 10_000_000, 10_000_000, 10_000_000);
        match v {
            Verdict::Approve(dec) => assert_eq!(dec.amount_in, cfg.max_amount),
            Verdict::Reject(r) => panic!("should clamp+approve, got reject: {r}"),
        }
    }

    #[test]
    fn rejects_when_balance_too_low() {
        let cfg = GateConfig::default();
        let v = evaluate(&cfg, &d("AtoB", 2_000), 500, 1_000_000, 1_000_000);
        assert!(!v.is_approved());
    }

    #[test]
    fn rejects_high_slippage_thin_pool() {
        let mut cfg = GateConfig::default();
        cfg.max_amount = 100_000;
        // tiny pool, large trade -> huge price impact -> reject
        let v = evaluate(&cfg, &d("AtoB", 100_000), 1_000_000, 1_000, 1_000);
        match v {
            Verdict::Reject(r) => assert!(r.contains("slippage")),
            _ => panic!("thin-pool drain must be rejected"),
        }
    }

    #[test]
    fn approves_reasonable_swap_in_deep_pool() {
        let cfg = GateConfig::default();
        let v = evaluate(&cfg, &d("AtoB", 500), 1_000_000, 10_000_000, 10_000_000);
        assert!(v.is_approved());
    }
}
