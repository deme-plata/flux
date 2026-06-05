//! Verifiable Build Consensus (VBC) — the BURST-3 core.
//!
//! A [`BuildClaim`] is one worker's signed assertion: "I compiled `unit` from
//! `source_hash` with `toolchain_hash`, and the artifact hashed to
//! `artifact_hash`." [`verify_consensus`] takes the claims from N workers for
//! the SAME unit and decides whether to trust the result:
//!
//! 1. **Provenance** — every claim's `fingerprint` must bind its own contents
//!    (a tampered claim is caught immediately). In production this is a SQIsign
//!    `.proof`; here it's the BLAKE3 fingerprint of the canonical bytes, which
//!    is the same binding minus the asymmetric key (wired in BURST-2).
//! 2. **Comparability** — all workers must have built the SAME `source_hash`
//!    and `toolchain_hash`. Comparing artifacts across different source or
//!    compiler is meaningless, so a mismatch is rejected outright.
//! 3. **Consensus** — group the claims by `artifact_hash`. If one hash reaches
//!    `quorum` votes, that's the trusted artifact; any worker that produced a
//!    different hash is flagged [`Byzantine`] (it tampered the *compilation*,
//!    not the claim — the attack the fingerprint alone can't catch). If no
//!    hash reaches quorum, the build is rejected.
//!
//! This is the exact shape of SIGIL's root-divergence check: independent
//! parties recompute the same thing; agreement is trust, divergence is a
//! caught attack.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// 32-byte BLAKE3 digest.
pub type Hash = [u8; 32];

/// One worker's signed build assertion for a single compile unit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildClaim {
    /// Which worker produced this (a settlement identity in the real mesh).
    pub worker: u32,
    /// Compile unit (crate) name.
    pub unit: String,
    /// BLAKE3 of the unit's source — must match across workers to be comparable.
    pub source_hash: Hash,
    /// BLAKE3 of (rustc version ∥ target ∥ flags) — comparability gate.
    pub toolchain_hash: Hash,
    /// BLAKE3 of the produced artifact — the SUBJECT of consensus.
    pub artifact_hash: Hash,
    /// Provenance binding over the canonical claim bytes (SQIsign in prod).
    pub fingerprint: Hash,
}

impl BuildClaim {
    /// Build a self-consistent claim (computes the binding fingerprint).
    pub fn new(
        worker: u32,
        unit: impl Into<String>,
        source_hash: Hash,
        toolchain_hash: Hash,
        artifact_hash: Hash,
    ) -> Self {
        let mut c = BuildClaim {
            worker,
            unit: unit.into(),
            source_hash,
            toolchain_hash,
            artifact_hash,
            fingerprint: [0u8; 32],
        };
        c.fingerprint = c.compute_fingerprint();
        c
    }

    /// Canonical bytes the fingerprint binds.
    fn signing_bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(8 + self.unit.len() + 96);
        b.extend_from_slice(&self.worker.to_le_bytes());
        b.extend_from_slice(self.unit.as_bytes());
        b.extend_from_slice(&self.source_hash);
        b.extend_from_slice(&self.toolchain_hash);
        b.extend_from_slice(&self.artifact_hash);
        b
    }

    fn compute_fingerprint(&self) -> Hash {
        *blake3::hash(&self.signing_bytes()).as_bytes()
    }

    /// Does the fingerprint bind these exact contents? (catches claim tampering)
    pub fn proof_valid(&self) -> bool {
        self.fingerprint == self.compute_fingerprint()
    }
}

/// A worker whose artifact disagreed with the quorum — it tampered the build.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Byzantine {
    /// The offending worker.
    pub worker: u32,
    /// The hash it produced.
    pub their_hash: Hash,
    /// The hash the quorum agreed on.
    pub majority_hash: Hash,
}

/// Why a build was rejected.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VbcReject {
    /// No claims supplied.
    Empty,
    /// A claim's fingerprint didn't bind its contents.
    InvalidProof {
        /// The worker whose claim failed verification.
        worker: u32,
    },
    /// Workers built different source — not comparable.
    SourceMismatch,
    /// Workers used different toolchains — not comparable.
    ToolchainMismatch,
    /// No single artifact hash reached the quorum.
    NoQuorum {
        /// Votes the leading hash got.
        top_votes: usize,
        /// Votes that were required.
        quorum: usize,
    },
}

/// The trust decision for a unit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VbcOutcome {
    /// A quorum agreed — this artifact is trusted.
    Accepted {
        /// The agreed artifact hash (link this one).
        artifact_hash: Hash,
        /// Workers that agreed.
        agreed: Vec<u32>,
        /// How many agreed.
        votes: usize,
        /// Quorum that was required.
        quorum: usize,
    },
    /// Rejected — see the reason.
    Rejected(VbcReject),
}

/// The full verifier report: the decision + any byzantine workers exposed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VbcReport {
    /// Accept (trusted artifact) or reject.
    pub outcome: VbcOutcome,
    /// Workers caught producing a divergent artifact (to slash / de-rank).
    pub byzantine: Vec<Byzantine>,
}

impl VbcReport {
    /// Convenience: did consensus accept a trusted artifact?
    pub fn accepted(&self) -> Option<Hash> {
        match self.outcome {
            VbcOutcome::Accepted { artifact_hash, .. } => Some(artifact_hash),
            _ => None,
        }
    }
}

/// Verify build consensus over `claims` (all for the same unit) requiring
/// `quorum` agreeing workers. Pure + deterministic.
pub fn verify_consensus(claims: &[BuildClaim], quorum: usize) -> VbcReport {
    let reject = |r| VbcReport { outcome: VbcOutcome::Rejected(r), byzantine: vec![] };

    if claims.is_empty() {
        return reject(VbcReject::Empty);
    }
    // 1. provenance: every claim must bind its own contents.
    for c in claims {
        if !c.proof_valid() {
            return reject(VbcReject::InvalidProof { worker: c.worker });
        }
    }
    // 2. comparability: same source + toolchain across all workers.
    let s0 = claims[0].source_hash;
    if claims.iter().any(|c| c.source_hash != s0) {
        return reject(VbcReject::SourceMismatch);
    }
    let t0 = claims[0].toolchain_hash;
    if claims.iter().any(|c| c.toolchain_hash != t0) {
        return reject(VbcReject::ToolchainMismatch);
    }
    // 3. consensus: group by artifact hash, find the leader.
    let mut groups: HashMap<Hash, Vec<u32>> = HashMap::new();
    for c in claims {
        groups.entry(c.artifact_hash).or_default().push(c.worker);
    }
    // Deterministic max: most votes, tie-broken by smallest hash.
    let (maj_hash, maj_workers) = groups
        .iter()
        .max_by(|a, b| a.1.len().cmp(&b.1.len()).then(b.0.cmp(a.0)))
        .map(|(h, w)| (*h, w.clone()))
        .expect("non-empty");

    if maj_workers.len() >= quorum {
        let mut agreed = maj_workers;
        agreed.sort_unstable();
        let byzantine: Vec<Byzantine> = claims
            .iter()
            .filter(|c| c.artifact_hash != maj_hash)
            .map(|c| Byzantine {
                worker: c.worker,
                their_hash: c.artifact_hash,
                majority_hash: maj_hash,
            })
            .collect();
        VbcReport {
            outcome: VbcOutcome::Accepted {
                artifact_hash: maj_hash,
                votes: agreed.len(),
                agreed,
                quorum,
            },
            byzantine,
        }
    } else {
        reject(VbcReject::NoQuorum {
            top_votes: maj_workers.len(),
            quorum,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(seed: u8) -> Hash {
        *blake3::hash(&[seed]).as_bytes()
    }

    /// Build a claim for a worker that HONESTLY produced `artifact`.
    fn claim(worker: u32, src: Hash, tc: Hash, artifact: Hash) -> BuildClaim {
        BuildClaim::new(worker, "flux-burst", src, tc, artifact)
    }

    #[test]
    fn honest_quorum_accepts() {
        let (src, tc, good) = (h(1), h(2), h(10));
        let claims = vec![claim(1, src, tc, good), claim(2, src, tc, good), claim(3, src, tc, good)];
        let r = verify_consensus(&claims, 2);
        assert_eq!(r.accepted(), Some(good));
        assert!(r.byzantine.is_empty());
    }

    #[test]
    fn byzantine_worker_is_caught_and_flagged() {
        // 2 honest + 1 that compiled a BACKDOORED artifact (different hash).
        let (src, tc, good, evil) = (h(1), h(2), h(10), h(99));
        let claims = vec![
            claim(1, src, tc, good),
            claim(2, src, tc, good),
            claim(3, src, tc, evil), // backdoor
        ];
        let r = verify_consensus(&claims, 2);
        // The trusted artifact is the honest one — the backdoor never links.
        assert_eq!(r.accepted(), Some(good));
        // And worker 3 is exposed for slashing.
        assert_eq!(r.byzantine.len(), 1);
        assert_eq!(r.byzantine[0].worker, 3);
        assert_eq!(r.byzantine[0].their_hash, evil);
        assert_eq!(r.byzantine[0].majority_hash, good);
    }

    #[test]
    fn no_quorum_rejects() {
        // 3 workers, 3 different artifacts → nobody agrees → reject (don't link).
        let (src, tc) = (h(1), h(2));
        let claims = vec![claim(1, src, tc, h(10)), claim(2, src, tc, h(11)), claim(3, src, tc, h(12))];
        let r = verify_consensus(&claims, 2);
        assert!(r.accepted().is_none());
        assert!(matches!(r.outcome, VbcOutcome::Rejected(VbcReject::NoQuorum { .. })));
    }

    #[test]
    fn claim_tampering_is_caught_by_fingerprint() {
        // Worker reports a different artifact_hash than it signed → invalid proof.
        let (src, tc) = (h(1), h(2));
        let mut c = claim(1, src, tc, h(10));
        c.artifact_hash = h(99); // tamper AFTER signing — fingerprint no longer binds
        let claims = vec![c, claim(2, src, tc, h(10)), claim(3, src, tc, h(10))];
        let r = verify_consensus(&claims, 2);
        assert!(matches!(r.outcome, VbcOutcome::Rejected(VbcReject::InvalidProof { worker: 1 })));
    }

    #[test]
    fn source_mismatch_rejects() {
        // Two workers built different source — not comparable.
        let tc = h(2);
        let claims = vec![claim(1, h(1), tc, h(10)), claim(2, h(7), tc, h(10))];
        let r = verify_consensus(&claims, 2);
        assert!(matches!(r.outcome, VbcOutcome::Rejected(VbcReject::SourceMismatch)));
    }

    /// The BURST-5 preview: a deterministic "3 workers, 1 byzantine" scenario,
    /// the exact thing the chronos sim will replay over flux-p2p.
    #[test]
    fn scenario_majority_outvotes_single_attacker() {
        let (src, tc, good, evil) = (h(1), h(2), h(50), h(51));
        // 4 honest, 1 attacker, quorum 3.
        let claims = vec![
            claim(1, src, tc, good),
            claim(2, src, tc, good),
            claim(3, src, tc, evil),
            claim(4, src, tc, good),
            claim(5, src, tc, good),
        ];
        let r = verify_consensus(&claims, 3);
        assert_eq!(r.accepted(), Some(good));
        assert_eq!(r.byzantine.len(), 1);
        assert_eq!(r.byzantine[0].worker, 3);
    }
}
