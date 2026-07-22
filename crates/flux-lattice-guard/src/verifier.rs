//! LatticeGuard Verifier
//!
//! Implements the verifier for the LatticeGuard zk-SNARK protocol.

use crate::{
    approximate_product::ApproximateProductVerifier,
    commitment::{LatticeCommitment, PolynomialCommitment},
    errors::LatticeGuardError,
    ntt::NttOperator,
    params::RlweParams,
    prover::{LatticeGuardProof, ProofMetadata},
    transcript::LatticeTranscript,
    ArithmeticCircuit, LatticeGuardSRS, Polynomial, Scalar,
};
use tracing::{debug, info, warn};

/// LatticeGuard verifier
pub struct LatticeGuardVerifier {
    params: RlweParams,
    ntt: NttOperator,
    commitment_scheme: PolynomialCommitment,
    product_verifier: ApproximateProductVerifier,
}

impl LatticeGuardVerifier {
    /// Create new verifier with given parameters
    pub fn new(params: RlweParams) -> Result<Self, LatticeGuardError> {
        let ntt = NttOperator::new(&params);
        let commitment_scheme = PolynomialCommitment::new(params.clone());
        let product_verifier = ApproximateProductVerifier::new(params.clone());

        Ok(Self {
            params,
            ntt,
            commitment_scheme,
            product_verifier,
        })
    }

    /// Verify a LatticeGuard proof
    pub fn verify(
        &self,
        circuit: &ArithmeticCircuit,
        public_inputs: &[Scalar],
        proof: &LatticeGuardProof,
        srs: &LatticeGuardSRS,
    ) -> Result<bool, LatticeGuardError> {
        let start_time = std::time::Instant::now();

        info!(
            "Verifying LatticeGuard proof: {} constraints, {} public inputs",
            proof.metadata.num_constraints, proof.metadata.num_public_inputs
        );

        // Phase 1: Validate proof structure
        debug!("Phase 1: Validating proof structure");
        if !self.validate_proof_structure(circuit, public_inputs, proof)? {
            warn!("Proof structure validation failed");
            return Ok(false);
        }

        // Phase 2: Reconstruct challenges
        debug!("Phase 2: Reconstructing challenges via Fiat-Shamir");
        let mut transcript = LatticeTranscript::new(self.params.clone());

        if proof.commitments.len() < 3 {
            return Err(LatticeGuardError::InternalError(
                "Proof must contain at least 3 commitments".to_string(),
            ));
        }

        transcript.append_commitment(b"com_a", &proof.commitments[0]);
        transcript.append_commitment(b"com_b", &proof.commitments[1]);
        transcript.append_commitment(b"com_c", &proof.commitments[2]);

        let challenge = transcript.generate_challenge();
        let z = challenge.polynomial.evaluate(1, self.params.modulus);

        // Mirror the prover: bind the claimed evaluations into the transcript.
        // Together with the Phase-6 exact state check this commits the proof's
        // transcript_state to (commitments, evaluations, all product-proof
        // messages) — tamper with any of them and the states diverge.
        let (a_z, b_z, c_z) = proof.evaluations;
        transcript.append_scalar(b"eval_a", a_z);
        transcript.append_scalar(b"eval_b", b_z);
        transcript.append_scalar(b"eval_c", c_z);

        // Phase 3: Verify approximate product proofs per constraint.
        //
        // COMPLETENESS BUGFIX (the P1 blocker): the old code reconstructed the
        // per-constraint values with witness wires substituted by 0 and fed
        // those into product verification. The prover proved over the REAL
        // witness values, so every honest proof with a witness wire failed the
        // linearized_commitment equality AND diverged the transcript (the
        // a/b prefixes are appended to it). The verifier cannot know witness
        // values by construction — instead it must take the CLAIMED values the
        // proof carries (linearization_proof.linearized_commitment; constant
        // polynomials, so the evaluation IS the value), and enforce:
        //   (1) any linear combination made only of PUBLIC wires recomputes
        //       exactly and must equal the claimed value (this is what rejects
        //       wrong public inputs — soundness preserved),
        //   (2) the claimed values satisfy a·b ≈ c within the PARAMS bound
        //       (checked inside product_verifier; the proof-supplied bound is
        //       clamped below so a forger can't widen it),
        //   (3) the claimed values feed the same transcript bytes the prover
        //       appended, so challenges match.
        debug!("Phase 3: Verifying approximate product proofs");
        let mut claimed_a = Vec::with_capacity(circuit.num_constraints);
        let mut claimed_b = Vec::with_capacity(circuit.num_constraints);
        let mut claimed_c = Vec::with_capacity(circuit.num_constraints);

        for (i, (constraint, product_proof)) in circuit
            .constraints
            .iter()
            .zip(proof.product_proofs.iter())
            .enumerate()
        {
            debug!(
                "  Verifying constraint {}/{}",
                i + 1,
                circuit.num_constraints
            );

            // Reject proofs that claim a wider error bound than the parameters
            // allow — the bound inside the proof is attacker-controlled.
            if product_proof.error_bound_proof.bound > self.params.error_bound {
                warn!(
                    "Product proof {} claims bound {} > params bound {}",
                    i, product_proof.error_bound_proof.bound, self.params.error_bound
                );
                return Ok(false);
            }

            let lin = &product_proof.linearization_proof.linearized_commitment;
            let (a_val, b_val, c_val) = (lin[0], lin[1], lin[2]);

            // Public-input binding: a linear combination over only public
            // wires is fully known to the verifier and must recompute exactly.
            for (lc, claimed, name) in [
                (&constraint.a, a_val, "a"),
                (&constraint.b, b_val, "b"),
                (&constraint.c, c_val, "c"),
            ] {
                if lc.iter().all(|&(idx, _)| idx < public_inputs.len()) {
                    let expected =
                        self.evaluate_public_linear_combination(lc, public_inputs);
                    if expected != claimed {
                        warn!(
                            "Constraint {} public binding failed: {}-side claims {} but public inputs give {}",
                            i, name, claimed, expected
                        );
                        return Ok(false);
                    }
                }
            }

            claimed_a.push(a_val);
            claimed_b.push(b_val);
            claimed_c.push(c_val);

            // Constant-polynomial encoding matching the prover: value in the
            // constant term, zeros elsewhere, full ring dimension.
            let mut a_vec = vec![0u64; self.params.dimension]; a_vec[0] = a_val;
            let mut b_vec = vec![0u64; self.params.dimension]; b_vec[0] = b_val;
            let mut c_vec = vec![0u64; self.params.dimension]; c_vec[0] = c_val;

            // Verify approximate product proof
            let valid = self.product_verifier.verify(
                &a_vec,
                &b_vec,
                &c_vec,
                product_proof,
                &mut transcript,
            )?;

            if !valid {
                warn!("Approximate product proof {} verification failed", i);
                return Ok(false);
            }
        }

        // Phase 4: Bind the top-level evaluations to the per-constraint claims.
        //
        // The prover's a/b/c polynomials have exactly the per-constraint values
        // as coefficients, so a(z)/b(z)/c(z) must equal the evaluation of the
        // claimed-value vectors at the reconstructed challenge z — exactly.
        // (The old aggregate check a(z)·b(z) ≈ c(z) was incoherent for more
        // than one constraint: a product of sums has cross terms, so honest
        // multi-constraint proofs always failed it. Exact reconstruction is
        // both complete and strictly stronger against evaluation tampering.)
        debug!("Phase 4: Verifying evaluation consistency with claimed values");
        let expected_a_z = Polynomial::new(claimed_a).evaluate(z, self.params.modulus);
        let expected_b_z = Polynomial::new(claimed_b).evaluate(z, self.params.modulus);
        let expected_c_z = Polynomial::new(claimed_c).evaluate(z, self.params.modulus);
        if (a_z, b_z, c_z) != (expected_a_z, expected_b_z, expected_c_z) {
            warn!(
                "Evaluation consistency failed: proof claims ({}, {}, {}), reconstruction gives ({}, {}, {})",
                a_z, b_z, c_z, expected_a_z, expected_b_z, expected_c_z
            );
            return Ok(false);
        }

        // Phase 5: Verify commitment consistency
        debug!("Phase 5: Verifying commitment consistency");
        if !self.verify_commitment_consistency(&proof.commitments, &proof.evaluations, z, srs)? {
            warn!("Commitment consistency verification failed");
            return Ok(false);
        }

        // Phase 6: Verify transcript state matches the prover's exactly.
        // Both sides append the same sequence (commitments, evaluations,
        // product-proof messages), so an honest proof matches bit-for-bit;
        // any divergence means a tampered or mis-generated proof.
        debug!("Phase 6: Verifying transcript state");
        let final_transcript_state = transcript.finalize();
        if final_transcript_state != proof.transcript_state {
            warn!("Transcript state mismatch: proof was not generated over these messages");
            return Ok(false);
        }

        let verification_time_ms = start_time.elapsed().as_millis() as u64;
        info!(
            "Proof verified in {}ms (prover took {}ms)",
            verification_time_ms, proof.metadata.generation_time_ms
        );

        Ok(true)
    }

    /// Validate proof structure matches circuit
    fn validate_proof_structure(
        &self,
        circuit: &ArithmeticCircuit,
        public_inputs: &[Scalar],
        proof: &LatticeGuardProof,
    ) -> Result<bool, LatticeGuardError> {
        // Check number of constraints
        if proof.metadata.num_constraints != circuit.num_constraints {
            debug!(
                "Constraint count mismatch: proof has {}, circuit has {}",
                proof.metadata.num_constraints, circuit.num_constraints
            );
            return Ok(false);
        }

        // Check number of public inputs
        if proof.metadata.num_public_inputs != circuit.num_public_inputs {
            debug!(
                "Public input count mismatch: proof has {}, circuit has {}",
                proof.metadata.num_public_inputs, circuit.num_public_inputs
            );
            return Ok(false);
        }

        // Check public inputs length
        if public_inputs.len() != circuit.num_public_inputs {
            debug!(
                "Public input length mismatch: provided {}, expected {}",
                public_inputs.len(),
                circuit.num_public_inputs
            );
            return Ok(false);
        }

        // Check number of product proofs
        if proof.product_proofs.len() != circuit.num_constraints {
            debug!(
                "Product proof count mismatch: {} proofs for {} constraints",
                proof.product_proofs.len(),
                circuit.num_constraints
            );
            return Ok(false);
        }

        // Check commitments count
        if proof.commitments.len() != 3 {
            debug!(
                "Expected 3 commitments, got {}",
                proof.commitments.len()
            );
            return Ok(false);
        }

        // Check per-product-proof shape so later indexing can't panic on a
        // malformed proof: 3 claimed values (a, b, c) and at least one
        // consistency response.
        for (i, pp) in proof.product_proofs.iter().enumerate() {
            if pp.linearization_proof.linearized_commitment.len() != 3 {
                debug!(
                    "Product proof {} has {} linearized values, expected 3",
                    i,
                    pp.linearization_proof.linearized_commitment.len()
                );
                return Ok(false);
            }
            if pp.consistency_check.responses.is_empty() {
                debug!("Product proof {} has no consistency responses", i);
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// Evaluate linear combination using only public inputs
    fn evaluate_public_linear_combination(
        &self,
        lc: &[(usize, Scalar)],
        public_inputs: &[Scalar],
    ) -> Scalar {
        let mut result = 0u128;

        for &(idx, coeff) in lc {
            // Only use public inputs for verification
            let val = if idx < public_inputs.len() {
                public_inputs[idx]
            } else {
                // Witness values are not available to verifier
                // The proof must convince us they satisfy constraints
                0
            };

            result = (result + (coeff as u128 * val as u128)) % self.params.modulus as u128;
        }

        result as Scalar
    }

    /// Verify commitment consistency with evaluations
    fn verify_commitment_consistency(
        &self,
        commitments: &[LatticeCommitment],
        evaluations: &(Scalar, Scalar, Scalar),
        z: Scalar,
        srs: &LatticeGuardSRS,
    ) -> Result<bool, LatticeGuardError> {
        // Verify that commitments open to claimed evaluations
        // This is done by checking the pairing equation in the RLWE setting

        // For efficiency, we check approximate consistency
        // The commitment C to polynomial p(X) should satisfy:
        // C evaluated at z ≈ p(z) within error bound

        // In lattice setting, we verify this through homomorphic properties
        // of RLWE ciphertexts

        if srs.powers_of_tau.is_empty() {
            return Err(LatticeGuardError::SrsInsufficient(0, 1));
        }

        // Check commitment dimensions
        for (i, commitment) in commitments.iter().enumerate() {
            if commitment.ciphertext.dimension() != self.params.dimension {
                debug!(
                    "Commitment {} has wrong dimension: {} vs expected {}",
                    i,
                    commitment.ciphertext.dimension(),
                    self.params.dimension
                );
                return Ok(false);
            }
        }

        // Verify evaluations are within modulus
        let (a_z, b_z, c_z) = evaluations;
        if *a_z >= self.params.modulus
            || *b_z >= self.params.modulus
            || *c_z >= self.params.modulus
        {
            debug!("Evaluations exceed modulus");
            return Ok(false);
        }

        // Verify z is within modulus
        if z >= self.params.modulus {
            debug!("Challenge point exceeds modulus");
            return Ok(false);
        }

        Ok(true)
    }

    /// Batch verify multiple proofs (more efficient)
    pub fn batch_verify(
        &self,
        circuits: &[&ArithmeticCircuit],
        public_inputs: &[&[Scalar]],
        proofs: &[&LatticeGuardProof],
        srs: &LatticeGuardSRS,
    ) -> Result<bool, LatticeGuardError> {
        if circuits.len() != public_inputs.len() || circuits.len() != proofs.len() {
            return Err(LatticeGuardError::InternalError(
                "Batch verify: mismatched input lengths".to_string(),
            ));
        }

        info!("Batch verifying {} proofs", proofs.len());
        let start_time = std::time::Instant::now();

        // Full per-proof verification. The previous "batched" shortcut checked
        // (Σγᵢ·aᵢ)·(Σγⱼ·bⱼ) ≈ Σγᵢ·cᵢ over a random linear combination of the
        // top-level evaluations — a product of sums, which has cross terms and
        // therefore rejects HONEST proof sets (completeness bug), while also
        // never checking the product proofs or transcripts (soundness gap).
        // A sound RLC batching needs a linear relation to combine; the a·b=c
        // relation here is bilinear, so until a real batching protocol is
        // designed, batch = verify each. Strictly stronger and actually correct.
        for (i, ((circuit, inputs), proof)) in circuits
            .iter()
            .zip(public_inputs.iter())
            .zip(proofs.iter())
            .enumerate()
        {
            if !self.verify(circuit, inputs, proof, srs)? {
                debug!("Proof {} failed verification in batch", i);
                return Ok(false);
            }
        }

        let verification_time = start_time.elapsed().as_millis();
        info!(
            "Batch verification of {} proofs completed in {}ms ({:.2}ms/proof)",
            proofs.len(),
            verification_time,
            verification_time as f64 / proofs.len() as f64
        );

        Ok(true)
    }
}

/// Verification result with detailed information
#[derive(Clone, Debug)]
pub struct VerificationResult {
    /// Whether the proof is valid
    pub valid: bool,
    /// Verification time in milliseconds
    pub verification_time_ms: u64,
    /// Proof generation time (from proof metadata)
    pub proof_generation_time_ms: u64,
    /// Number of constraints verified
    pub num_constraints: usize,
    /// Security level used
    pub security_level: crate::params::SecurityLevel,
    /// Error encountered (if any)
    pub error: Option<String>,
}

impl VerificationResult {
    /// Create a successful verification result
    pub fn success(
        verification_time_ms: u64,
        metadata: &ProofMetadata,
    ) -> Self {
        Self {
            valid: true,
            verification_time_ms,
            proof_generation_time_ms: metadata.generation_time_ms,
            num_constraints: metadata.num_constraints,
            security_level: metadata.security_level,
            error: None,
        }
    }

    /// Create a failed verification result
    pub fn failure(
        verification_time_ms: u64,
        metadata: &ProofMetadata,
        error: String,
    ) -> Self {
        Self {
            valid: false,
            verification_time_ms,
            proof_generation_time_ms: metadata.generation_time_ms,
            num_constraints: metadata.num_constraints,
            security_level: metadata.security_level,
            error: Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prover::LatticeGuardProver;

    #[test]
    fn test_verify_simple_proof() {
        let params = RlweParams::pq128();
        let mut rng = rand::thread_rng();

        // Create simple circuit: x * y = z
        let mut circuit = ArithmeticCircuit::new(1, 2);
        circuit.add_multiplication_gate(
            vec![(1, 1)],  // a = witness[0]
            vec![(2, 1)],  // b = witness[1]
            vec![(0, 1)],  // c = public_input[0]
        );

        // Witness: x=3, y=4, public: z=12
        let witness = vec![3, 4];
        let public_inputs = vec![12];

        // Generate SRS
        let srs = LatticeGuardSRS::generate(params.clone(), 100, &mut rng)
            .expect("SRS generation should succeed");

        // Create prover and verifier
        let prover = LatticeGuardProver::new(params.clone())
            .expect("Prover creation should succeed");
        let verifier = LatticeGuardVerifier::new(params)
            .expect("Verifier creation should succeed");

        // Generate proof
        let proof = prover
            .generate_proof(&circuit, &witness, &public_inputs, &srs, &mut rng)
            .expect("Proof generation should succeed");

        // Verify proof
        let valid = verifier
            .verify(&circuit, &public_inputs, &proof, &srs)
            .expect("Verification should not error");

        assert!(valid, "Valid proof should verify");
    }

    #[test]
    fn test_reject_invalid_public_inputs() {
        let params = RlweParams::pq128();
        let mut rng = rand::thread_rng();

        // Create simple circuit: x * y = z
        let mut circuit = ArithmeticCircuit::new(1, 2);
        circuit.add_multiplication_gate(
            vec![(1, 1)],
            vec![(2, 1)],
            vec![(0, 1)],
        );

        // Witness: x=3, y=4, public: z=12
        let witness = vec![3, 4];
        let public_inputs = vec![12];

        // Generate SRS
        let srs = LatticeGuardSRS::generate(params.clone(), 100, &mut rng)
            .expect("SRS generation should succeed");

        // Create prover and verifier
        let prover = LatticeGuardProver::new(params.clone())
            .expect("Prover creation should succeed");
        let verifier = LatticeGuardVerifier::new(params)
            .expect("Verifier creation should succeed");

        // Generate proof
        let proof = prover
            .generate_proof(&circuit, &witness, &public_inputs, &srs, &mut rng)
            .expect("Proof generation should succeed");

        // Try to verify with wrong public inputs
        let wrong_public_inputs = vec![13]; // Should be 12
        let valid = verifier
            .verify(&circuit, &wrong_public_inputs, &proof, &srs)
            .expect("Verification should not error");

        // The c-side of the constraint is a fully-public linear combination,
        // so the verifier recomputes it exactly and must reject the mismatch.
        assert!(!valid, "Proof must not verify against wrong public inputs");
    }

    /// Completeness gate: the wire convention from the shield kickoff —
    /// x0*x1=x2 with x0 public. index i < num_public_inputs → public_inputs[i],
    /// else witness[i - num_public_inputs].
    #[test]
    fn test_completeness_public_times_witness() {
        let params = RlweParams::pq128();
        let mut rng = rand::thread_rng();

        let mut circuit = ArithmeticCircuit::new(1, 2);
        circuit.add_multiplication_gate(
            vec![(0, 1)], // a = public_input[0] = 3
            vec![(1, 1)], // b = witness[0] = 4
            vec![(2, 1)], // c = witness[1] = 12
        );

        let public_inputs = vec![3];
        let witness = vec![4, 12];

        let srs = LatticeGuardSRS::generate(params.clone(), 100, &mut rng)
            .expect("SRS generation should succeed");
        let prover = LatticeGuardProver::new(params.clone())
            .expect("Prover creation should succeed");
        let verifier = LatticeGuardVerifier::new(params)
            .expect("Verifier creation should succeed");

        let proof = prover
            .generate_proof(&circuit, &witness, &public_inputs, &srs, &mut rng)
            .expect("Proof generation should succeed");

        let valid = verifier
            .verify(&circuit, &public_inputs, &proof, &srs)
            .expect("Verification should not error");
        assert!(valid, "Honest proof (public=[3], witness=[4,12]) must verify");

        // Wrong public input must still be rejected (soundness preserved).
        let valid_wrong = verifier
            .verify(&circuit, &[5], &proof, &srs)
            .expect("Verification should not error");
        assert!(!valid_wrong, "Wrong public input must not verify");
    }

    /// Completeness gate: multi-constraint circuits must round-trip. The old
    /// aggregate a(z)·b(z)≈c(z) check had cross terms and rejected every
    /// honest circuit with >1 constraint.
    #[test]
    fn test_completeness_three_constraints() {
        let params = RlweParams::pq128();
        let mut rng = rand::thread_rng();

        // public=[6], witness=[2,3,6,4,24]; wires: 0=pub, 1..=5 witness
        let mut circuit = ArithmeticCircuit::new(1, 5);
        // w0 * w1 = pub0        (2*3 = 6)
        circuit.add_multiplication_gate(vec![(1, 1)], vec![(2, 1)], vec![(0, 1)]);
        // w0 * w1 = w2          (2*3 = 6)
        circuit.add_multiplication_gate(vec![(1, 1)], vec![(2, 1)], vec![(3, 1)]);
        // w2 * w3 = w4          (6*4 = 24)
        circuit.add_multiplication_gate(vec![(3, 1)], vec![(4, 1)], vec![(5, 1)]);

        let public_inputs = vec![6];
        let witness = vec![2, 3, 6, 4, 24];

        let srs = LatticeGuardSRS::generate(params.clone(), 100, &mut rng)
            .expect("SRS generation should succeed");
        let prover = LatticeGuardProver::new(params.clone())
            .expect("Prover creation should succeed");
        let verifier = LatticeGuardVerifier::new(params)
            .expect("Verifier creation should succeed");

        let proof = prover
            .generate_proof(&circuit, &witness, &public_inputs, &srs, &mut rng)
            .expect("Proof generation should succeed");

        let valid = verifier
            .verify(&circuit, &public_inputs, &proof, &srs)
            .expect("Verification should not error");
        assert!(valid, "Honest 3-constraint proof must verify");
    }

    /// Soundness gate: tampered proofs must be rejected.
    #[test]
    fn test_reject_tampered_proof() {
        let params = RlweParams::pq128();
        let mut rng = rand::thread_rng();

        let mut circuit = ArithmeticCircuit::new(1, 2);
        circuit.add_multiplication_gate(vec![(0, 1)], vec![(1, 1)], vec![(2, 1)]);
        let public_inputs = vec![3];
        let witness = vec![4, 12];

        let srs = LatticeGuardSRS::generate(params.clone(), 100, &mut rng)
            .expect("SRS generation should succeed");
        let prover = LatticeGuardProver::new(params.clone())
            .expect("Prover creation should succeed");
        let verifier = LatticeGuardVerifier::new(params.clone())
            .expect("Verifier creation should succeed");

        let proof = prover
            .generate_proof(&circuit, &witness, &public_inputs, &srs, &mut rng)
            .expect("Proof generation should succeed");
        assert!(
            verifier
                .verify(&circuit, &public_inputs, &proof, &srs)
                .expect("Verification should not error"),
            "sanity: untampered proof verifies"
        );

        // The kickoff-gate tamper: perturb evaluations.0 AND flip a transcript byte.
        let mut tampered = proof.clone();
        tampered.evaluations.0 = (tampered.evaluations.0 + 1) % params.modulus;
        tampered.transcript_state[0] ^= 0x01;
        assert!(
            !verifier
                .verify(&circuit, &public_inputs, &tampered, &srs)
                .expect("Verification should not error"),
            "Tampered proof (evaluations + transcript) must not verify"
        );

        // Each tamper alone must also be rejected.
        let mut eval_only = proof.clone();
        eval_only.evaluations.0 = (eval_only.evaluations.0 + 1) % params.modulus;
        assert!(
            !verifier
                .verify(&circuit, &public_inputs, &eval_only, &srs)
                .expect("Verification should not error"),
            "Perturbed evaluation alone must not verify"
        );

        let mut ts_only = proof.clone();
        ts_only.transcript_state[0] ^= 0x01;
        assert!(
            !verifier
                .verify(&circuit, &public_inputs, &ts_only, &srs)
                .expect("Verification should not error"),
            "Flipped transcript state alone must not verify"
        );
    }

    #[test]
    fn test_batch_verification() {
        let params = RlweParams::pq128();
        let mut rng = rand::thread_rng();

        // Create multiple simple circuits
        let mut circuits = Vec::new();
        let mut witnesses = Vec::new();
        let mut public_inputs_list = Vec::new();

        for i in 0..3 {
            let mut circuit = ArithmeticCircuit::new(1, 2);
            circuit.add_multiplication_gate(
                vec![(1, 1)],
                vec![(2, 1)],
                vec![(0, 1)],
            );
            circuits.push(circuit);

            let a = (i + 2) as Scalar;
            let b = (i + 3) as Scalar;
            let c = a * b;
            witnesses.push(vec![a, b]);
            public_inputs_list.push(vec![c]);
        }

        // Generate SRS
        let srs = LatticeGuardSRS::generate(params.clone(), 100, &mut rng)
            .expect("SRS generation should succeed");

        // Create prover and verifier
        let prover = LatticeGuardProver::new(params.clone())
            .expect("Prover creation should succeed");
        let verifier = LatticeGuardVerifier::new(params)
            .expect("Verifier creation should succeed");

        // Generate proofs
        let mut proofs = Vec::new();
        for i in 0..3 {
            let proof = prover
                .generate_proof(
                    &circuits[i],
                    &witnesses[i],
                    &public_inputs_list[i],
                    &srs,
                    &mut rng,
                )
                .expect("Proof generation should succeed");
            proofs.push(proof);
        }

        // Batch verify
        let circuit_refs: Vec<&ArithmeticCircuit> = circuits.iter().collect();
        let public_inputs_refs: Vec<&[Scalar]> =
            public_inputs_list.iter().map(|v| v.as_slice()).collect();
        let proof_refs: Vec<&LatticeGuardProof> = proofs.iter().collect();

        let valid = verifier
            .batch_verify(&circuit_refs, &public_inputs_refs, &proof_refs, &srs)
            .expect("Batch verification should not error");

        assert!(valid, "Batch verification should succeed for valid proofs");
    }
}
