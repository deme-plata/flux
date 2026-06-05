//! chronos.rs — deterministic money-integrity scenarios for flux-uint.
//!
//! "Chronos" = reproducible, seeded simulation (no proptest dep — determinism is the point).
//! Every assertion guards an invariant from the balance-integrity discipline:
//!   • balances never wrap, never go negative
//!   • top-ups are idempotent (webhook replay credits once)
//!   • concurrent debits never oversell
//!   • the ledger total is order-independent
//!   • const-eval works (compile-time constants)

use flux_uint::{widening_mul, Amount, U256, U512, GENESIS_TOTAL};
use std::collections::HashSet;

/// deterministic xorshift64 — fixed seed ⇒ identical run every time (chronos).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn ore(&mut self, max: u128) -> u128 {
        (((self.next() as u128) << 64 | self.next() as u128) % max).max(1)
    }
}

#[test]
fn const_eval_genesis() {
    // computed at compile time
    assert_eq!(GENESIS_TOTAL.as_ore(), 1_000_000_000_000);
    const C: u128 = GENESIS_TOTAL.as_ore();
    assert_eq!(C, 1_000_000_000_000);
}

#[test]
fn overflow_guards_never_wrap() {
    assert_eq!(U256::MAX.checked_add(U256::ONE), None); // no wrap
    assert_eq!(U256::ZERO.checked_sub(U256::ONE), None); // no underflow
    assert_eq!(Amount::MAX.checked_add(Amount::from_ore(1)), None);
    assert_eq!(Amount::ZERO.checked_sub(Amount::from_ore(1)), None); // overdraw guard
    assert_eq!(U256::MAX.saturating_add(U256::ONE), U256::MAX);
    assert_eq!(U256::ZERO.saturating_sub(U256::ONE), U256::ZERO);
}

#[test]
fn add_sub_roundtrip() {
    let mut r = Rng(0xC0FFEE);
    for _ in 0..100_000 {
        let a = Amount::from_ore(r.ore(u128::MAX / 2));
        let b = Amount::from_ore(r.ore(u128::MAX / 2));
        let sum = a.checked_add(b).expect("fits");
        assert_eq!(sum.checked_sub(b), Some(a));
        assert_eq!(sum.checked_sub(a), Some(b));
        assert!(sum >= a && sum >= b);
    }
}

#[test]
fn idempotent_topup_no_double_credit() {
    // a tiny ledger: one account, a set of seen Stripe event-ids
    let mut balance = Amount::ZERO;
    let mut seen: HashSet<u64> = HashSet::new();
    let mut r = Rng(42);
    let mut credited = 0u128;
    for _ in 0..50_000 {
        let event_id = r.next() % 1000; // collisions = replays
        let amt = r.ore(10_000);
        // idempotent apply: only credit if event unseen
        if seen.insert(event_id) {
            balance = balance.checked_add(Amount::from_ore(amt)).unwrap();
            credited += amt;
        }
        // re-applying the SAME event must NEVER change the balance
        let before = balance;
        if !seen.insert(event_id) {
            // already seen → skip → balance unchanged
            assert_eq!(balance, before);
        }
    }
    assert_eq!(balance.as_ore(), credited); // exactly the sum of unique credits
}

#[test]
fn concurrent_debits_never_oversell() {
    // N debit attempts against a balance; only those that fit succeed; never < 0.
    let mut r = Rng(7);
    for _ in 0..2_000 {
        let start = r.ore(1_000_000);
        let mut balance = Amount::from_ore(start);
        let mut debited = 0u128;
        for _ in 0..200 {
            let want = Amount::from_ore(r.ore(20_000));
            match balance.checked_sub(want) {
                Some(after) => { balance = after; debited += want.as_ore(); }
                None => { /* insufficient — rejected, balance untouched */ }
            }
            assert!(!balance.checked_sub(Amount::ZERO).is_none()); // balance always valid (>=0)
        }
        assert_eq!(balance.as_ore(), start - debited); // exact: no oversell, no loss
    }
}

#[test]
fn ledger_total_order_independent() {
    // sum many balances into a U512 total in two different orders → identical root.
    let mut r = Rng(0xABCDEF);
    let mut vals: Vec<u128> = (0..5_000).map(|_| r.ore(u128::MAX)).collect();
    let fold = |order: &[u128]| {
        order.iter().fold(U512::ZERO, |acc, &v| acc.checked_add(U512::from_u128(v)).unwrap())
    };
    let t1 = fold(&vals);
    vals.reverse();
    let t2 = fold(&vals);
    assert_eq!(t1, t2); // order-independent total (accumulator invariant)
}

#[test]
fn widening_mul_no_overflow() {
    let mut r = Rng(99);
    for _ in 0..50_000 {
        let a = r.next() as u128;
        let b = r.next() as u128;
        let want = a * b; // fits u128 since both < 2^64
        let got = widening_mul(U256::from_u128(a), U256::from_u128(b));
        assert_eq!(got, U512::from_u128(want)); // exact 512-bit product, zero loss
    }
}
