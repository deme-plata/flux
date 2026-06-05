//! flux-money-kit — shared agentic-money substrate for the Flux money-factory.
//!
//! Pure std. Every mold (gateway, launchpad, estate, bank, bridge) imports this:
//! a multi-token [`Treasury`] ledger, conserving [`Treasury::settle`], the +10%
//! [`skim`] red-line, and a QUG→BTC [`btc_route`]. Amounts are integers in
//! micro-units (µ): 1 QUG = 1e6 µ — so all money math is exact u128, no floats.

use std::collections::HashMap;

/// Micro-units per whole token. All balances are µ-denominated integers.
pub const MICRO: u128 = 1_000_000;
/// Native settlement token symbol.
pub const NATIVE: &str = "QUG";
/// usdSIGIL stablecoin symbol.
pub const USDS: &str = "USDS";
/// Block reward: 0.083 QUG/blk, expressed in µ.
pub const BLOCK_REWARD_UQUG: u128 = 83_000;
/// Hard supply cap: 21,000,000 tokens, in µ.
pub const SUPPLY_CAP_U: u128 = 21_000_000 * MICRO;
/// Satoshis per whole BTC.
pub const SATS_PER_BTC: u128 = 100_000_000;

/// The shared money-error vocabulary. Fails loud — never silently swallowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoneyError {
    Insufficient { wallet: String, token: String, have: u128, need: u128 },
    Overflow,
    BadRate,
    Unroutable,
    CapExceeded { cap: u128, got: u128 },
}

impl std::fmt::Display for MoneyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MoneyError::Insufficient { wallet, token, have, need } =>
                write!(f, "insufficient {token} for {wallet}: have {have}, need {need}"),
            MoneyError::Overflow => write!(f, "arithmetic overflow"),
            MoneyError::BadRate => write!(f, "rate/argument out of range"),
            MoneyError::Unroutable => write!(f, "no eligible counterparty/route"),
            MoneyError::CapExceeded { cap, got } => write!(f, "supply cap {cap} exceeded by {got}"),
        }
    }
}
impl std::error::Error for MoneyError {}

pub type Result<T> = std::result::Result<T, MoneyError>;

/// Audit record of one balance move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub from: String,
    pub to: String,
    pub token: String,
    pub amount: u128,
}

/// Multi-token ledger — the shared bookkeeping substrate every mold builds on.
#[derive(Debug, Default, Clone)]
pub struct Treasury {
    bal: HashMap<(String, String), u128>,
}

impl Treasury {
    pub fn new() -> Self { Self::default() }

    /// Current balance of `token` held by `wallet` (0 if none).
    pub fn balance(&self, wallet: &str, token: &str) -> u128 {
        *self.bal.get(&(wallet.to_string(), token.to_string())).unwrap_or(&0)
    }

    /// Credit (mint / receive). Overflow-checked.
    pub fn credit(&mut self, wallet: &str, token: &str, amount: u128) -> Result<()> {
        let e = self.bal.entry((wallet.to_string(), token.to_string())).or_insert(0);
        *e = e.checked_add(amount).ok_or(MoneyError::Overflow)?;
        Ok(())
    }

    /// Debit (burn / spend). Fails loud if insufficient.
    pub fn debit(&mut self, wallet: &str, token: &str, amount: u128) -> Result<()> {
        let have = self.balance(wallet, token);
        if have < amount {
            return Err(MoneyError::Insufficient {
                wallet: wallet.to_string(), token: token.to_string(), have, need: amount,
            });
        }
        self.bal.insert((wallet.to_string(), token.to_string()), have - amount);
        Ok(())
    }

    /// Move `amount` of `token` from→to. Conserves total supply.
    pub fn settle(&mut self, from: &str, to: &str, token: &str, amount: u128) -> Result<Receipt> {
        self.debit(from, token, amount)?;
        self.credit(to, token, amount)?;
        Ok(Receipt { from: from.to_string(), to: to.to_string(), token: token.to_string(), amount })
    }

    /// Total of one token across all wallets — the conservation invariant.
    pub fn supply(&self, token: &str) -> u128 {
        self.bal.iter().filter(|((_, t), _)| t == token).map(|(_, v)| *v).sum()
    }
}

/// The +10% red-line. Split `gross` into `(net_to_provider, fee_to_house)` at
/// `bps` basis points (1000 = 10%). Errors if `bps > 10000`.
pub fn skim(gross: u128, bps: u32) -> Result<(u128, u128)> {
    if bps > 10_000 { return Err(MoneyError::BadRate); }
    let fee = gross.checked_mul(bps as u128).ok_or(MoneyError::Overflow)? / 10_000;
    Ok((gross - fee, fee))
}

/// DCA route: convert `amount_uqug` (µQUG) into satoshis at `sats_per_qug`.
pub fn btc_route(amount_uqug: u128, sats_per_qug: u128) -> Result<u128> {
    amount_uqug.checked_mul(sats_per_qug).ok_or(MoneyError::Overflow).map(|x| x / MICRO)
}

/// Enforce the hard supply cap.
pub fn check_cap(total: u128) -> Result<()> {
    if total > SUPPLY_CAP_U {
        Err(MoneyError::CapExceeded { cap: SUPPLY_CAP_U, got: total })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skim_is_ten_percent() {
        let (net, fee) = skim(1_000, 1000).unwrap();
        assert_eq!((net, fee), (900, 100));
        assert_eq!(net + fee, 1_000);
    }

    #[test]
    fn skim_rejects_bad_rate() {
        assert_eq!(skim(100, 10_001), Err(MoneyError::BadRate));
    }

    #[test]
    fn settle_conserves_supply() {
        let mut t = Treasury::new();
        t.credit("alice", NATIVE, 1_000).unwrap();
        let before = t.supply(NATIVE);
        t.settle("alice", "bob", NATIVE, 400).unwrap();
        assert_eq!(t.balance("alice", NATIVE), 600);
        assert_eq!(t.balance("bob", NATIVE), 400);
        assert_eq!(t.supply(NATIVE), before);
    }

    #[test]
    fn debit_insufficient_fails_loud() {
        let mut t = Treasury::new();
        t.credit("alice", NATIVE, 10).unwrap();
        assert!(matches!(t.debit("alice", NATIVE, 11), Err(MoneyError::Insufficient { .. })));
    }

    #[test]
    fn btc_route_converts() {
        // 2 QUG (= 2_000_000 µ) at 500 sats/QUG = 1000 sats
        assert_eq!(btc_route(2 * MICRO, 500).unwrap(), 1000);
    }

    #[test]
    fn cap_enforced() {
        assert!(check_cap(SUPPLY_CAP_U).is_ok());
        assert!(matches!(check_cap(SUPPLY_CAP_U + 1), Err(MoneyError::CapExceeded { .. })));
    }
}
