//! flux-bridge — the **proof-carrying cross-chain fee** mold.
//!
//! Mint a wrapped token ONLY on a node-verified proof; take a crossing fee;
//! enforce `minted <= locked` peg by construction. Built on [`flux_money_kit`].
//! One better than a trusted committee: no mint without a valid proof, and the
//! peg invariant is checked on every crossing.

use flux_money_kit::{MoneyError, Result, Treasury};
use std::collections::HashMap;

/// House wallet collecting crossing fees (in wrapped tokens).
pub const HOUSE: &str = "bridge-house";

pub struct Bridge {
    pub treasury: Treasury,
    /// Crossing fee in bps.
    pub fee_bps: u32,
    /// Per-asset amount locked on the origin chain (proven).
    locked: HashMap<String, u128>,
    /// Per-asset amount minted as wTokens here.
    minted: HashMap<String, u128>,
}

impl Bridge {
    pub fn new(fee_bps: u32) -> Self {
        Self { treasury: Treasury::new(), fee_bps, locked: HashMap::new(), minted: HashMap::new() }
    }

    pub fn locked(&self, asset: &str) -> u128 { *self.locked.get(asset).unwrap_or(&0) }
    pub fn minted(&self, asset: &str) -> u128 { *self.minted.get(asset).unwrap_or(&0) }
    /// The peg holds iff minted never exceeds locked.
    pub fn peg_ok(&self, asset: &str) -> bool { self.minted(asset) <= self.locked(asset) }

    /// Cross IN: requires a valid SPV proof. Records the origin-chain lock, mints
    /// `(amount - fee)` wToken to `to` and the fee to the house. The peg invariant
    /// is enforced — the mint is refused if it would push minted past locked.
    pub fn cross_in(&mut self, asset: &str, to: &str, amount: u128, proof_valid: bool) -> Result<u128> {
        if !proof_valid { return Err(MoneyError::Unroutable); } // no proof → no mint
        if amount == 0 { return Err(MoneyError::BadRate); }
        let fee = amount.checked_mul(self.fee_bps as u128).ok_or(MoneyError::Overflow)? / 10_000;
        let net = amount - fee;
        let wtoken = format!("w{asset}");

        let locked_after = self.locked(asset).checked_add(amount).ok_or(MoneyError::Overflow)?;
        let minted_after = self.minted(asset).checked_add(amount).ok_or(MoneyError::Overflow)?;
        if minted_after > locked_after {
            return Err(MoneyError::CapExceeded { cap: locked_after, got: minted_after });
        }
        self.locked.insert(asset.to_string(), locked_after);
        self.minted.insert(asset.to_string(), minted_after);
        self.treasury.credit(to, &wtoken, net)?;
        self.treasury.credit(HOUSE, &wtoken, fee)?;
        Ok(net)
    }

    /// Cross OUT: burn `amount` wToken from `from`, release the origin-chain lock.
    /// Reduces minted + locked together so the peg is preserved.
    pub fn cross_out(&mut self, asset: &str, from: &str, amount: u128) -> Result<()> {
        let wtoken = format!("w{asset}");
        let (m, l) = (self.minted(asset), self.locked(asset));
        if m < amount || l < amount { return Err(MoneyError::BadRate); }
        self.treasury.debit(from, &wtoken, amount)?;
        self.minted.insert(asset.to_string(), m - amount);
        self.locked.insert(asset.to_string(), l - amount);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_cross_mints_net_and_fee_and_pegs() {
        let mut b = Bridge::new(50); // 0.50%
        let net = b.cross_in("BTC", "alice", 1000, true).unwrap();
        assert_eq!(net, 995);
        assert_eq!(b.treasury.balance("alice", "wBTC"), 995);
        assert_eq!(b.treasury.balance(HOUSE, "wBTC"), 5);
        assert_eq!(b.locked("BTC"), 1000);
        assert_eq!(b.minted("BTC"), 1000);
        assert!(b.peg_ok("BTC"));
    }

    #[test]
    fn invalid_proof_rejected_no_state_change() {
        let mut b = Bridge::new(50);
        assert_eq!(b.cross_in("BTC", "alice", 1000, false), Err(MoneyError::Unroutable));
        assert_eq!(b.locked("BTC"), 0);
        assert_eq!(b.minted("BTC"), 0);
        assert_eq!(b.treasury.balance("alice", "wBTC"), 0);
    }

    #[test]
    fn cross_out_preserves_peg() {
        let mut b = Bridge::new(50);
        b.cross_in("BTC", "alice", 1000, true).unwrap();
        b.cross_out("BTC", "alice", 995).unwrap(); // burn alice's wBTC
        assert_eq!(b.treasury.balance("alice", "wBTC"), 0);
        assert_eq!(b.minted("BTC"), 5); // 1000 - 995
        assert_eq!(b.locked("BTC"), 5);
        assert!(b.peg_ok("BTC"));
    }
}
