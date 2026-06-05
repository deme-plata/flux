//! U256 — fixed-width 256-bit unsigned integer as 4 little-endian u64 limbs.
//! Vendored (no deps), `const fn` throughout, no_std-friendly. The foundation for
//! exact money math (`Amount`) and the ledger accumulator root.

#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct U256(pub [u64; 4]); // limbs[0] = least significant

impl U256 {
    pub const ZERO: U256 = U256([0, 0, 0, 0]);
    pub const ONE: U256 = U256([1, 0, 0, 0]);
    pub const MAX: U256 = U256([u64::MAX; 4]);

    pub const fn from_u64(v: u64) -> U256 { U256([v, 0, 0, 0]) }
    pub const fn from_u128(v: u128) -> U256 { U256([v as u64, (v >> 64) as u64, 0, 0]) }

    pub const fn is_zero(self) -> bool {
        self.0[0] == 0 && self.0[1] == 0 && self.0[2] == 0 && self.0[3] == 0
    }

    /// Value as u128 if it fits (high 128 bits are zero), else None.
    pub const fn as_u128(self) -> Option<u128> {
        if self.0[2] == 0 && self.0[3] == 0 {
            Some((self.0[0] as u128) | ((self.0[1] as u128) << 64))
        } else {
            None
        }
    }

    /// Addition with carry-out (wrapping value + overflow flag). const.
    pub const fn overflowing_add(self, rhs: U256) -> (U256, bool) {
        let mut res = [0u64; 4];
        let mut carry = 0u64;
        let mut i = 0;
        while i < 4 {
            let (s1, c1) = self.0[i].overflowing_add(rhs.0[i]);
            let (s2, c2) = s1.overflowing_add(carry);
            res[i] = s2;
            carry = (c1 as u64) + (c2 as u64);
            i += 1;
        }
        (U256(res), carry != 0)
    }

    /// Subtraction with borrow-out (wrapping value + underflow flag). const.
    pub const fn overflowing_sub(self, rhs: U256) -> (U256, bool) {
        let mut res = [0u64; 4];
        let mut borrow = 0u64;
        let mut i = 0;
        while i < 4 {
            let (d1, b1) = self.0[i].overflowing_sub(rhs.0[i]);
            let (d2, b2) = d1.overflowing_sub(borrow);
            res[i] = d2;
            borrow = (b1 as u64) + (b2 as u64);
            i += 1;
        }
        (U256(res), borrow != 0)
    }

    /// Checked add — None on overflow. Money never wraps silently.
    pub const fn checked_add(self, rhs: U256) -> Option<U256> {
        let (v, o) = self.overflowing_add(rhs);
        if o { None } else { Some(v) }
    }
    /// Checked sub — None on underflow (would go negative).
    pub const fn checked_sub(self, rhs: U256) -> Option<U256> {
        let (v, u) = self.overflowing_sub(rhs);
        if u { None } else { Some(v) }
    }

    pub const fn saturating_add(self, rhs: U256) -> U256 {
        let (v, o) = self.overflowing_add(rhs);
        if o { U256::MAX } else { v }
    }
    pub const fn saturating_sub(self, rhs: U256) -> U256 {
        let (v, u) = self.overflowing_sub(rhs);
        if u { U256::ZERO } else { v }
    }

    /// Schoolbook multiply, returning (low 256, overflow if high limbs nonzero). const.
    pub const fn overflowing_mul(self, rhs: U256) -> (U256, bool) {
        let mut acc = [0u128; 8];
        let mut i = 0;
        while i < 4 {
            let mut j = 0;
            while j < 4 {
                acc[i + j] += (self.0[i] as u128) * (rhs.0[j] as u128);
                j += 1;
            }
            i += 1;
        }
        // normalise carries
        let mut limbs = [0u64; 8];
        let mut carry: u128 = 0;
        let mut k = 0;
        while k < 8 {
            let cur = acc[k] + carry;
            limbs[k] = cur as u64;
            carry = cur >> 64;
            k += 1;
        }
        let low = U256([limbs[0], limbs[1], limbs[2], limbs[3]]);
        let overflow = limbs[4] != 0 || limbs[5] != 0 || limbs[6] != 0 || limbs[7] != 0;
        (low, overflow)
    }

    /// 3-way compare without traits (const). -1 lt, 0 eq, 1 gt.
    pub const fn cmp_const(self, rhs: U256) -> i8 {
        let mut i = 4;
        while i > 0 {
            i -= 1;
            if self.0[i] < rhs.0[i] { return -1; }
            if self.0[i] > rhs.0[i] { return 1; }
        }
        0
    }

    /// Big-endian 32 bytes (e.g. a BLAKE3 hash → U256).
    pub const fn from_be_bytes(b: [u8; 32]) -> U256 {
        let mut limbs = [0u64; 4];
        let mut i = 0;
        while i < 4 {
            let off = (3 - i) * 8;
            limbs[i] = ((b[off] as u64) << 56) | ((b[off + 1] as u64) << 48)
                | ((b[off + 2] as u64) << 40) | ((b[off + 3] as u64) << 32)
                | ((b[off + 4] as u64) << 24) | ((b[off + 5] as u64) << 16)
                | ((b[off + 6] as u64) << 8) | (b[off + 7] as u64);
            i += 1;
        }
        U256(limbs)
    }
}

impl core::cmp::PartialOrd for U256 {
    fn partial_cmp(&self, o: &U256) -> Option<core::cmp::Ordering> { Some(self.cmp(o)) }
}
impl core::cmp::Ord for U256 {
    fn cmp(&self, o: &U256) -> core::cmp::Ordering {
        match self.cmp_const(*o) { -1 => core::cmp::Ordering::Less, 1 => core::cmp::Ordering::Greater, _ => core::cmp::Ordering::Equal }
    }
}
impl core::fmt::Debug for U256 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "U256(0x{:016x}{:016x}{:016x}{:016x})", self.0[3], self.0[2], self.0[1], self.0[0])
    }
}
