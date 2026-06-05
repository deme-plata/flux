//! flux-bank — the **interest-spread** mold.
//!
//! Collateralized lending with an LTV cap; interest accrues to the bank as the
//! spread. Built on [`flux_money_kit`]. Extends the spirit of the on-chain
//! `sigil-bank` keystone into a reusable credit-desk template.

use flux_money_kit::{MoneyError, Result, Treasury, NATIVE};

/// The bank's own wallet (reserve + spread).
pub const BANK: &str = "bank-house";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loan {
    pub id: u64,
    pub borrower: String,
    pub principal: u128,
    pub collateral: u128,
    /// Interest per `accrue` call, in bps of principal.
    pub rate_bps: u32,
    pub accrued: u128,
    pub open: bool,
}

pub struct Bank {
    pub treasury: Treasury,
    /// Max loan-to-value in bps (e.g. 6600 = 66%).
    pub max_ltv_bps: u32,
    loans: Vec<Loan>,
}

impl Bank {
    pub fn new(max_ltv_bps: u32) -> Self {
        Self { treasury: Treasury::new(), max_ltv_bps, loans: Vec::new() }
    }

    /// Open a loan: lock `collateral` QUG (borrower→bank), disburse `principal`
    /// QUG (bank→borrower). Enforces principal/collateral <= max_ltv. The bank
    /// must hold enough reserve to fund the principal.
    pub fn borrow(&mut self, borrower: &str, collateral: u128, principal: u128, rate_bps: u32) -> Result<u64> {
        if collateral == 0 { return Err(MoneyError::BadRate); }
        let ltv = principal.checked_mul(10_000).ok_or(MoneyError::Overflow)? / collateral;
        if ltv > self.max_ltv_bps as u128 { return Err(MoneyError::BadRate); }
        self.treasury.settle(borrower, BANK, NATIVE, collateral)?; // lock
        self.treasury.settle(BANK, borrower, NATIVE, principal)?;  // disburse
        let id = self.loans.len() as u64;
        self.loans.push(Loan {
            id, borrower: borrower.into(), principal, collateral, rate_bps, accrued: 0, open: true,
        });
        Ok(id)
    }

    pub fn loan(&self, id: u64) -> Option<&Loan> { self.loans.get(id as usize) }

    /// Accrue one period of interest (added to what the borrower owes).
    pub fn accrue(&mut self, id: u64) -> Result<u128> {
        let l = self.loans.get_mut(id as usize).ok_or(MoneyError::BadRate)?;
        if !l.open { return Err(MoneyError::BadRate); }
        let interest = l.principal.checked_mul(l.rate_bps as u128).ok_or(MoneyError::Overflow)? / 10_000;
        l.accrued += interest;
        Ok(interest)
    }

    /// Repay principal + accrued interest; release collateral. The interest is
    /// the bank's captured spread. Borrower must hold principal + accrued.
    pub fn repay(&mut self, id: u64) -> Result<()> {
        let (borrower, principal, collateral, accrued) = {
            let l = self.loans.get(id as usize).ok_or(MoneyError::BadRate)?;
            if !l.open { return Err(MoneyError::BadRate); }
            (l.borrower.clone(), l.principal, l.collateral, l.accrued)
        };
        self.treasury.settle(&borrower, BANK, NATIVE, principal + accrued)?; // repay
        self.treasury.settle(BANK, &borrower, NATIVE, collateral)?;          // release
        self.loans[id as usize].open = false;
        Ok(())
    }

    /// Bank's current QUG holdings (reserve + captured spread − open exposure).
    pub fn bank_balance(&self) -> u128 {
        self.treasury.balance(BANK, NATIVE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux_money_kit::MICRO;

    fn bank() -> Bank {
        let mut b = Bank::new(6600); // 66% LTV
        b.treasury.credit(BANK, NATIVE, 1000 * MICRO).unwrap(); // reserve
        b.treasury.credit("bob", NATIVE, 100 * MICRO).unwrap(); // collateral funds
        b
    }

    #[test]
    fn borrow_accrue_repay_captures_spread() {
        let mut b = bank();
        let reserve0 = b.bank_balance();
        let id = b.borrow("bob", 100 * MICRO, 60 * MICRO, 1000).unwrap(); // 60% LTV, 10% interest
        let interest = b.accrue(id).unwrap();
        assert_eq!(interest, 6 * MICRO);
        // bob needs principal(60)+interest(6)=66 to repay; he holds 60 → top up 6
        b.treasury.credit("bob", NATIVE, 6 * MICRO).unwrap();
        b.repay(id).unwrap();
        assert!(!b.loan(id).unwrap().open);
        // bank captured exactly the 6 QUG spread on top of its reserve
        assert_eq!(b.bank_balance(), reserve0 + 6 * MICRO);
        // collateral returned, bob left with the 6 he topped up minus interest = 0
        assert_eq!(b.treasury.balance("bob", NATIVE), 100 * MICRO);
    }

    #[test]
    fn over_ltv_rejected() {
        let mut b = bank();
        assert!(matches!(b.borrow("bob", 100 * MICRO, 70 * MICRO, 1000), Err(MoneyError::BadRate)));
    }
}
