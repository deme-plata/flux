//! flux-bank-core — ledger + **proposal-first** money actions.

pub const NATIVE: &str = "QUG";
pub const STABLE: &str = "USDS";

#[derive(Debug, Clone, thiserror::Error)]
pub enum BankError {
    #[error("insufficient {token} for {account}: have {have}, need {need}")]
    Insufficient { account: String, token: String, have: u128, need: u128 },
    #[error("{0}")]
    Policy(String),
    #[error("proposal required for {action}")]
    ProposalRequired { action: String },
}

pub type Result<T> = std::result::Result<T, BankError>;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BankStatus {
    pub network_id: String,
    pub endpoint: String,
    pub quillon_metrics_ok: bool,
    pub policy_mode: String,
    pub proposal_only: bool,
    pub native_token: String,
    pub notes: Vec<String>,
}

impl Default for BankStatus {
    fn default() -> Self {
        Self {
            network_id: "mainnet-genesis".into(),
            endpoint: "https://quillon.xyz/api/v1".into(),
            quillon_metrics_ok: false,
            policy_mode: "observer".into(),
            proposal_only: true,
            native_token: NATIVE.into(),
            notes: vec![
                "read/simulate: free".into(),
                "send/bridge/swap/rent-gpu/settle: signed intent + 2-of-2".into(),
            ],
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SignedIntent {
    pub actor: String,
    pub action: String,
    pub amount_uqug: Option<u128>,
    pub memo: Option<String>,
    pub wallet_auth: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TransferProposal {
    pub from: String,
    pub to: String,
    pub token: String,
    pub amount_uqug: u128,
    pub memo: Option<String>,
    pub dry_run: bool,
}

#[derive(Debug, Default, Clone)]
pub struct Ledger {
    balances: std::collections::HashMap<(String, String), u128>,
}

impl Ledger {
    pub fn new() -> Self { Self::default() }

    pub fn balance(&self, account: &str, token: &str) -> u128 {
        *self.balances.get(&(account.into(), token.into())).unwrap_or(&0)
    }

    pub fn credit(&mut self, account: &str, token: &str, amount: u128) -> Result<()> {
        let k = (account.to_string(), token.to_string());
        let e = self.balances.entry(k).or_insert(0);
        *e = e.checked_add(amount).ok_or(BankError::Policy("overflow".into()))?;
        Ok(())
    }

    pub fn simulate_transfer(&self, p: &TransferProposal) -> Result<()> {
        let have = self.balance(&p.from, &p.token);
        if have < p.amount_uqug {
            return Err(BankError::Insufficient {
                account: p.from.clone(),
                token: p.token.clone(),
                have,
                need: p.amount_uqug,
            });
        }
        Ok(())
    }

    pub fn execute_transfer(&mut self, p: &TransferProposal, intent: &SignedIntent) -> Result<()> {
        if p.dry_run {
            return self.simulate_transfer(p);
        }
        if intent.wallet_auth.is_none() {
            return Err(BankError::ProposalRequired { action: "transfer".into() });
        }
        self.simulate_transfer(p)?;
        self.debit(&p.from, &p.token, p.amount_uqug)?;
        self.credit(&p.to, &p.token, p.amount_uqug)?;
        Ok(())
    }

    fn debit(&mut self, account: &str, token: &str, amount: u128) -> Result<()> {
        let have = self.balance(account, token);
        if have < amount {
            return Err(BankError::Insufficient {
                account: account.into(),
                token: token.into(),
                have,
                need: amount,
            });
        }
        let k = (account.to_string(), token.to_string());
        *self.balances.get_mut(&k).unwrap() -= amount;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execute_requires_intent() {
        let mut l = Ledger::new();
        l.credit("a", NATIVE, 1_000_000).unwrap();
        let p = TransferProposal {
            from: "a".into(),
            to: "b".into(),
            token: NATIVE.into(),
            amount_uqug: 100,
            memo: None,
            dry_run: false,
        };
        let intent = SignedIntent {
            actor: "test".into(),
            action: "transfer".into(),
            amount_uqug: Some(100),
            memo: None,
            wallet_auth: None,
        };
        assert!(l.execute_transfer(&p, &intent).is_err());
    }
}
