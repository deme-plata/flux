//! flux-money-flow — the canonical **"gate decides → kit settles"** pattern.
//!
//! Composes a DECISION layer (a [`DecisionGate`] — e.g. rocky-epsilon's
//! `agentic-money-kit` Verified Execution Gate) with the LEDGER layer
//! ([`flux_money_kit::Treasury`]). An agent proposes a [`MoneyAction`]; the gate
//! rules approve / reject / honeypot; only an **Approve** ever reaches the books,
//! which stay conserved. Rejected actions leave zero half-applied state.
//!
//! Bring your own gate by implementing [`DecisionGate`] — the reference
//! [`VerifiedExecutionGate`] mirrors the agentic-money-kit shape so a real gate
//! drops straight in.

use flux_money_kit::{skim, Receipt, Result, Treasury};

/// A money action an agent proposes. The gate sees this BEFORE the ledger does.
#[derive(Debug, Clone)]
pub enum MoneyAction {
    /// Move `amount` of `token` from→to.
    Settle { from: String, to: String, token: String, amount: u128 },
    /// Resell `gross` skimming `bps` to the house: customer pays gross, provider
    /// nets, house keeps the skim.
    Skim { customer: String, provider: String, house: String, token: String, gross: u128, bps: u32 },
}

impl MoneyAction {
    /// Counterparties this action touches (for whitelist / honeypot checks).
    pub fn parties(&self) -> Vec<&str> {
        match self {
            MoneyAction::Settle { from, to, .. } => vec![from, to],
            MoneyAction::Skim { customer, provider, house, .. } => vec![customer, provider, house],
        }
    }
    /// Value at risk (for slippage / size caps).
    pub fn value(&self) -> u128 {
        match self {
            MoneyAction::Settle { amount, .. } => *amount,
            MoneyAction::Skim { gross, .. } => *gross,
        }
    }
}

/// The gate's ruling on a proposed action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Approve,
    Reject(String),
    /// Looks like a honeypot / trap — refuse and flag.
    Honeypot(String),
}

/// The DECISION layer. Implement this to plug in any gate. It only RULES — it
/// never touches the ledger (separation of duties is the whole point).
pub trait DecisionGate {
    fn decide(&self, action: &MoneyAction) -> Verdict;
}

/// Reference gate: whitelist of allowed parties + honeypot blocklist + a max
/// per-action value (slippage / size cap). Mirrors the agentic-money-kit gate so
/// a real gate can drop in via [`DecisionGate`].
pub struct VerifiedExecutionGate {
    pub whitelist: Vec<String>,
    pub honeypots: Vec<String>,
    pub max_value: u128,
}

impl DecisionGate for VerifiedExecutionGate {
    fn decide(&self, action: &MoneyAction) -> Verdict {
        for p in action.parties() {
            if self.honeypots.iter().any(|h| h == p) {
                return Verdict::Honeypot(format!("party '{p}' is a known honeypot"));
            }
            if !self.whitelist.iter().any(|w| w == p) {
                return Verdict::Reject(format!("party '{p}' not on whitelist"));
            }
        }
        if action.value() > self.max_value {
            return Verdict::Reject(format!("value {} exceeds cap {}", action.value(), self.max_value));
        }
        Verdict::Approve
    }
}

/// The result of pushing an action through the flow.
#[derive(Debug, Clone)]
pub enum FlowOutcome {
    Settled(Vec<Receipt>),
    Blocked(Verdict),
}

/// The composition: a gate + a treasury. [`execute`](MoneyFlow::execute) runs
/// `gate.decide()` FIRST; only an `Approve` reaches the ledger. Rejected /
/// honeypot actions leave the books untouched.
pub struct MoneyFlow<G: DecisionGate> {
    pub gate: G,
    pub treasury: Treasury,
}

impl<G: DecisionGate> MoneyFlow<G> {
    pub fn new(gate: G) -> Self {
        Self { gate, treasury: Treasury::new() }
    }

    /// Gate-then-settle. Returns `Settled(receipts)` on approval, `Blocked(verdict)`
    /// otherwise. A ledger error (e.g. insufficient funds) surfaces as `Err`.
    pub fn execute(&mut self, action: &MoneyAction) -> Result<FlowOutcome> {
        match self.gate.decide(action) {
            Verdict::Approve => {}
            v => return Ok(FlowOutcome::Blocked(v)),
        }
        let receipts = match action {
            MoneyAction::Settle { from, to, token, amount } => {
                vec![self.treasury.settle(from, to, token, *amount)?]
            }
            MoneyAction::Skim { customer, provider, house, token, gross, bps } => {
                let (net, fee) = skim(*gross, *bps)?;
                let r1 = self.treasury.settle(customer, provider, token, net)?;
                let r2 = self.treasury.settle(customer, house, token, fee)?;
                vec![r1, r2]
            }
        };
        Ok(FlowOutcome::Settled(receipts))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux_money_kit::MICRO;

    fn gate() -> VerifiedExecutionGate {
        VerifiedExecutionGate {
            whitelist: vec!["alice".into(), "bob".into(), "house".into(), "provider".into()],
            honeypots: vec!["evil".into()],
            max_value: 1000 * MICRO,
        }
    }

    #[test]
    fn approved_settle_moves_and_conserves() {
        let mut f = MoneyFlow::new(gate());
        f.treasury.credit("alice", "QUG", 100 * MICRO).unwrap();
        let before = f.treasury.supply("QUG");
        let out = f.execute(&MoneyAction::Settle {
            from: "alice".into(), to: "bob".into(), token: "QUG".into(), amount: 40 * MICRO,
        }).unwrap();
        assert!(matches!(out, FlowOutcome::Settled(_)));
        assert_eq!(f.treasury.balance("bob", "QUG"), 40 * MICRO);
        assert_eq!(f.treasury.supply("QUG"), before);
    }

    #[test]
    fn non_whitelisted_blocked_books_untouched() {
        let mut f = MoneyFlow::new(gate());
        f.treasury.credit("alice", "QUG", 100 * MICRO).unwrap();
        let out = f.execute(&MoneyAction::Settle {
            from: "alice".into(), to: "stranger".into(), token: "QUG".into(), amount: 10 * MICRO,
        }).unwrap();
        assert!(matches!(out, FlowOutcome::Blocked(Verdict::Reject(_))));
        assert_eq!(f.treasury.balance("alice", "QUG"), 100 * MICRO);
        assert_eq!(f.treasury.balance("stranger", "QUG"), 0);
    }

    #[test]
    fn honeypot_blocked_books_untouched() {
        let mut f = MoneyFlow::new(gate());
        f.treasury.credit("alice", "QUG", 100 * MICRO).unwrap();
        let out = f.execute(&MoneyAction::Settle {
            from: "alice".into(), to: "evil".into(), token: "QUG".into(), amount: MICRO,
        }).unwrap();
        assert!(matches!(out, FlowOutcome::Blocked(Verdict::Honeypot(_))));
        assert_eq!(f.treasury.balance("alice", "QUG"), 100 * MICRO);
    }

    #[test]
    fn over_cap_blocked() {
        let mut f = MoneyFlow::new(gate());
        f.treasury.credit("alice", "QUG", 5000 * MICRO).unwrap();
        let out = f.execute(&MoneyAction::Settle {
            from: "alice".into(), to: "bob".into(), token: "QUG".into(), amount: 2000 * MICRO,
        }).unwrap();
        assert!(matches!(out, FlowOutcome::Blocked(Verdict::Reject(_))));
    }

    #[test]
    fn skim_flow_pays_house_and_conserves() {
        let mut f = MoneyFlow::new(gate());
        f.treasury.credit("alice", "QUG", 100 * MICRO).unwrap();
        let before = f.treasury.supply("QUG");
        let out = f.execute(&MoneyAction::Skim {
            customer: "alice".into(), provider: "provider".into(), house: "house".into(),
            token: "QUG".into(), gross: 100 * MICRO, bps: 1000,
        }).unwrap();
        assert!(matches!(out, FlowOutcome::Settled(_)));
        assert_eq!(f.treasury.balance("alice", "QUG"), 0);
        assert_eq!(f.treasury.balance("provider", "QUG"), 90 * MICRO);
        assert_eq!(f.treasury.balance("house", "QUG"), 10 * MICRO);
        assert_eq!(f.treasury.supply("QUG"), before);
    }
}
