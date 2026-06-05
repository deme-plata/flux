//! Amount — exact money in øre (1/100 DKK). Never negative, never wraps.
//!
//! Invariant: the inner U256 is always <= u128::MAX. A single balance can never
//! exceed ~3.4e36 DKK, which is plenty; the LEDGER TOTAL (sum of many) uses U512.
//! Every public op enforces the invariant — overflow/underflow return None, never
//! a silent wrap. This is the type-level guard behind "balances never corrupt".

use crate::u256::U256;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Debug)]
pub struct Amount(U256); // invariant: 0 <= inner <= u128::MAX

impl Amount {
    pub const ZERO: Amount = Amount(U256::ZERO);
    pub const MAX: Amount = Amount(U256::from_u128(u128::MAX));

    /// The only constructor — øre always fit the invariant.
    pub const fn from_ore(ore: u128) -> Amount { Amount(U256::from_u128(ore)) }

    /// DKK kroner → øre (×100), checked.
    pub const fn from_kroner(kr: u128) -> Option<Amount> {
        match kr.checked_mul(100) {
            Some(ore) => Some(Amount::from_ore(ore)),
            None => None,
        }
    }

    pub const fn as_ore(self) -> u128 {
        match self.0.as_u128() { Some(v) => v, None => u128::MAX } // invariant guarantees Some
    }

    pub const fn is_zero(self) -> bool { self.0.is_zero() }

    /// Add — None if the result would exceed the single-balance ceiling (u128::MAX).
    pub const fn checked_add(self, rhs: Amount) -> Option<Amount> {
        match self.0.checked_add(rhs.0) {
            Some(v) => {
                // keep the <= u128::MAX invariant
                if v.cmp_const(U256::from_u128(u128::MAX)) == 1 { None } else { Some(Amount(v)) }
            }
            None => None,
        }
    }

    /// Subtract — None on underflow (balance would go negative). The overdraw guard.
    pub const fn checked_sub(self, rhs: Amount) -> Option<Amount> {
        match self.0.checked_sub(rhs.0) {
            Some(v) => Some(Amount(v)),
            None => None,
        }
    }

    pub const fn saturating_add(self, rhs: Amount) -> Amount {
        match self.checked_add(rhs) { Some(v) => v, None => Amount::MAX }
    }
    pub const fn saturating_sub(self, rhs: Amount) -> Amount {
        match self.checked_sub(rhs) { Some(v) => v, None => Amount::ZERO }
    }

    /// The leaf this balance contributes to the ledger accumulator: hash-domain U256.
    pub const fn inner(self) -> U256 { self.0 }
}
