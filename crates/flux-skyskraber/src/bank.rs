//! Quillon Bank — the main organ.
//!
//! Every flow in the tower ends in a ledger row: payroll to robots and humans,
//! vault storage fees, auditorium honoraria. The ledger is the real
//! `flux-bank-core::Ledger` — the same propose→simulate→execute discipline the
//! Flux bank uses, so a transfer cannot happen without a signed intent and a
//! successful dry-run.
//!
//! Honest boundary: this ledger is the building's *local* book in micro-QUG.
//! Live settlement to the Quillon Graph chain goes through the quillon-wallet
//! MCP — that seam is deliberate (the building must keep operating through a
//! chain outage, then reconcile).

use crate::workforce::Workforce;
use flux_bank_core::{BankError, Ledger, SignedIntent, TransferProposal};
use serde::{Deserialize, Serialize};

pub const TOKEN: &str = "QUG";
/// The building's own operating account.
pub const TREASURY: &str = "skyskraber-treasury";
/// Storage fee per gold bar per day, in uQUG.
pub const VAULT_FEE_PER_BAR_UQUG: u128 = 250;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PayrollReport {
    pub paid_workers: u32,
    pub total_uqug: u128,
    pub failures: u32,
}

pub struct QuillonBank {
    ledger: Ledger,
}

impl QuillonBank {
    pub fn new() -> Self {
        Self { ledger: Ledger::new() }
    }

    /// Fund the treasury and open a zero account per worker wallet.
    pub fn bootstrap_accounts(&mut self, wf: &Workforce) {
        self.ledger
            .credit(TREASURY, TOKEN, 10_000_000)
            .expect("bootstrap credit cannot fail on a fresh ledger");
        for w in &wf.workers {
            // Opening balance of 0 is implicit; a symbolic 1 uQUG dust makes the
            // account visible in balance queries.
            let _ = self.ledger.credit(&w.wallet, TOKEN, 1);
        }
    }

    pub fn balance(&self, account: &str) -> u128 {
        self.ledger.balance(account, TOKEN)
    }

    /// The one gate all money passes through: proposal + signed intent,
    /// simulate-first semantics enforced by flux-bank-core.
    pub fn pay(&mut self, from: &str, to: &str, amount_uqug: u128, memo: &str) -> Result<(), BankError> {
        let proposal = TransferProposal {
            from: from.into(),
            to: to.into(),
            token: TOKEN.into(),
            amount_uqug,
            memo: Some(memo.into()),
            dry_run: false,
        };
        let intent = SignedIntent {
            actor: "skyskraber-cortex".into(),
            action: "transfer".into(),
            amount_uqug: Some(amount_uqug),
            memo: Some(memo.into()),
            wallet_auth: Some("building-ops-key".into()),
        };
        self.ledger.execute_transfer(&proposal, &intent)
    }

    /// Daily payroll: every worker is paid their wage from the treasury.
    pub fn run_payroll(&mut self, wf: &Workforce) -> PayrollReport {
        let mut report = PayrollReport { paid_workers: 0, total_uqug: 0, failures: 0 };
        for w in &wf.workers {
            match self.pay(TREASURY, &w.wallet, w.wage_uqug, "daily payroll") {
                Ok(()) => {
                    report.paid_workers += 1;
                    report.total_uqug += w.wage_uqug;
                }
                Err(_) => report.failures += 1,
            }
        }
        report
    }

    /// Vault custody is a service the bank charges for.
    pub fn charge_vault_fee(&mut self, client: &str, bars: usize) -> Result<u128, BankError> {
        let fee = VAULT_FEE_PER_BAR_UQUG * bars as u128;
        if fee > 0 {
            self.pay(client, TREASURY, fee, "vault storage fee")?;
        }
        Ok(fee)
    }
}

impl Default for QuillonBank {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workforce::Workforce;

    #[test]
    fn payroll_pays_everyone_and_conserves_money() {
        let wf = Workforce::quillon_default(10, 2);
        let mut bank = QuillonBank::new();
        bank.bootstrap_accounts(&wf);
        let before = bank.balance(TREASURY);
        let report = bank.run_payroll(&wf);
        assert_eq!(report.paid_workers, 12);
        assert_eq!(report.failures, 0);
        assert_eq!(bank.balance(TREASURY), before - report.total_uqug);
        // Every wallet got its wage on top of the 1 uQUG dust.
        let w0 = &wf.workers[0];
        assert_eq!(bank.balance(&w0.wallet), 1 + w0.wage_uqug);
    }

    #[test]
    fn overdraft_is_refused_not_absorbed() {
        let mut bank = QuillonBank::new();
        // Empty account tries to pay: flux-bank-core must refuse.
        let err = bank.pay("ghost", TREASURY, 100, "nope");
        assert!(err.is_err());
    }

    #[test]
    fn vault_fee_scales_with_bars() {
        let wf = Workforce::quillon_default(1, 0);
        let mut bank = QuillonBank::new();
        bank.bootstrap_accounts(&wf);
        // Give a client funds, then charge for 4 bars.
        bank.pay(TREASURY, "gold-client", 10_000, "client funding").unwrap();
        let fee = bank.charge_vault_fee("gold-client", 4).unwrap();
        assert_eq!(fee, 4 * VAULT_FEE_PER_BAR_UQUG);
        assert_eq!(bank.balance("gold-client"), 10_000 - fee);
    }
}
