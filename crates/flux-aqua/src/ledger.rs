//! The bridge chain — "longest bridge wins", done so that it actually works.
//!
//! The source `void-walker::ledger::MultiverseChain` had three defects that
//! between them meant it was not a chain at all:
//!
//! 1. **No cryptographic linkage.** `Bridge::checksum()` hashed
//!    `(length, charge, parallel_sig, timestamp)` and the result was stored as
//!    `header_checksum`. `previous_hash` was assigned *afterwards* and never fed
//!    back into any hash. Editing a parent block therefore changed no child's
//!    checksum — the chain had zero tamper-evidence.
//! 2. **Re-sorting destroyed the links.** `push()` set
//!    `previous_hash = self.blocks.last()` and then sorted the whole vector by
//!    `bridge_length` descending. After the first sort `.last()` is the
//!    *shortest* bridge, not the previous block, so every subsequent parent
//!    pointer was wrong.
//! 3. **Heights collided.** `block_height = self.blocks.len()` combined with the
//!    sort meant heights ran in arbitrary order; `verify_chain()` contained a
//!    literal empty `if` block whose comment conceded heights "should be unique
//!    after sorting ... This is acceptable".
//!
//! # No floats in the hash
//!
//! The source stored `bridge_length: f64` and `k_parameter_snapshot: f64` in the
//! block and hashed them with `to_le_bytes()`. This crate did the same until a
//! test caught it: a block carrying `score = 0.38686833693985195` came back off
//! the wire as `0.3868683369398519`, because `serde_json`'s float parser is not
//! bit-exact for every value it can print. One ULP is enough to change the
//! BLAKE3 digest, so two peers holding *the same block* would compute different
//! hashes and disagree about the chain — a silent fork with no bad actor
//! anywhere.
//!
//! So there are no floats in [`BridgeBlock`]. Work and score are stored as
//! fixed-point integers in millionths, quantised once at construction by
//! [`quantize_micro`]. Integers survive JSON exactly, hash identically on every
//! platform, and make fork choice an integer comparison. The `f64` accessors are
//! presentation only.

use serde::{Deserialize, Serialize};

use crate::droplet::{IsotopicSig, TopoCharge};
use crate::species::Species;

pub type BlockHash = [u8; 32];

/// The all-zero hash, used as the genesis parent.
pub const NULL_HASH: BlockHash = [0u8; 32];

/// Fixed-point scale: one unit is 10⁻⁶.
pub const MICRO: f64 = 1_000_000.0;

/// A normalised score of 1.0, in fixed point.
pub const SCORE_ONE: u32 = 1_000_000;

/// Quantise a non-negative finite `f64` into millionths.
///
/// Returns `None` for NaN, infinity or a negative value — the three inputs that
/// must never reach a consensus structure.
pub fn quantize_micro(x: f64) -> Option<u64> {
    if !x.is_finite() || x < 0.0 {
        return None;
    }
    let scaled = (x * MICRO).round();
    if scaled > u64::MAX as f64 {
        return None;
    }
    Some(scaled as u64)
}

/// One recorded bridge crossing. Every field is exactly representable in JSON.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeBlock {
    /// Position in the chain. Always equal to the block's index.
    pub height: u64,
    /// Hash of the parent block. `NULL_HASH` for genesis.
    pub parent: BlockHash,
    /// Phase-space displacement in millionths — the block's work contribution.
    pub displacement_micro: u64,
    /// Topological charge at the time of crossing.
    pub charge: TopoCharge,
    /// Isotopic signature of the water bridged to.
    pub target_sig: IsotopicSig,
    /// Which species produced the block.
    pub species: Species,
    /// Producer's droplet id.
    pub producer: String,
    /// Normalised bridge score in millionths, `0..=`[`SCORE_ONE`].
    pub score_micro: u32,
    /// Milliseconds since the Unix epoch, supplied by the caller.
    ///
    /// The source called this field `attosecond_timestamp` while computing it as
    /// `as_nanos() / 1_000_000_000` — whole seconds, off by 10^18. Downstream,
    /// `chain_age_seconds` then multiplied by `1e-18`, so a chain running for a
    /// day reported an age of about a nanosecond. Milliseconds, honestly named.
    pub recorded_at_ms: u64,
}

impl BridgeBlock {
    /// Hash over **every** field including the parent — this is what makes the
    /// chain tamper-evident.
    pub fn hash(&self) -> BlockHash {
        let mut h = blake3::Hasher::new();
        h.update(b"flux-aqua/bridge-block/v1");
        h.update(&self.height.to_le_bytes());
        h.update(&self.parent);
        h.update(&self.displacement_micro.to_le_bytes());
        h.update(&self.charge.to_le_bytes());
        h.update(&self.target_sig);
        h.update(self.species.name().as_bytes());
        h.update(self.producer.as_bytes());
        h.update(&self.score_micro.to_le_bytes());
        h.update(&self.recorded_at_ms.to_le_bytes());
        *h.finalize().as_bytes()
    }

    /// Short hex id for logs.
    pub fn id(&self) -> String {
        hex::encode(&self.hash()[..8])
    }

    /// Displacement as a float. Presentation only — never hashed.
    pub fn displacement(&self) -> f64 {
        self.displacement_micro as f64 / MICRO
    }

    /// Score as a float in `[0, 1]`. Presentation only — never hashed.
    pub fn score(&self) -> f64 {
        self.score_micro as f64 / MICRO
    }
}

/// Why a block was rejected.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChainError {
    /// The block's height is not `chain.len()`.
    WrongHeight { expected: u64, got: u64 },
    /// The block's parent is not the current head's hash.
    WrongParent,
    /// Displacement or score was negative, non-finite, or out of range.
    BadWork,
    /// Timestamp went backwards relative to the head.
    NonMonotonicTime,
}

/// An append-only chain of bridge crossings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BridgeChain {
    blocks: Vec<BridgeBlock>,
    /// Cached Σ displacement in millionths — the chain's total work. Integer, so
    /// two peers summing the same blocks always agree.
    total_work_micro: u128,
}

impl Default for BridgeChain {
    fn default() -> Self {
        Self::new()
    }
}

impl BridgeChain {
    /// A chain containing only its genesis block.
    pub fn new() -> Self {
        let genesis = BridgeBlock {
            height: 0,
            parent: NULL_HASH,
            displacement_micro: 0,
            charge: 0,
            target_sig: [0u8; 32],
            species: Species::Scholar,
            producer: "genesis".to_string(),
            score_micro: 0,
            recorded_at_ms: 0,
        };
        Self {
            blocks: vec![genesis],
            total_work_micro: 0,
        }
    }

    /// The current head — the most recently appended block. Always present.
    pub fn head(&self) -> &BridgeBlock {
        self.blocks.last().expect("genesis is never removed")
    }

    pub fn height(&self) -> u64 {
        self.head().height
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    pub fn blocks(&self) -> &[BridgeBlock] {
        &self.blocks
    }

    /// Accumulated work in millionths. This is the quantity "longest bridge
    /// wins" is actually about, and the value fork choice compares.
    pub fn total_work_micro(&self) -> u128 {
        self.total_work_micro
    }

    /// Accumulated work as a float. Presentation only.
    pub fn total_work(&self) -> f64 {
        self.total_work_micro as f64 / MICRO
    }

    /// Build the next block on top of this chain's head, quantising the float
    /// inputs. Callers get a correctly-linked block without having to know the
    /// linkage rules — and cannot smuggle a NaN into consensus.
    #[allow(clippy::too_many_arguments)]
    pub fn next_block(
        &self,
        displacement: f64,
        charge: TopoCharge,
        target_sig: IsotopicSig,
        species: Species,
        producer: impl Into<String>,
        score: f64,
        recorded_at_ms: u64,
    ) -> Result<BridgeBlock, ChainError> {
        let displacement_micro = quantize_micro(displacement).ok_or(ChainError::BadWork)?;
        let score_micro = quantize_micro(score).ok_or(ChainError::BadWork)?;
        if score_micro > SCORE_ONE as u64 {
            return Err(ChainError::BadWork);
        }
        let head = self.head();
        Ok(BridgeBlock {
            height: head.height + 1,
            parent: head.hash(),
            displacement_micro,
            charge,
            target_sig,
            species,
            producer: producer.into(),
            score_micro: score_micro as u32,
            recorded_at_ms,
        })
    }

    /// Append a block, rejecting anything that would break the chain.
    pub fn push(&mut self, block: BridgeBlock) -> Result<(), ChainError> {
        let head = self.head();
        let expected = head.height + 1;
        if block.height != expected {
            return Err(ChainError::WrongHeight {
                expected,
                got: block.height,
            });
        }
        if block.parent != head.hash() {
            return Err(ChainError::WrongParent);
        }
        if block.score_micro > SCORE_ONE {
            return Err(ChainError::BadWork);
        }
        if block.recorded_at_ms < head.recorded_at_ms {
            return Err(ChainError::NonMonotonicTime);
        }
        self.total_work_micro += block.displacement_micro as u128;
        self.blocks.push(block);
        Ok(())
    }

    /// Walk the whole chain and confirm every link. Returns the height of the
    /// first bad block, if any.
    pub fn verify(&self) -> Result<(), u64> {
        for (i, b) in self.blocks.iter().enumerate() {
            if b.height != i as u64 {
                return Err(i as u64);
            }
            let expected_parent = if i == 0 {
                NULL_HASH
            } else {
                self.blocks[i - 1].hash()
            };
            if b.parent != expected_parent {
                return Err(i as u64);
            }
        }
        Ok(())
    }

    /// Fork choice. `true` when `other` should replace `self`: strictly more
    /// accumulated work, ties broken by the lower head hash so the rule is
    /// deterministic across nodes. The comparison is over integers, so it cannot
    /// differ between peers because of floating-point rounding.
    pub fn should_adopt(&self, other: &BridgeChain) -> bool {
        if other.verify().is_err() {
            return false;
        }
        match other.total_work_micro.cmp(&self.total_work_micro) {
            std::cmp::Ordering::Greater => true,
            std::cmp::Ordering::Less => false,
            std::cmp::Ordering::Equal => other.head().hash() < self.head().hash(),
        }
    }

    /// BLAKE3 digest over every block hash — a single value identifying the
    /// whole chain.
    pub fn digest(&self) -> String {
        let mut h = blake3::Hasher::new();
        for b in &self.blocks {
            h.update(&b.hash());
        }
        hex::encode(h.finalize().as_bytes())
    }

    /// Aggregate statistics.
    pub fn stats(&self) -> ChainStats {
        let n = self.blocks.len() as u64;
        let mut species: Vec<&'static str> =
            self.blocks.iter().map(|b| b.species.name()).collect();
        species.sort_unstable();
        species.dedup();

        let span_ms = self
            .head()
            .recorded_at_ms
            .saturating_sub(self.blocks[0].recorded_at_ms);

        let non_genesis = n.saturating_sub(1);
        let score_sum: u64 = self.blocks.iter().skip(1).map(|b| b.score_micro as u64).sum();

        ChainStats {
            block_count: n,
            species_count: species.len() as u32,
            total_work: self.total_work(),
            mean_displacement: if non_genesis > 0 {
                self.total_work() / non_genesis as f64
            } else {
                0.0
            },
            mean_score: if non_genesis > 0 {
                score_sum as f64 / non_genesis as f64 / MICRO
            } else {
                0.0
            },
            span_ms,
        }
    }
}

/// Aggregate chain statistics. Floats here are presentation only — nothing in
/// consensus reads them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChainStats {
    pub block_count: u64,
    pub species_count: u32,
    pub total_work: f64,
    pub mean_displacement: f64,
    pub mean_score: f64,
    /// Wall-clock span of the chain in milliseconds.
    pub span_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extend(chain: &mut BridgeChain, n: usize) {
        for i in 0..n {
            let b = chain
                .next_block(
                    0.1 * (i + 1) as f64,
                    (i % 8) as i32,
                    [i as u8; 32],
                    Species::Scout,
                    format!("droplet-{i}"),
                    0.5,
                    1_000 + i as u64,
                )
                .unwrap();
            chain.push(b).unwrap();
        }
    }

    #[test]
    fn genesis_is_self_consistent() {
        let c = BridgeChain::new();
        assert_eq!(c.height(), 0);
        assert_eq!(c.head().parent, NULL_HASH);
        assert!(c.verify().is_ok());
        assert_eq!(c.total_work_micro(), 0);
    }

    #[test]
    fn appending_keeps_heights_monotone_and_links_intact() {
        let mut c = BridgeChain::new();
        extend(&mut c, 20);
        assert_eq!(c.height(), 20);
        assert_eq!(c.len(), 21);
        assert!(c.verify().is_ok());
        for (i, b) in c.blocks().iter().enumerate() {
            assert_eq!(b.height, i as u64);
        }
    }

    #[test]
    fn tampering_with_a_parent_breaks_every_child() {
        // The property the source chain did NOT have: its checksum excluded
        // previous_hash, so this edit was invisible.
        let mut c = BridgeChain::new();
        extend(&mut c, 5);
        assert!(c.verify().is_ok());
        c.blocks[2].displacement_micro = 99_000_000;
        assert_eq!(c.verify(), Err(3));
    }

    #[test]
    fn a_block_survives_a_json_roundtrip_bit_for_bit() {
        // The regression this whole fixed-point design exists for: with f64
        // fields, `score = 0.38686833693985195` came back as
        // `0.3868683369398519` and the hash changed.
        let mut c = BridgeChain::new();
        extend(&mut c, 3);
        for b in c.blocks() {
            let json = serde_json::to_string(b).unwrap();
            let back: BridgeBlock = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, b);
            assert_eq!(back.hash(), b.hash());
        }
    }

    #[test]
    fn quantization_rejects_what_must_never_reach_consensus() {
        assert_eq!(quantize_micro(f64::NAN), None);
        assert_eq!(quantize_micro(f64::INFINITY), None);
        assert_eq!(quantize_micro(-1.0), None);
        assert_eq!(quantize_micro(0.0), Some(0));
        assert_eq!(quantize_micro(1.5), Some(1_500_000));
    }

    #[test]
    fn wrong_parent_is_rejected() {
        let mut c = BridgeChain::new();
        extend(&mut c, 3);
        let mut bad = c
            .next_block(0.5, 1, [0; 32], Species::Trader, "x", 0.5, 9_000)
            .unwrap();
        bad.parent = [9u8; 32];
        assert_eq!(c.push(bad), Err(ChainError::WrongParent));
        assert_eq!(c.height(), 3, "a rejected block must not be appended");
    }

    #[test]
    fn wrong_height_is_rejected() {
        let mut c = BridgeChain::new();
        let mut bad = c
            .next_block(0.5, 1, [0; 32], Species::Trader, "x", 0.5, 10)
            .unwrap();
        bad.height = 7;
        assert_eq!(
            c.push(bad),
            Err(ChainError::WrongHeight {
                expected: 1,
                got: 7
            })
        );
    }

    #[test]
    fn nonfinite_negative_and_oversized_work_never_becomes_a_block() {
        let c = BridgeChain::new();
        for bad in [f64::NAN, f64::INFINITY, -1.0] {
            assert_eq!(
                c.next_block(bad, 1, [0; 32], Species::Trader, "x", 0.5, 10)
                    .unwrap_err(),
                ChainError::BadWork
            );
        }
        // A score above 1.0 is out of the normalised range.
        assert_eq!(
            c.next_block(0.5, 1, [0; 32], Species::Trader, "x", 1.5, 10)
                .unwrap_err(),
            ChainError::BadWork
        );
    }

    #[test]
    fn time_must_not_run_backwards() {
        let mut c = BridgeChain::new();
        let a = c
            .next_block(0.1, 1, [0; 32], Species::Trader, "x", 0.5, 5_000)
            .unwrap();
        c.push(a).unwrap();
        let b = c
            .next_block(0.1, 1, [0; 32], Species::Trader, "x", 0.5, 4_000)
            .unwrap();
        assert_eq!(c.push(b), Err(ChainError::NonMonotonicTime));
    }

    #[test]
    fn total_work_is_the_exact_integer_sum() {
        let mut c = BridgeChain::new();
        extend(&mut c, 4);
        // 0.1 + 0.2 + 0.3 + 0.4, with no floating-point drift at all.
        assert_eq!(c.total_work_micro(), 1_000_000);
    }

    #[test]
    fn fork_choice_prefers_more_accumulated_work() {
        let mut short = BridgeChain::new();
        extend(&mut short, 3);
        let mut long = BridgeChain::new();
        extend(&mut long, 10);

        assert!(short.should_adopt(&long));
        assert!(!long.should_adopt(&short));
    }

    #[test]
    fn fork_choice_refuses_an_invalid_challenger() {
        let mut ours = BridgeChain::new();
        extend(&mut ours, 2);
        let mut theirs = BridgeChain::new();
        extend(&mut theirs, 10);
        theirs.blocks[4].charge = 123; // break the linkage
        assert!(!ours.should_adopt(&theirs), "invalid chain must never win");
    }

    #[test]
    fn fork_choice_ties_break_deterministically() {
        let mut a = BridgeChain::new();
        extend(&mut a, 3);
        let mut b = BridgeChain::new();
        extend(&mut b, 3);
        // Identical construction → identical hashes → neither adopts the other.
        assert_eq!(a.digest(), b.digest());
        assert!(!a.should_adopt(&b));
        assert!(!b.should_adopt(&a));
    }

    #[test]
    fn digest_changes_when_the_chain_changes() {
        let mut c = BridgeChain::new();
        let d0 = c.digest();
        extend(&mut c, 1);
        assert_ne!(d0, c.digest());
    }

    #[test]
    fn stats_are_computed_over_non_genesis_blocks() {
        let mut c = BridgeChain::new();
        extend(&mut c, 4);
        let s = c.stats();
        assert_eq!(s.block_count, 5);
        assert!((s.mean_score - 0.5).abs() < 1e-12);
        assert!(s.mean_displacement > 0.0);
        // Genesis is at t=0 and the last block at t=1003.
        assert_eq!(s.span_ms, 1_003);
    }
}
