//! flux-aether v2 — private, self-healing, *paid* storage primitives.
//!
//! v1 hid files (K-of-N erasure + blake3-keystream encryption, mixed over the
//! mesh). v2 makes hiding them **verifiable** and **conditional**:
//!   • [`TimeLock`]      — here · there · nowhere · *not-yet*: a key released
//!                          only at a block height, or only if a heartbeat stops.
//!   • proof-of-storage — a host proves it still holds a shard to a verifier
//!                          that holds *only a Merkle root* (Filecoin-style PoSt).
//!   • XOR-PIR          — fetch a shard *without revealing which shard* — secret
//!                          even from the hosts (and the AI).
//!
//! All blake3 + serde (no heavy deps). The cryptographic-enforcement seams
//! (VDF time-lock, multi-server PIR privacy amplification) are noted inline.

use serde::{Deserialize, Serialize};
use crate::aether::Hash;

fn h(bytes: &[u8]) -> Hash {
    *blake3::hash(bytes).as_bytes()
}
fn h2(a: &[u8], b: &[u8]) -> Hash {
    let mut hr = blake3::Hasher::new();
    hr.update(a);
    hr.update(b);
    *hr.finalize().as_bytes()
}

// ─────────────────────────── 1. Time-lock / dead-man ───────────────────────────

/// Conditional key-release policy on a shard. `unlock_height` gates decryption
/// until a block height; `deadman_after_ms` gates it on a *missed* heartbeat
/// (the owner went away). Either or both may be active.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeLock {
    /// Decrypt only at/after this height (0 = no height gate).
    pub unlock_height: u64,
    /// Decrypt only if `now - last_heartbeat >= this` (0 = no dead-man gate).
    pub deadman_after_ms: u64,
}

impl TimeLock {
    /// A pure time-lock: opens at `height`.
    pub fn time_lock(unlock_height: u64) -> Self {
        Self { unlock_height, deadman_after_ms: 0 }
    }
    /// A pure dead-man switch: opens once the heartbeat lapses for `after_ms`.
    pub fn dead_man(after_ms: u64) -> Self {
        Self { unlock_height: 0, deadman_after_ms: after_ms }
    }
    /// Both gates must pass.
    pub fn unlocked(&self, height: u64, now_ms: u64, last_heartbeat_ms: u64) -> bool {
        let height_ok = height >= self.unlock_height;
        let deadman_ok = self.deadman_after_ms == 0
            || now_ms >= last_heartbeat_ms.saturating_add(self.deadman_after_ms);
        height_ok && deadman_ok
    }
    /// Release the shard key iff unlocked. Policy gate today; a VDF / time-lock
    /// puzzle makes it *cryptographically* un-openable early in AETHER-v2.5.
    pub fn release<'a>(&self, key: &'a [u8], height: u64, now_ms: u64, hb_ms: u64) -> Option<&'a [u8]> {
        if self.unlocked(height, now_ms, hb_ms) { Some(key) } else { None }
    }
}

// ─────────────────────────── 2. Proof-of-storage ───────────────────────────

/// Split a shard into fixed-size chunks (the Merkle leaves).
pub fn chunks(data: &[u8], chunk: usize) -> Vec<Vec<u8>> {
    if chunk == 0 || data.is_empty() {
        return vec![data.to_vec()];
    }
    data.chunks(chunk).map(|c| c.to_vec()).collect()
}

/// Pad hashed leaves up to a power of two (repeat the last) so the tree is
/// perfect — no odd-node edge cases in root or path.
fn pad_pow2(mut level: Vec<Hash>) -> Vec<Hash> {
    if level.is_empty() {
        return vec![[0u8; 32]];
    }
    let mut n = 1usize;
    while n < level.len() {
        n <<= 1;
    }
    let last = *level.last().unwrap();
    while level.len() < n {
        level.push(last);
    }
    level
}

/// The storage commitment a verifier keeps (it holds *only this*, not the data).
pub fn merkle_root(leaves: &[Vec<u8>]) -> Hash {
    let mut level = pad_pow2(leaves.iter().map(|l| h(l)).collect());
    while level.len() > 1 {
        level = level.chunks(2).map(|p| h2(&p[0], &p[1])).collect();
    }
    level[0]
}

/// Inclusion path for leaf `idx`: each step is (sibling, sibling_is_on_right).
pub fn merkle_path(leaves: &[Vec<u8>], idx: usize) -> Vec<(Hash, bool)> {
    let mut level = pad_pow2(leaves.iter().map(|l| h(l)).collect());
    let mut i = idx.min(level.len() - 1);
    let mut path = Vec::new();
    while level.len() > 1 {
        let (sib, sib_is_right) = if i % 2 == 0 { (level[i + 1], true) } else { (level[i - 1], false) };
        path.push((sib, sib_is_right));
        level = level.chunks(2).map(|p| h2(&p[0], &p[1])).collect();
        i /= 2;
    }
    path
}

/// Verify a host's response (the challenged chunk + its path) against the root.
/// True ⇒ the host demonstrably holds that chunk. The verifier never had the data.
pub fn verify_storage(root: Hash, chunk_bytes: &[u8], path: &[(Hash, bool)]) -> bool {
    let mut acc = h(chunk_bytes);
    for (sib, sib_is_right) in path {
        acc = if *sib_is_right { h2(&acc, sib) } else { h2(sib, &acc) };
    }
    acc == root
}

/// Deterministic challenge: which chunk index to ask for, from a random seed.
pub fn storage_challenge(seed: u64, n_chunks: usize) -> usize {
    if n_chunks == 0 { 0 } else { (h(&seed.to_le_bytes())[0] as usize) % n_chunks }
}

// ─────────────────────────── 3. XOR-PIR (oblivious retrieval) ───────────────────────────

/// 2-server information-theoretic PIR. To fetch item `idx` from an N-item DB,
/// the client sends server A a uniformly-random query and server B the same
/// query with bit `idx` flipped. Each server sees a uniform vector → learns
/// nothing about `idx`. Client XORs the two answers to recover the item.
pub fn pir_query(n: usize, idx: usize, seed: u64) -> (Vec<bool>, Vec<bool>) {
    let mut qa = vec![false; n];
    let mut s = seed | 1; // xorshift, never 0
    for b in qa.iter_mut() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        *b = (s & 1) == 1;
    }
    let mut qb = qa.clone();
    if idx < n {
        qb[idx] = !qb[idx];
    }
    (qa, qb)
}

/// A server's answer: XOR of every DB item its query bit selects.
pub fn pir_answer(db: &[Vec<u8>], q: &[bool]) -> Vec<u8> {
    let len = db.iter().map(|x| x.len()).max().unwrap_or(0);
    let mut acc = vec![0u8; len];
    for (item, &bit) in db.iter().zip(q) {
        if bit {
            for (a, b) in acc.iter_mut().zip(item) {
                *a ^= *b;
            }
        }
    }
    acc
}

/// Client recombines: `answer_a XOR answer_b == db[idx]`.
pub fn pir_reconstruct(answer_a: &[u8], answer_b: &[u8]) -> Vec<u8> {
    let n = answer_a.len().max(answer_b.len());
    (0..n)
        .map(|i| answer_a.get(i).copied().unwrap_or(0) ^ answer_b.get(i).copied().unwrap_or(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timelock_height_gate() {
        let tl = TimeLock::time_lock(100);
        assert!(!tl.unlocked(99, 0, 0));
        assert!(tl.unlocked(100, 0, 0));
        assert_eq!(tl.release(b"key", 99, 0, 0), None);
        assert_eq!(tl.release(b"key", 100, 0, 0), Some(&b"key"[..]));
    }

    #[test]
    fn deadman_heartbeat_gate() {
        let dm = TimeLock::dead_man(1000);
        // heartbeat at t=5000; alive while now < 6000, opens at/after 6000
        assert!(!dm.unlocked(0, 5500, 5000));
        assert!(dm.unlocked(0, 6000, 5000));
    }

    #[test]
    fn proof_of_storage_roundtrip() {
        let shard: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        let cs = chunks(&shard, 64);
        let root = merkle_root(&cs);
        let idx = storage_challenge(42, cs.len());
        let path = merkle_path(&cs, idx);
        // honest host holds the chunk → verifies
        assert!(verify_storage(root, &cs[idx], &path));
        // a host that lies about the chunk → fails
        assert!(!verify_storage(root, b"not the real chunk", &path));
    }

    #[test]
    fn xor_pir_retrieves_without_leaking_index() {
        let db: Vec<Vec<u8>> = (0..16u8).map(|i| vec![i, i.wrapping_mul(7), i ^ 0xAB]).collect();
        let idx = 11;
        let (qa, qb) = pir_query(db.len(), idx, 0xDEADBEEF);
        // the two queries differ in exactly one position — the secret index
        let diffs: usize = qa.iter().zip(&qb).filter(|(a, b)| a != b).count();
        assert_eq!(diffs, 1);
        let ans_a = pir_answer(&db, &qa);
        let ans_b = pir_answer(&db, &qb);
        assert_eq!(pir_reconstruct(&ans_a, &ans_b), db[idx]);
    }

    #[test]
    fn pir_any_index() {
        let db: Vec<Vec<u8>> = (0..32u32).map(|i| i.to_le_bytes().to_vec()).collect();
        for idx in [0usize, 1, 7, 16, 31] {
            let (qa, qb) = pir_query(db.len(), idx, 12345 + idx as u64);
            let got = pir_reconstruct(&pir_answer(&db, &qa), &pir_answer(&db, &qb));
            assert_eq!(got, db[idx], "PIR failed at idx {idx}");
        }
    }
}
