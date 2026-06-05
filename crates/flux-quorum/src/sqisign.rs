//! Real SQIsign-L5 verifier behind the crypto-agile `QuorumVerifier`.
//! Enable with `--features sqisign`. Attestors (and VBC / DNS-5 / R8) sign with
//! post-quantum SQIsign Level 5; the quorum logic is unchanged — it just gets a
//! real verifier injected instead of the BLAKE3-MAC test stand-in.
use crate::quorum::QuorumVerifier;

/// Verifies M-of-N shares as SQIsign-L5 signatures (292-byte sigs, PQ-secure).
pub struct SqiSignVerifier;

impl QuorumVerifier for SqiSignVerifier {
    fn verify(&self, pubkey: &[u8], msg: &[u8], sig: &[u8]) -> bool {
        flux_sqisign::verify(msg, sig, pubkey).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{verify_quorum, QuorumMember, QuorumPolicy, SignedShare};

    #[test]
    fn real_sqisign_quorum_reaches_and_rejects_tamper() {
        // 3 attestors with real SQIsign-L5 keypairs.
        let mut members = Vec::new();
        let mut keys = Vec::new();
        for i in 0..3u32 {
            let (sk, pk) = flux_sqisign::keygen();
            members.push(QuorumMember { id: i, pubkey: pk.clone() });
            keys.push((sk, pk));
        }
        let policy = QuorumPolicy::new(2, members);
        let msg = b"flux-nations/attest/v1: identity X runs on quote Q";

        // 2 of 3 attestors sign the real message.
        let shares: Vec<SignedShare> = (0..2u32)
            .map(|i| {
                let (sk, pk) = &keys[i as usize];
                SignedShare { member: i, sig: flux_sqisign::sign(msg, sk, pk).unwrap() }
            })
            .collect();

        // quorum of 2 real PQ sigs over the real msg → reached.
        assert!(verify_quorum(&policy, msg, &shares, &SqiSignVerifier).reached());
        // the SAME sigs over a DIFFERENT msg → rejected (no replay/substitution).
        assert!(!verify_quorum(&policy, b"tampered", &shares, &SqiSignVerifier).reached());
        // sig length is the L5 size — proves it's really Level 5.
        assert_eq!(shares[0].sig.len(), 292);
    }
}
