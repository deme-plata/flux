//! The basement gold vault.
//!
//! The trick is this: a vault's real product is not storage, it is *provable
//! custody*. So every operation — deposit, withdrawal, audit — is an entry in
//! a BLAKE3 hash chain: each entry's hash commits to the previous head plus
//! the operation's bytes. Rewrite any historical entry and `verify_chain()`
//! breaks at exactly that link.
//!
//! **Now, here's the thing that should bother you** (v0.1 got this wrong):
//! a hash chain alone proves integrity relative to a head you already trust.
//! A custodian who controls the WHOLE database doesn't need a collision to
//! rewrite history — they change entry 500 and recompute every later link,
//! producing a different but perfectly valid chain. The fix is external
//! **witnessing**: periodically publish an anchor commitment
//! `A = BLAKE3(head ‖ inventory_root ‖ tick)` somewhere the custodian cannot
//! rewrite (the Quillon Graph chain — the tower is a full node, and the
//! light column's stronger pulse marks each committed anchor). After that,
//! history *before the last anchor* is publicly frozen: `verify_witnessed()`
//! catches the full recompute attack that `verify_chain()` cannot. History
//! after the newest anchor remains rewritable until the next pulse — a
//! window we test for and state, not hide.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Canonical capacity of the two vault floors (B4–B3), in Good Delivery bars.
pub const VAULT_CAPACITY_BARS: usize = 8_000;

/// LBMA Good Delivery: ~12.4 kg, fineness at least 995.0 ‰ (995_000 ppm).
pub const MIN_FINENESS_PPM: u32 = 995_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoldBar {
    pub serial: String,
    pub weight_g: u32,
    /// Parts-per-million gold content, e.g. 999_900 for four nines.
    pub fineness_ppm: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Clearance {
    /// Bank officers: may deposit.
    Officer,
    /// Custodian quorum: may deposit and withdraw.
    Custodian,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VaultOp {
    Deposit { bar: GoldBar, actor: String },
    Withdraw { serial: String, actor: String },
    Audit { actor: String, bars_counted: usize },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Simulation tick (deterministic time — no wall clocks in the chain).
    pub tick: u64,
    pub op: VaultOp,
    /// blake3(prev_head ‖ serde_json(op) ‖ tick_le_bytes)
    pub head: [u8; 32],
}

#[derive(Debug, Error, PartialEq)]
pub enum VaultError {
    #[error("vault full: {0} bars")]
    Full(usize),
    #[error("bar {0} not found")]
    NotFound(String),
    #[error("duplicate serial {0}")]
    DuplicateSerial(String),
    #[error("fineness {0} ppm below Good Delivery minimum")]
    SubStandard(u32),
    #[error("clearance {0:?} may not perform this operation")]
    Denied(Clearance),
}

/// An external witness record: what was published (to Quillon Graph, and
/// periodically onward to Bitcoin) and where the chain stood when it was.
/// The wire to the live chain is a seam (SPECIFIED); the commitment math and
/// its verification are PROVEN here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Anchor {
    pub tick: u64,
    /// Number of audit entries covered (the anchor freezes entries [0, n)).
    pub entries_covered: usize,
    /// BLAKE3(head ‖ inventory_root ‖ tick)
    pub commitment: [u8; 32],
}

pub struct Vault {
    capacity: usize,
    bars: Vec<GoldBar>,
    audit: Vec<AuditEntry>,
    head: [u8; 32],
    anchors: Vec<Anchor>,
}

fn chain(prev: &[u8; 32], op: &VaultOp, tick: u64) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(prev);
    h.update(serde_json::to_string(op).expect("vault op serializes").as_bytes());
    h.update(&tick.to_le_bytes());
    *h.finalize().as_bytes()
}

/// Binary BLAKE3 Merkle root over leaf hashes (odd node promoted).
fn merkle_root(mut layer: Vec<[u8; 32]>) -> [u8; 32] {
    if layer.is_empty() {
        return [0u8; 32];
    }
    while layer.len() > 1 {
        let mut next = Vec::with_capacity(layer.len().div_ceil(2));
        for pair in layer.chunks(2) {
            if pair.len() == 2 {
                let mut h = blake3::Hasher::new();
                h.update(&pair[0]);
                h.update(&pair[1]);
                next.push(*h.finalize().as_bytes());
            } else {
                next.push(pair[0]);
            }
        }
        layer = next;
    }
    layer[0]
}

impl Vault {
    pub fn new(capacity: usize) -> Self {
        Self { capacity, bars: Vec::new(), audit: Vec::new(), head: [0u8; 32], anchors: Vec::new() }
    }

    /// Merkle root of the current physical inventory (sorted serials).
    pub fn inventory_root(&self) -> [u8; 32] {
        let mut serials: Vec<&str> = self.bars.iter().map(|b| b.serial.as_str()).collect();
        serials.sort_unstable();
        merkle_root(serials.iter().map(|s| *blake3::hash(s.as_bytes()).as_bytes()).collect())
    }

    /// Publish an external anchor: freeze everything up to now. Returns the
    /// commitment the full node commits to Quillon Graph (the beacon's
    /// stronger pulse).
    pub fn anchor(&mut self, tick: u64) -> Anchor {
        let mut h = blake3::Hasher::new();
        h.update(&self.head);
        h.update(&self.inventory_root());
        h.update(&tick.to_le_bytes());
        let a = Anchor { tick, entries_covered: self.audit.len(), commitment: *h.finalize().as_bytes() };
        self.anchors.push(a.clone());
        a
    }

    pub fn anchors(&self) -> &[Anchor] {
        &self.anchors
    }

    /// Verify the log against the EXTERNAL witnesses: replay the operations,
    /// rebuilding both the hash chain and the physical inventory, and at each
    /// anchor point recompute the commitment. Catches the full-recompute
    /// rewrite that `verify_chain()` alone cannot — for everything up to the
    /// newest anchor. Returns the index of the first anchor that fails.
    pub fn verify_witnessed(&self) -> Result<usize, usize> {
        let mut head = [0u8; 32];
        let mut inventory: Vec<String> = Vec::new();
        let mut entry_idx = 0usize;
        for (ai, a) in self.anchors.iter().enumerate() {
            while entry_idx < a.entries_covered {
                let e = match self.audit.get(entry_idx) {
                    Some(e) => e,
                    None => return Err(ai),
                };
                head = chain(&head, &e.op, e.tick);
                match &e.op {
                    VaultOp::Deposit { bar, .. } => inventory.push(bar.serial.clone()),
                    VaultOp::Withdraw { serial, .. } => inventory.retain(|s| s != serial),
                    VaultOp::Audit { .. } => {}
                }
                entry_idx += 1;
            }
            let mut serials: Vec<&str> = inventory.iter().map(String::as_str).collect();
            serials.sort_unstable();
            let inv_root =
                merkle_root(serials.iter().map(|s| *blake3::hash(s.as_bytes()).as_bytes()).collect());
            let mut h = blake3::Hasher::new();
            h.update(&head);
            h.update(&inv_root);
            h.update(&a.tick.to_le_bytes());
            if *h.finalize().as_bytes() != a.commitment {
                return Err(ai);
            }
        }
        Ok(self.anchors.len())
    }

    fn record(&mut self, op: VaultOp, tick: u64) {
        self.head = chain(&self.head, &op, tick);
        self.audit.push(AuditEntry { tick, op, head: self.head });
    }

    pub fn deposit(&mut self, bar: GoldBar, actor: &str, _who: Clearance, tick: u64) -> Result<(), VaultError> {
        if self.bars.len() >= self.capacity {
            return Err(VaultError::Full(self.bars.len()));
        }
        if bar.fineness_ppm < MIN_FINENESS_PPM {
            return Err(VaultError::SubStandard(bar.fineness_ppm));
        }
        if self.bars.iter().any(|b| b.serial == bar.serial) {
            return Err(VaultError::DuplicateSerial(bar.serial));
        }
        self.record(VaultOp::Deposit { bar: bar.clone(), actor: actor.into() }, tick);
        self.bars.push(bar);
        Ok(())
    }

    pub fn withdraw(&mut self, serial: &str, actor: &str, who: Clearance, tick: u64) -> Result<GoldBar, VaultError> {
        if who != Clearance::Custodian {
            return Err(VaultError::Denied(who));
        }
        let idx = self
            .bars
            .iter()
            .position(|b| b.serial == serial)
            .ok_or_else(|| VaultError::NotFound(serial.into()))?;
        let bar = self.bars.remove(idx);
        self.record(VaultOp::Withdraw { serial: serial.into(), actor: actor.into() }, tick);
        Ok(bar)
    }

    /// A counted audit is itself a chained event — auditors leave fingerprints.
    pub fn run_audit(&mut self, actor: &str, tick: u64) -> usize {
        let n = self.bars.len();
        self.record(VaultOp::Audit { actor: actor.into(), bars_counted: n }, tick);
        n
    }

    pub fn holdings_kg(&self) -> f64 {
        self.bars.iter().map(|b| b.weight_g as f64).sum::<f64>() / 1_000.0
    }

    pub fn bar_count(&self) -> usize {
        self.bars.len()
    }

    pub fn head_hex(&self) -> String {
        hex::encode(self.head)
    }

    pub fn audit_log(&self) -> &[AuditEntry] {
        &self.audit
    }

    /// Recompute the whole chain from genesis. Returns the index of the first
    /// broken link, or Ok(entries) when the history is intact.
    pub fn verify_chain(&self) -> Result<usize, usize> {
        let mut prev = [0u8; 32];
        for (i, e) in self.audit.iter().enumerate() {
            let expect = chain(&prev, &e.op, e.tick);
            if expect != e.head {
                return Err(i);
            }
            prev = e.head;
        }
        Ok(self.audit.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(serial: &str) -> GoldBar {
        GoldBar { serial: serial.into(), weight_g: 12_400, fineness_ppm: 999_900 }
    }

    #[test]
    fn deposit_withdraw_roundtrip_and_chain_verifies() {
        let mut v = Vault::new(10);
        v.deposit(bar("AU-0001"), "quillon-bank", Clearance::Officer, 1).unwrap();
        v.deposit(bar("AU-0002"), "quillon-bank", Clearance::Officer, 2).unwrap();
        assert_eq!(v.bar_count(), 2);
        assert!((v.holdings_kg() - 24.8).abs() < 1e-9);
        let b = v.withdraw("AU-0001", "custodian-quorum", Clearance::Custodian, 3).unwrap();
        assert_eq!(b.serial, "AU-0001");
        assert_eq!(v.verify_chain(), Ok(3));
    }

    #[test]
    fn officer_cannot_withdraw() {
        let mut v = Vault::new(10);
        v.deposit(bar("AU-1"), "bank", Clearance::Officer, 1).unwrap();
        assert_eq!(
            v.withdraw("AU-1", "bank", Clearance::Officer, 2),
            Err(VaultError::Denied(Clearance::Officer))
        );
    }

    #[test]
    fn substandard_and_duplicate_bars_rejected() {
        let mut v = Vault::new(10);
        let mut fake = bar("AU-9");
        fake.fineness_ppm = 900_000;
        assert_eq!(v.deposit(fake, "x", Clearance::Officer, 1), Err(VaultError::SubStandard(900_000)));
        v.deposit(bar("AU-2"), "x", Clearance::Officer, 2).unwrap();
        assert_eq!(
            v.deposit(bar("AU-2"), "x", Clearance::Officer, 3),
            Err(VaultError::DuplicateSerial("AU-2".into()))
        );
    }

    /// The custodian's real attack: rewrite one op, keep the inventory
    /// record CONSISTENT with the forgery, then recompute EVERY later link
    /// so the internal chain is valid again.
    fn full_recompute_rewrite(v: &mut Vault, idx: usize, forged_serial: &str) {
        let mut old_serial = String::new();
        if let VaultOp::Deposit { bar, .. } = &mut v.audit[idx].op {
            old_serial = bar.serial.clone();
            bar.serial = forged_serial.into();
        }
        for b in v.bars.iter_mut() {
            if b.serial == old_serial {
                b.serial = forged_serial.into();
            }
        }
        let mut head = [0u8; 32];
        for i in 0..v.audit.len() {
            head = chain(&head, &v.audit[i].op, v.audit[i].tick);
            v.audit[i].head = head;
        }
        v.head = head;
    }

    #[test]
    fn full_recompute_defeats_the_chain_but_not_the_witness() {
        let mut v = Vault::new(10);
        v.deposit(bar("AU-1"), "bank", Clearance::Officer, 1).unwrap();
        v.deposit(bar("AU-2"), "bank", Clearance::Officer, 2).unwrap();
        v.anchor(3); // published externally — the beacon's stronger pulse
        v.deposit(bar("AU-3"), "bank", Clearance::Officer, 4).unwrap();

        full_recompute_rewrite(&mut v, 0, "AU-FORGED");
        // The internal chain is fooled: the attacker rebuilt it consistently.
        assert_eq!(v.verify_chain(), Ok(3));
        // The external witness is not: anchor 0 covered the rewritten entry.
        assert_eq!(v.verify_witnessed(), Err(0));
    }

    #[test]
    fn honest_negative_rewrites_after_the_last_anchor_are_invisible() {
        // This is the stated window, not a hidden one: history AFTER the
        // newest anchor is rewritable until the next pulse freezes it.
        let mut v = Vault::new(10);
        v.deposit(bar("AU-1"), "bank", Clearance::Officer, 1).unwrap();
        v.anchor(2);
        v.deposit(bar("AU-2"), "bank", Clearance::Officer, 3).unwrap();

        full_recompute_rewrite(&mut v, 1, "AU-FORGED");
        assert_eq!(v.verify_chain(), Ok(2));
        assert_eq!(v.verify_witnessed(), Ok(1)); // both pass — the window is real
        // Anchor again: from here the forged entry is frozen too.
        v.anchor(4);
        assert_eq!(v.verify_witnessed(), Ok(2));
    }

    #[test]
    fn anchor_commitment_binds_the_physical_inventory_too() {
        let mut v = Vault::new(10);
        v.deposit(bar("AU-1"), "bank", Clearance::Officer, 1).unwrap();
        v.deposit(bar("AU-2"), "bank", Clearance::Officer, 2).unwrap();
        let a = v.anchor(3);
        assert_eq!(a.entries_covered, 2);
        // A vault with different contents produces a different inventory root,
        // hence a different commitment at the same head/tick.
        let root_before = v.inventory_root();
        v.withdraw("AU-2", "custodian-quorum", Clearance::Custodian, 4).unwrap();
        assert_ne!(root_before, v.inventory_root());
        assert_eq!(v.verify_witnessed(), Ok(1));
    }

    #[test]
    fn tampered_history_is_detected_at_the_exact_link() {
        let mut v = Vault::new(10);
        v.deposit(bar("AU-1"), "bank", Clearance::Officer, 1).unwrap();
        v.deposit(bar("AU-2"), "bank", Clearance::Officer, 2).unwrap();
        v.run_audit("auditor", 3);
        // Rewrite history: claim the first deposit was a different bar.
        if let VaultOp::Deposit { bar: b, .. } = &mut v.audit[0].op {
            b.serial = "AU-FORGED".into();
        }
        assert_eq!(v.verify_chain(), Err(0));
    }
}
