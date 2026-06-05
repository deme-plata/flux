//! flux-estate — the **RWA rent** mold.
//!
//! Tokenize an asset into fractional shares; stream rent to holders pro-rata,
//! minus a 0.30% management fee to the house. Built on [`flux_money_kit`].

use flux_money_kit::{MoneyError, Result, Treasury, NATIVE};
use std::collections::HashMap;

/// House wallet collecting the management fee + rounding dust.
pub const HOUSE: &str = "estate-house";
/// Default management fee: 0.30% = 30 bps.
pub const MGMT_FEE_BPS: u32 = 30;

#[derive(Debug, Clone)]
pub struct Asset {
    pub id: String,
    pub total_shares: u128,
}

/// Tokenized-RWA registry with a rent-distribution engine.
pub struct Estate {
    pub treasury: Treasury,
    pub mgmt_fee_bps: u32,
    assets: HashMap<String, Asset>,
    /// (asset_id, holder) -> share count.
    shares: HashMap<(String, String), u128>,
}

impl Default for Estate {
    fn default() -> Self { Self::new() }
}

impl Estate {
    pub fn new() -> Self {
        Self {
            treasury: Treasury::new(),
            mgmt_fee_bps: MGMT_FEE_BPS,
            assets: HashMap::new(),
            shares: HashMap::new(),
        }
    }

    /// Register an asset and issue all its shares to `owner`.
    pub fn register(&mut self, id: &str, total_shares: u128, owner: &str) -> Result<()> {
        if total_shares == 0 || self.assets.contains_key(id) {
            return Err(MoneyError::BadRate);
        }
        self.assets.insert(id.to_string(), Asset { id: id.to_string(), total_shares });
        self.shares.insert((id.to_string(), owner.to_string()), total_shares);
        Ok(())
    }

    pub fn shares_of(&self, asset: &str, holder: &str) -> u128 {
        *self.shares.get(&(asset.to_string(), holder.to_string())).unwrap_or(&0)
    }

    /// Transfer shares between holders (secondary-market move).
    pub fn transfer_shares(&mut self, asset: &str, from: &str, to: &str, amount: u128) -> Result<()> {
        let have = self.shares_of(asset, from);
        if have < amount {
            return Err(MoneyError::Insufficient {
                wallet: from.into(), token: format!("shares:{asset}"), have, need: amount,
            });
        }
        self.shares.insert((asset.to_string(), from.to_string()), have - amount);
        *self.shares.entry((asset.to_string(), to.to_string())).or_insert(0) += amount;
        Ok(())
    }

    /// Distribute `gross_qug` rent for `asset` pro-rata to holders, minus the
    /// management fee to the house. `payer` must be pre-funded. Returns total
    /// paid to holders. Rounding dust goes to the house (conservation).
    pub fn distribute_rent(&mut self, asset: &str, payer: &str, gross_qug: u128) -> Result<u128> {
        let a = self.assets.get(asset).ok_or(MoneyError::BadRate)?.clone();
        let fee = gross_qug.checked_mul(self.mgmt_fee_bps as u128).ok_or(MoneyError::Overflow)? / 10_000;
        let net = gross_qug - fee;

        let holders: Vec<(String, u128)> = self.shares.iter()
            .filter(|((aid, _), _)| aid == asset)
            .map(|((_, h), s)| (h.clone(), *s))
            .collect();

        self.treasury.debit(payer, NATIVE, gross_qug)?;
        self.treasury.credit(HOUSE, NATIVE, fee)?;

        let mut paid = 0u128;
        for (h, s) in &holders {
            let cut = net.checked_mul(*s).ok_or(MoneyError::Overflow)? / a.total_shares;
            if cut > 0 {
                self.treasury.credit(h, NATIVE, cut)?;
                paid += cut;
            }
        }
        let dust = net - paid;
        if dust > 0 { self.treasury.credit(HOUSE, NATIVE, dust)?; }
        Ok(paid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux_money_kit::MICRO;

    #[test]
    fn register_issues_all_shares() {
        let mut e = Estate::new();
        e.register("flat-1", 100, "owner").unwrap();
        assert_eq!(e.shares_of("flat-1", "owner"), 100);
        assert!(e.register("flat-1", 100, "owner").is_err()); // dup
    }

    #[test]
    fn transfer_then_rent_pro_rata_and_conserves() {
        let mut e = Estate::new();
        e.register("flat-1", 100, "owner").unwrap();
        e.transfer_shares("flat-1", "owner", "bob", 40).unwrap();
        assert_eq!(e.shares_of("flat-1", "owner"), 60);
        assert_eq!(e.shares_of("flat-1", "bob"), 40);

        e.treasury.credit("tenant", NATIVE, 1000 * MICRO).unwrap();
        let before = e.treasury.supply(NATIVE);
        e.distribute_rent("flat-1", "tenant", 1000 * MICRO).unwrap();

        // owner's 60% cut beats bob's 40% cut
        assert!(e.treasury.balance("flat-1-owner-na", NATIVE) == 0); // sanity: no stray key
        let owner_cut = e.treasury.balance("owner", NATIVE);
        let bob_cut = e.treasury.balance("bob", NATIVE);
        assert!(owner_cut > bob_cut, "owner {owner_cut} should beat bob {bob_cut}");
        // house took the 0.30% mgmt fee (+dust)
        assert!(e.treasury.balance(HOUSE, NATIVE) >= 3 * MICRO);
        // total supply conserved (payer funded the whole gross)
        assert_eq!(e.treasury.supply(NATIVE), before);
        assert_eq!(e.treasury.balance("tenant", NATIVE), 0);
    }

    #[test]
    fn over_transfer_rejected() {
        let mut e = Estate::new();
        e.register("flat-1", 100, "owner").unwrap();
        assert!(matches!(
            e.transfer_shares("flat-1", "owner", "bob", 101),
            Err(MoneyError::Insufficient { .. })
        ));
    }
}
