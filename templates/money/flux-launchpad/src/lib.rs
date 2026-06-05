//! flux-launchpad — the **mint → LP → vest** mold.
//!
//! Deploy a token, charge a launch fee, and bootstrap a founding LP estate that
//! earns 0.30% on every swap forever. Built on [`flux_money_kit`]. Supply is
//! cap-enforced at mint. The constant-product pool keeps its fee, so `k` grows.

use flux_money_kit::{check_cap, MoneyError, Result, Treasury, NATIVE};

/// House wallet collecting launch fees + owning the founding LP estates.
pub const HOUSE: &str = "launchpad-house";
/// Default LP fee: 0.30% = 30 bps.
pub const LP_FEE_BPS: u32 = 30;

/// What to mint.
#[derive(Debug, Clone)]
pub struct TokenSpec {
    pub symbol: String,
    /// Total supply in µ.
    pub supply: u128,
    pub decimals: u8,
}

/// Proof a launch happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchReceipt {
    pub symbol: String,
    pub founder: String,
    pub supply: u128,
    pub fee_paid: u128,
}

/// A constant-product LP estate (token ↔ QUG).
#[derive(Debug, Clone)]
pub struct Lp {
    pub token: String,
    pub reserve_token: u128,
    pub reserve_qug: u128,
    pub lp_shares: u128,
    pub fee_bps: u32,
    /// Lifetime QUG fees retained by the pool (reporting counter).
    pub accrued_fees_qug: u128,
}

/// Swap direction.
pub enum Dir { QugToToken, TokenToQug }

/// The launchpad. Holds the [`Treasury`], a flat launch fee, and its LP estates.
pub struct Launchpad {
    pub treasury: Treasury,
    /// Flat launch fee in µQUG.
    pub launch_fee: u128,
    pools: Vec<Lp>,
}

impl Launchpad {
    pub fn new(launch_fee: u128) -> Self {
        Self { treasury: Treasury::new(), launch_fee, pools: Vec::new() }
    }

    /// Deploy a token: founder pays the launch fee (QUG) to the house, then the
    /// new supply is minted to the founder. Cap-enforced before any state change.
    pub fn launch(&mut self, founder: &str, spec: &TokenSpec) -> Result<LaunchReceipt> {
        check_cap(spec.supply)?;
        self.treasury.settle(founder, HOUSE, NATIVE, self.launch_fee)?;
        self.treasury.credit(founder, &spec.symbol, spec.supply)?;
        Ok(LaunchReceipt {
            symbol: spec.symbol.clone(), founder: founder.to_string(),
            supply: spec.supply, fee_paid: self.launch_fee,
        })
    }

    /// Bootstrap a founding LP estate. The founder deposits both sides; the pool
    /// is the house's estate (it accrues fees). Returns the pool index.
    pub fn bootstrap_lp(&mut self, founder: &str, token: &str, amount_token: u128, amount_qug: u128) -> Result<usize> {
        if amount_token == 0 || amount_qug == 0 { return Err(MoneyError::BadRate); }
        self.treasury.debit(founder, token, amount_token)?;
        self.treasury.debit(founder, NATIVE, amount_qug)?;
        self.pools.push(Lp {
            token: token.to_string(),
            reserve_token: amount_token,
            reserve_qug: amount_qug,
            lp_shares: amount_qug, // simple monotone share basis
            fee_bps: LP_FEE_BPS,
            accrued_fees_qug: 0,
        });
        Ok(self.pools.len() - 1)
    }

    pub fn pool(&self, idx: usize) -> Option<&Lp> { self.pools.get(idx) }

    /// Constant-product swap with a 0.30% fee retained by the pool (the estate
    /// earns). Trader funds/credits flow through the treasury. Returns amount_out.
    pub fn swap(&mut self, idx: usize, trader: &str, dir: Dir, amount_in: u128) -> Result<u128> {
        // Snapshot the pool (immutable borrow) before touching the treasury.
        let (token, fee_bps, reserve_token, reserve_qug) = {
            let p = self.pools.get(idx).ok_or(MoneyError::BadRate)?;
            (p.token.clone(), p.fee_bps, p.reserve_token, p.reserve_qug)
        };
        let fee = amount_in.checked_mul(fee_bps as u128).ok_or(MoneyError::Overflow)? / 10_000;
        let amount_in_net = amount_in - fee;

        let (out, new_rt, new_rq, fee_qug) = match dir {
            Dir::QugToToken => {
                let out = reserve_token.checked_mul(amount_in_net).ok_or(MoneyError::Overflow)?
                    / (reserve_qug + amount_in_net);
                // full amount_in (incl. fee) stays in the pool; token `out` leaves.
                (out, reserve_token - out, reserve_qug + amount_in, fee)
            }
            Dir::TokenToQug => {
                let out = reserve_qug.checked_mul(amount_in_net).ok_or(MoneyError::Overflow)?
                    / (reserve_token + amount_in_net);
                (out, reserve_token + amount_in, reserve_qug - out, 0)
            }
        };

        // Settle through the treasury.
        match dir {
            Dir::QugToToken => {
                self.treasury.debit(trader, NATIVE, amount_in)?;
                self.treasury.credit(trader, &token, out)?;
            }
            Dir::TokenToQug => {
                self.treasury.debit(trader, &token, amount_in)?;
                self.treasury.credit(trader, NATIVE, out)?;
            }
        }

        // Commit the new reserves + fee accrual.
        let p = self.pools.get_mut(idx).ok_or(MoneyError::BadRate)?;
        p.reserve_token = new_rt;
        p.reserve_qug = new_rq;
        p.accrued_fees_qug += fee_qug;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux_money_kit::{MICRO, SUPPLY_CAP_U};

    fn lp() -> Launchpad {
        let mut l = Launchpad::new(10 * MICRO); // 10 QUG launch fee
        l.treasury.credit("founder", NATIVE, 1000 * MICRO).unwrap();
        l
    }

    #[test]
    fn launch_charges_fee_and_mints() {
        let mut l = lp();
        let spec = TokenSpec { symbol: "ACME".into(), supply: 1_000_000 * MICRO, decimals: 6 };
        let r = l.launch("founder", &spec).unwrap();
        assert_eq!(r.fee_paid, 10 * MICRO);
        assert_eq!(l.treasury.balance(HOUSE, NATIVE), 10 * MICRO);
        assert_eq!(l.treasury.balance("founder", "ACME"), 1_000_000 * MICRO);
        assert_eq!(l.treasury.balance("founder", NATIVE), 990 * MICRO);
    }

    #[test]
    fn launch_rejects_over_cap() {
        let mut l = lp();
        let spec = TokenSpec { symbol: "BIG".into(), supply: SUPPLY_CAP_U + 1, decimals: 6 };
        assert!(matches!(l.launch("founder", &spec), Err(MoneyError::CapExceeded { .. })));
        // failed launch must not have charged the fee
        assert_eq!(l.treasury.balance("founder", NATIVE), 1000 * MICRO);
    }

    #[test]
    fn bootstrap_and_swap_accrues_fee_and_grows_k() {
        let mut l = lp();
        let spec = TokenSpec { symbol: "ACME".into(), supply: 1_000_000 * MICRO, decimals: 6 };
        l.launch("founder", &spec).unwrap();
        let idx = l.bootstrap_lp("founder", "ACME", 100_000 * MICRO, 100 * MICRO).unwrap();
        let p0 = l.pool(idx).unwrap().clone();
        let k0 = p0.reserve_token * p0.reserve_qug;

        l.treasury.credit("trader", NATIVE, 10 * MICRO).unwrap();
        let out = l.swap(idx, "trader", Dir::QugToToken, 10 * MICRO).unwrap();
        assert!(out > 0);

        let p1 = l.pool(idx).unwrap();
        let k1 = p1.reserve_token * p1.reserve_qug;
        assert!(k1 > k0, "constant product must grow from fees: {k1} <= {k0}");
        assert!(p1.accrued_fees_qug > 0);
        assert_eq!(l.treasury.balance("trader", "ACME"), out);
        assert_eq!(l.treasury.balance("trader", NATIVE), 0);
    }
}
