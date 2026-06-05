//! The M-of-N quorum core.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// Inject the signature scheme here. The quorum logic calls `verify` and never
/// knows whether it's SQIsign, Dilithium, or a test MAC — that's the crypto
/// agility rule: consensus code dispatches on a scheme, it doesn't bake one in.
pub trait QuorumVerifier {
    /// Does `sig` verify over `msg` under `pubkey`?
    fn verify(&self, pubkey: &[u8], msg: &[u8], sig: &[u8]) -> bool;
}

/// A member of the signing set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuorumMember {
    /// Stable id (a settlement identity / validator index).
    pub id: u32,
    /// The member's public key bytes (scheme-specific; opaque here).
    pub pubkey: Vec<u8>,
}

/// An M-of-N policy: at least `m` of `members` must sign.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuorumPolicy {
    /// Threshold.
    pub m: usize,
    /// The signing set (N = `members.len()`).
    pub members: Vec<QuorumMember>,
}

impl QuorumPolicy {
    /// New policy. `m` is clamped to `1..=members.len()`.
    pub fn new(m: usize, members: Vec<QuorumMember>) -> Self {
        let n = members.len().max(1);
        QuorumPolicy { m: m.clamp(1, n), members }
    }
    /// N — the size of the signing set.
    pub fn n(&self) -> usize {
        self.members.len()
    }
}

/// One member's signature share over the message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedShare {
    /// Which member signed.
    pub member: u32,
    /// The signature bytes.
    pub sig: Vec<u8>,
}

/// Why a share didn't count.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RejectReason {
    /// Share's member id isn't in the policy's set.
    UnknownMember,
    /// Signature didn't verify under the member's pubkey.
    InvalidSig,
    /// This member already contributed a (valid-or-not) share; counted once.
    DuplicateSigner,
}

/// Did the quorum form?
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuorumOutcome {
    /// ≥ M distinct members produced valid signatures.
    Reached {
        /// The members whose valid sigs formed the quorum.
        signers: Vec<u32>,
        /// Threshold required.
        m: usize,
        /// Set size.
        n: usize,
    },
    /// Fewer than M valid distinct signatures.
    NotReached {
        /// Valid distinct sigs collected.
        valid: usize,
        /// Threshold required.
        m: usize,
        /// Set size.
        n: usize,
    },
}

/// The verifier's full report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuorumReport {
    /// Reached or not.
    pub outcome: QuorumOutcome,
    /// Shares that were dropped, with why (for audit / slashing).
    pub rejected: Vec<(u32, RejectReason)>,
}

impl QuorumReport {
    /// Convenience: did the quorum form?
    pub fn reached(&self) -> bool {
        matches!(self.outcome, QuorumOutcome::Reached { .. })
    }
}

/// Verify an M-of-N quorum: count distinct members whose signature over `msg`
/// verifies under their policy pubkey; quorum forms iff that count ≥ `policy.m`.
/// Deterministic; one member counts at most once.
pub fn verify_quorum(
    policy: &QuorumPolicy,
    msg: &[u8],
    shares: &[SignedShare],
    verifier: &dyn QuorumVerifier,
) -> QuorumReport {
    let mut seen: HashSet<u32> = HashSet::new();
    let mut signers: Vec<u32> = Vec::new();
    let mut rejected: Vec<(u32, RejectReason)> = Vec::new();

    for share in shares {
        if !seen.insert(share.member) {
            rejected.push((share.member, RejectReason::DuplicateSigner));
            continue;
        }
        match policy.members.iter().find(|m| m.id == share.member) {
            None => rejected.push((share.member, RejectReason::UnknownMember)),
            Some(mem) => {
                if verifier.verify(&mem.pubkey, msg, &share.sig) {
                    signers.push(share.member);
                } else {
                    rejected.push((share.member, RejectReason::InvalidSig));
                }
            }
        }
    }

    signers.sort_unstable();
    let (m, n) = (policy.m, policy.n());
    let outcome = if signers.len() >= m {
        QuorumOutcome::Reached { signers, m, n }
    } else {
        QuorumOutcome::NotReached { valid: signers.len(), m, n }
    };
    QuorumReport { outcome, rejected }
}

/// Dev/test scheme: `sig == BLAKE3(pubkey ‖ msg)`. NOT for production — it's a
/// MAC anyone holding the pubkey can forge. The real path is the `sqisign`
/// feature's verifier. Exists so the quorum LOGIC is testable without dragging
/// SQIsign keygen into every unit test.
pub struct Blake3MacVerifier;

/// Produce a [`Blake3MacVerifier`]-valid signature (test helper).
pub fn blake3_mac(pubkey: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut h = blake3::Hasher::new();
    h.update(pubkey);
    h.update(msg);
    h.finalize().as_bytes().to_vec()
}

impl QuorumVerifier for Blake3MacVerifier {
    fn verify(&self, pubkey: &[u8], msg: &[u8], sig: &[u8]) -> bool {
        sig == blake3_mac(pubkey, msg).as_slice()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(id: u32) -> QuorumMember {
        QuorumMember { id, pubkey: format!("pk-{id}").into_bytes() }
    }
    fn policy(m: usize, n: u32) -> QuorumPolicy {
        QuorumPolicy::new(m, (0..n).map(member).collect())
    }
    /// A valid share from member `id` over `msg`.
    fn share(id: u32, msg: &[u8]) -> SignedShare {
        SignedShare { member: id, sig: blake3_mac(format!("pk-{id}").as_bytes(), msg) }
    }

    #[test]
    fn three_of_five_reached() {
        let p = policy(3, 5);
        let msg = b"release v0.0.5";
        let shares = vec![share(0, msg), share(1, msg), share(2, msg)];
        let r = verify_quorum(&p, msg, &shares, &Blake3MacVerifier);
        assert!(r.reached());
        assert_eq!(r.outcome, QuorumOutcome::Reached { signers: vec![0, 1, 2], m: 3, n: 5 });
    }

    #[test]
    fn two_of_five_not_reached() {
        let p = policy(3, 5);
        let msg = b"release v0.0.5";
        let shares = vec![share(0, msg), share(1, msg)];
        let r = verify_quorum(&p, msg, &shares, &Blake3MacVerifier);
        assert!(!r.reached());
        assert!(matches!(r.outcome, QuorumOutcome::NotReached { valid: 2, m: 3, n: 5 }));
    }

    #[test]
    fn duplicate_signer_counts_once() {
        let p = policy(3, 5);
        let msg = b"x";
        // member 0 signs three times — still one vote, two duplicates rejected.
        let shares = vec![share(0, msg), share(0, msg), share(0, msg), share(1, msg)];
        let r = verify_quorum(&p, msg, &shares, &Blake3MacVerifier);
        assert!(!r.reached(), "2 distinct < 3");
        assert_eq!(r.rejected.iter().filter(|(_, why)| *why == RejectReason::DuplicateSigner).count(), 2);
    }

    #[test]
    fn unknown_member_rejected() {
        let p = policy(2, 3);
        let msg = b"x";
        let shares = vec![share(0, msg), share(99, msg)]; // 99 not in set
        let r = verify_quorum(&p, msg, &shares, &Blake3MacVerifier);
        assert!(!r.reached());
        assert!(r.rejected.contains(&(99, RejectReason::UnknownMember)));
    }

    #[test]
    fn invalid_sig_rejected() {
        let p = policy(2, 3);
        let msg = b"x";
        let bad = SignedShare { member: 1, sig: b"forged".to_vec() };
        let shares = vec![share(0, msg), bad];
        let r = verify_quorum(&p, msg, &shares, &Blake3MacVerifier);
        assert!(!r.reached(), "1 valid < 2");
        assert!(r.rejected.contains(&(1, RejectReason::InvalidSig)));
    }

    #[test]
    fn wrong_message_does_not_verify() {
        // A sig over a DIFFERENT message must not count (replay/substitution).
        let p = policy(1, 3);
        let signed_over = share(0, b"original");
        let r = verify_quorum(&p, b"tampered", &[signed_over], &Blake3MacVerifier);
        assert!(!r.reached());
    }
}
