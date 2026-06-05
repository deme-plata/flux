//! Transaction knot diagrams + invariants.
//!
//! Lifted from `QTFT/blockchain/knot.rs::TransactionKnot,KnotInvariants,KnotSecurityProof`
//! (2026-05-29). The upstream binds directly to `QuantumTransaction`. This
//! port decouples via the [`TransactionLike`] trait so any Flux/SIGIL
//! transaction type can be analysed.

use num_complex::Complex64;
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

use super::crossing::{Crossing, CrossingType, KnotStrand};
use super::jones::JonesPolynomial;

// ─── Trait-based decoupling ─────────────────────────────────────────────────

/// Adapter trait: any type that can enumerate inputs + outputs becomes
/// knot-able. Stays generic — SIGIL transactions implement this with their
/// own input/output structs; tests use small synthetic structs.
pub trait TransactionLike {
    /// Input handle.
    type Input: TransactionInputLike;
    /// Output handle.
    type Output: TransactionOutputLike;
    /// Iterate inputs.
    fn inputs(&self) -> &[Self::Input];
    /// Iterate outputs.
    fn outputs(&self) -> &[Self::Output];
}

/// What the knot constructor needs from a transaction input.
pub trait TransactionInputLike {
    /// 32-byte previous-tx identifier — used to seed strand position.
    fn prev_txid(&self) -> &[u8; 32];
}

/// What the knot constructor needs from a transaction output.
pub trait TransactionOutputLike {
    /// 32-byte recipient pubkey hash — seeds strand position.
    fn pubkey_hash(&self) -> &[u8; 32];
    /// Monetary amount — becomes the strand's `weight`.
    /// Returned as `u128` to be SIGIL-precision-agnostic; downcast to f64
    /// for the knot-diagram math.
    fn amount(&self) -> u128;
}

// ─── TransactionKnot ────────────────────────────────────────────────────────

/// A knot diagram derived from one transaction. Inputs are under-crossings;
/// outputs are over-crossings; one crossing per (input, output) pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionKnot {
    /// All strands (inputs first, then outputs).
    pub strands: Vec<KnotStrand>,
    /// All crossings.
    pub crossings: Vec<Crossing>,
    /// Number of components — 1 for a knot, >1 for a multi-component link.
    pub components: usize,
    /// Ambient space dimension (3 in this implementation).
    pub dimension: usize,
}

impl TransactionKnot {
    /// Build a diagram from any [`TransactionLike`] value.
    pub fn from_transaction<T: TransactionLike>(tx: &T) -> Self {
        let inputs = tx.inputs();
        let outputs = tx.outputs();
        let input_count = inputs.len();

        let mut strands: Vec<KnotStrand> = Vec::with_capacity(input_count + outputs.len());
        let mut crossings: Vec<Crossing> = Vec::with_capacity(input_count * outputs.len());

        // Input strands — under-crossings, seeded by prev_txid hash.
        for (i, input) in inputs.iter().enumerate() {
            let h = input.prev_txid();
            let hash_val = u32::from_le_bytes([h[0], h[1], h[2], h[3]]) as f64
                / u32::MAX as f64;
            strands.push(KnotStrand {
                id: i,
                start: [hash_val, 0.0, 0.0],
                end: [hash_val, 1.0, 0.0],
                is_input: true,
                weight: 1.0,
            });
        }

        // Output strands — over-crossings, seeded by pubkey hash, weighted by amount.
        for (j, output) in outputs.iter().enumerate() {
            let h = output.pubkey_hash();
            let hash_val = u32::from_le_bytes([h[0], h[1], h[2], h[3]]) as f64
                / u32::MAX as f64;
            strands.push(KnotStrand {
                id: input_count + j,
                start: [hash_val, 0.0, 0.5],
                end: [hash_val, 1.0, 0.5],
                is_input: false,
                weight: output.amount() as f64,
            });
        }

        // One crossing per (input, output) pair.
        for i in 0..input_count {
            for j in 0..outputs.len() {
                let input_x = strands[i].start[0];
                let output_x = strands[input_count + j].start[0];
                let crossing_type = if input_x < output_x {
                    CrossingType::Positive
                } else {
                    CrossingType::Negative
                };
                crossings.push(Crossing {
                    over_strand: input_count + j,
                    under_strand: i,
                    crossing_type,
                    weight: outputs[j].amount() as f64,
                });
            }
        }

        let components = if input_count > 1 || outputs.len() > 1 {
            input_count.min(outputs.len()).max(1)
        } else {
            1
        };

        Self {
            strands,
            crossings,
            components,
            dimension: 3,
        }
    }

    /// True if this diagram is (or simplifies to) the unknot.
    pub fn is_unknot(&self) -> bool {
        self.crossings.is_empty()
            || (self.components == 1 && self.can_simplify_to_unknot())
    }

    /// Cancellation heuristic — equal positive/negative crossings in a
    /// single-component diagram likely cancels to the unknot. Sufficient
    /// for the SIGIL-side fork-detection use case (real R1 / R2 moves
    /// are not enumerated).
    fn can_simplify_to_unknot(&self) -> bool {
        let pos = self
            .crossings
            .iter()
            .filter(|c| c.crossing_type == CrossingType::Positive)
            .count() as i32;
        let neg = self
            .crossings
            .iter()
            .filter(|c| c.crossing_type == CrossingType::Negative)
            .count() as i32;
        pos == neg && self.components == 1
    }

    /// Writhe — signed sum of all crossing signs.
    pub fn writhe(&self) -> i32 {
        self.crossings
            .iter()
            .map(|c| c.crossing_type.sign() as i32)
            .sum()
    }

    /// Total crossing count.
    pub fn crossing_number(&self) -> usize {
        self.crossings.len()
    }

    /// Linking number for multi-component links.
    /// Zero if the diagram has a single component.
    pub fn linking_number(&self) -> i32 {
        if self.components <= 1 {
            return 0;
        }
        self.crossings
            .iter()
            .filter(|c| {
                let over_is_input = self.strands[c.over_strand].is_input;
                let under_is_input = self.strands[c.under_strand].is_input;
                over_is_input != under_is_input
            })
            .map(|c| c.crossing_type.sign() as i32)
            .sum::<i32>()
            / 2
    }
}

// ─── KnotInvariants ─────────────────────────────────────────────────────────

/// Bundle of computed invariants for one knot diagram. Used as the unit of
/// fork-detection comparison and as the basis for the security bound.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnotInvariants {
    /// Jones polynomial.
    pub jones: JonesPolynomial,
    /// Jones evaluated at t = -1.
    pub jones_at_minus_one: f64,
    /// Alexander polynomial Δ(1) — always 1 for knots; placeholder for now.
    pub alexander_at_one: f64,
    /// Kauffman bracket evaluation.
    pub kauffman_bracket: f64,
    /// Writhe.
    pub writhe: i32,
    /// Crossing number.
    pub crossing_number: usize,
    /// Linking number (multi-component links only).
    pub linking_number: i32,
    /// Component count.
    pub components: usize,
    /// κ-corrected Jones at `t = exp(2πiκ)`, multiplied by the framing phase.
    pub quantum_jones: Complex64,
}

impl KnotInvariants {
    /// Compute invariants for a [`TransactionKnot`], given the QTFT κ-coupling.
    pub fn from_knot(knot: &TransactionKnot, kappa: f64) -> Self {
        let jones = if knot.is_unknot() {
            JonesPolynomial::unknot()
        } else if knot.components > 1 {
            // Coinjoin / multi-output → Hopf-link approximation.
            JonesPolynomial::hopf_link()
        } else if knot.crossing_number() == 3 {
            JonesPolynomial::trefoil()
        } else {
            Self::compute_jones_from_crossings(knot)
        };

        let jones_at_minus_one = jones.evaluate(-1.0);
        let alexander_at_one = 1.0;
        let kauffman_bracket = jones_at_minus_one * (-1.0_f64).powi(knot.writhe());

        let t_quantum = Complex64::new(0.0, 2.0 * PI * kappa).exp();
        let quantum_jones = jones.evaluate_complex(t_quantum)
            * Complex64::new(0.0, kappa * knot.writhe() as f64).exp();

        Self {
            jones,
            jones_at_minus_one,
            alexander_at_one,
            kauffman_bracket,
            writhe: knot.writhe(),
            crossing_number: knot.crossing_number(),
            linking_number: knot.linking_number(),
            components: knot.components,
            quantum_jones,
        }
    }

    /// Fallback Jones-from-skein heuristic. Returns an approximate sparse
    /// polynomial bounded by writhe + crossing-number difference. Matches
    /// upstream behaviour — full skein recursion is too expensive for
    /// hot-path fork detection.
    fn compute_jones_from_crossings(knot: &TransactionKnot) -> JonesPolynomial {
        let n = knot.crossing_number() as i32;
        let w = knot.writhe();
        let mut coeffs = vec![1.0];
        let mut powers = vec![w];
        if n > w.abs() {
            coeffs.push(((n - w.abs()) / 2) as f64);
            powers.push(w + 1);
        }
        JonesPolynomial::new(coeffs, powers)
    }

    /// Compute directly from a transaction.
    pub fn from_transaction<T: TransactionLike>(tx: &T, kappa: f64) -> Self {
        let knot = TransactionKnot::from_transaction(tx);
        Self::from_knot(&knot, kappa)
    }
}

// ─── KnotSecurityProof ──────────────────────────────────────────────────────

/// Probabilistic security bound from knot invariants — the QTFT paper's
/// `2^(-128) · 2^(-8c)` combined SHA + knot collision bound.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnotSecurityProof {
    /// Invariants underpinning the proof.
    pub original_invariants: KnotInvariants,
    /// Collision probability bound.
    pub collision_probability: f64,
    /// Effective security in bits — `-log₂(collision_probability)`.
    pub security_bits: f64,
    /// Human-readable explanation.
    pub explanation: String,
}

impl KnotSecurityProof {
    /// Generate a security proof for any [`TransactionLike`].
    pub fn for_transaction<T: TransactionLike>(tx: &T, kappa: f64) -> Self {
        let invariants = KnotInvariants::from_transaction(tx, kappa);
        let sha256_bound = 2.0_f64.powi(-128);
        let knot_bound = 2.0_f64.powi(-(invariants.crossing_number as i32 * 8));
        let collision_probability = sha256_bound * knot_bound;
        let security_bits = -collision_probability.log2();

        let explanation = format!(
            "Transaction security from knot theory:\n\
             - Crossing number: {} (provides {} bits)\n\
             - Writhe: {} (provides sign uniqueness)\n\
             - Jones polynomial: evaluates to {:.6} at t=-1\n\
             - Combined with SHA256: {:.0} bits of security\n\
             - Modification requires solving knot isomorphism (NP-hard)",
            invariants.crossing_number,
            invariants.crossing_number * 8,
            invariants.writhe,
            invariants.jones_at_minus_one,
            security_bits
        );

        Self {
            original_invariants: invariants,
            collision_probability,
            security_bits,
            explanation,
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // A tiny in-test transaction type satisfying TransactionLike.
    struct TestInput { prev: [u8; 32] }
    struct TestOutput { pk: [u8; 32], amt: u128 }
    struct TestTx { ins: Vec<TestInput>, outs: Vec<TestOutput> }

    impl TransactionInputLike for TestInput {
        fn prev_txid(&self) -> &[u8; 32] { &self.prev }
    }
    impl TransactionOutputLike for TestOutput {
        fn pubkey_hash(&self) -> &[u8; 32] { &self.pk }
        fn amount(&self) -> u128 { self.amt }
    }
    impl TransactionLike for TestTx {
        type Input = TestInput;
        type Output = TestOutput;
        fn inputs(&self) -> &[Self::Input] { &self.ins }
        fn outputs(&self) -> &[Self::Output] { &self.outs }
    }

    fn tx_1in_1out() -> TestTx {
        TestTx {
            ins:  vec![TestInput  { prev: [1u8; 32] }],
            outs: vec![TestOutput { pk: [2u8; 32], amt: 50_000 }],
        }
    }

    fn tx_coinjoin_2x2() -> TestTx {
        TestTx {
            ins: vec![
                TestInput { prev: [1u8; 32] },
                TestInput { prev: [2u8; 32] },
            ],
            outs: vec![
                TestOutput { pk: [3u8; 32], amt: 25_000 },
                TestOutput { pk: [4u8; 32], amt: 25_000 },
            ],
        }
    }

    #[test]
    fn from_transaction_builds_two_strands_one_crossing() {
        let knot = TransactionKnot::from_transaction(&tx_1in_1out());
        assert_eq!(knot.strands.len(), 2);
        assert_eq!(knot.crossings.len(), 1);
        assert_eq!(knot.dimension, 3);
    }

    #[test]
    fn coinjoin_detected_as_multi_component() {
        let knot = TransactionKnot::from_transaction(&tx_coinjoin_2x2());
        // 2 inputs × 2 outputs = 4 strands, 4 crossings.
        assert_eq!(knot.strands.len(), 4);
        assert_eq!(knot.crossings.len(), 4);
        // Either components > 1 OR a non-zero linking number tells us it's a link.
        assert!(knot.components > 1 || knot.linking_number() != 0);
    }

    #[test]
    fn invariants_round_trip_through_json() {
        let knot = TransactionKnot::from_transaction(&tx_1in_1out());
        let inv = KnotInvariants::from_knot(&knot, 0.1);
        let j = serde_json::to_string(&inv).unwrap();
        let back: KnotInvariants = serde_json::from_str(&j).unwrap();
        assert_eq!(back.crossing_number, inv.crossing_number);
        assert_eq!(back.writhe, inv.writhe);
        assert_eq!(back.components, inv.components);
    }

    #[test]
    fn security_proof_reports_positive_bits() {
        let proof = KnotSecurityProof::for_transaction(&tx_1in_1out(), 0.1);
        // 128 (SHA) + 8×crossings ⇒ at least 128 bits.
        assert!(proof.security_bits >= 128.0);
        assert!(proof.collision_probability < 1.0);
    }

    #[test]
    fn writhe_is_sum_of_crossing_signs() {
        let knot = TransactionKnot::from_transaction(&tx_coinjoin_2x2());
        let manual: i32 = knot
            .crossings
            .iter()
            .map(|c| c.crossing_type.sign() as i32)
            .sum();
        assert_eq!(knot.writhe(), manual);
    }
}
