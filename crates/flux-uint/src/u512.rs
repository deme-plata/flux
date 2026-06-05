//! U512 — fixed-width 512-bit unsigned integer (8 little-endian u64 limbs).
//! Used for ledger TOTALS (sum of many u128 balances can exceed 256 bits in theory,
//! and the full 256×256 product needs 512 bits) + accumulator math. Vendored, const fn.

use crate::u256::U256;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct U512(pub [u64; 8]);

impl U512 {
    pub const ZERO: U512 = U512([0; 8]);

    pub const fn from_u256(v: U256) -> U512 {
        U512([v.0[0], v.0[1], v.0[2], v.0[3], 0, 0, 0, 0])
    }
    pub const fn from_u128(v: u128) -> U512 {
        U512([v as u64, (v >> 64) as u64, 0, 0, 0, 0, 0, 0])
    }

    pub const fn is_zero(self) -> bool {
        let mut i = 0;
        while i < 8 { if self.0[i] != 0 { return false; } i += 1; }
        true
    }

    pub const fn overflowing_add(self, rhs: U512) -> (U512, bool) {
        let mut res = [0u64; 8];
        let mut carry = 0u64;
        let mut i = 0;
        while i < 8 {
            let (s1, c1) = self.0[i].overflowing_add(rhs.0[i]);
            let (s2, c2) = s1.overflowing_add(carry);
            res[i] = s2;
            carry = (c1 as u64) + (c2 as u64);
            i += 1;
        }
        (U512(res), carry != 0)
    }

    pub const fn overflowing_sub(self, rhs: U512) -> (U512, bool) {
        let mut res = [0u64; 8];
        let mut borrow = 0u64;
        let mut i = 0;
        while i < 8 {
            let (d1, b1) = self.0[i].overflowing_sub(rhs.0[i]);
            let (d2, b2) = d1.overflowing_sub(borrow);
            res[i] = d2;
            borrow = (b1 as u64) + (b2 as u64);
            i += 1;
        }
        (U512(res), borrow != 0)
    }

    pub const fn checked_add(self, rhs: U512) -> Option<U512> {
        let (v, o) = self.overflowing_add(rhs);
        if o { None } else { Some(v) }
    }

    pub const fn cmp_const(self, rhs: U512) -> i8 {
        let mut i = 8;
        while i > 0 {
            i -= 1;
            if self.0[i] < rhs.0[i] { return -1; }
            if self.0[i] > rhs.0[i] { return 1; }
        }
        0
    }
}

/// Full 256×256 → 512 product — NO overflow loss. The honest widening multiply.
pub const fn widening_mul(a: U256, b: U256) -> U512 {
    let mut acc = [0u128; 8];
    let mut i = 0;
    while i < 4 {
        let mut j = 0;
        while j < 4 {
            acc[i + j] += (a.0[i] as u128) * (b.0[j] as u128);
            j += 1;
        }
        i += 1;
    }
    let mut limbs = [0u64; 8];
    let mut carry: u128 = 0;
    let mut k = 0;
    while k < 8 {
        let cur = acc[k] + carry;
        limbs[k] = cur as u64;
        carry = cur >> 64;
        k += 1;
    }
    U512(limbs)
}

impl core::cmp::PartialOrd for U512 {
    fn partial_cmp(&self, o: &U512) -> Option<core::cmp::Ordering> { Some(self.cmp(o)) }
}
impl core::cmp::Ord for U512 {
    fn cmp(&self, o: &U512) -> core::cmp::Ordering {
        match self.cmp_const(*o) { -1 => core::cmp::Ordering::Less, 1 => core::cmp::Ordering::Greater, _ => core::cmp::Ordering::Equal }
    }
}
impl core::fmt::Debug for U512 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "U512(hi={:?} lo={:?})", &self.0[4..8], &self.0[0..4])
    }
}
