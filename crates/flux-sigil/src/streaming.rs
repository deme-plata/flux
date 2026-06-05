//! Streaming hasher that signs the BLAKE3 root with SQIsign on `finalize()`.
//!
//! Single-pass post-quantum-authenticated hashing: feed arbitrary bytes through
//! `update`, then `finalize()` returns the BLAKE3 32-byte digest together with
//! a 177-byte SQIsign signature over that digest. A holder of the matching
//! public key can verify with [`SignedDigest::verify`].

pub struct SignedHasher {
    hasher: blake3::Hasher,
    sk: Vec<u8>,
    pk: Vec<u8>,
}

impl SignedHasher {
    /// Create a new SignedHasher bound to the given SQIsign keypair.
    pub fn new(sk: &[u8], pk: &[u8]) -> Self {
        Self {
            hasher: blake3::Hasher::new(),
            sk: sk.to_vec(),
            pk: pk.to_vec(),
        }
    }

    /// Feed bytes into the BLAKE3 hasher. Returns `&mut self` for chaining.
    pub fn update(&mut self, data: &[u8]) -> &mut Self {
        self.hasher.update(data);
        self
    }

    /// Finalize: compute the BLAKE3 root, SQIsign-sign it, return both atomically.
    pub fn finalize(&self) -> Result<SignedDigest, String> {
        let digest: [u8; 32] = *self.hasher.finalize().as_bytes();
        let signature = flux_sqisign::sign(&digest, &self.sk, &self.pk)?;
        Ok(SignedDigest { digest, signature })
    }
}

/// The output of [`SignedHasher::finalize`]: a 32-byte BLAKE3 digest plus a
/// 177-byte SQIsign signature over that digest.
#[derive(Clone, Debug)]
pub struct SignedDigest {
    pub digest: [u8; 32],
    pub signature: Vec<u8>,
}

impl SignedDigest {
    /// Verify the signature against `pk`. Returns `Ok(true)` iff `signature`
    /// is a valid SQIsign signature over `digest` under `pk`.
    pub fn verify(&self, pk: &[u8]) -> Result<bool, String> {
        flux_sqisign::verify(&self.digest, &self.signature, pk)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keypair() -> (Vec<u8>, Vec<u8>) {
        flux_sqisign::keygen()
    }

    #[test]
    fn finalize_produces_signed_digest() {
        let (sk, pk) = keypair();
        let mut h = SignedHasher::new(&sk, &pk);
        h.update(b"hello world");
        let signed = h.finalize().expect("sign");
        assert_eq!(signed.digest.len(), 32);
        assert_eq!(signed.signature.len(), flux_sqisign::signature_size());
    }

    #[test]
    fn digest_matches_raw_blake3() {
        let (sk, pk) = keypair();
        let msg = b"the quick brown fox jumps over the lazy dog";
        let mut h = SignedHasher::new(&sk, &pk);
        h.update(msg);
        let signed = h.finalize().unwrap();
        let raw: [u8; 32] = *blake3::hash(msg).as_bytes();
        assert_eq!(signed.digest, raw);
    }

    #[test]
    fn verify_succeeds_with_correct_pk() {
        let (sk, pk) = keypair();
        let mut h = SignedHasher::new(&sk, &pk);
        h.update(b"verifiable content");
        let signed = h.finalize().unwrap();
        assert!(signed.verify(&pk).unwrap());
    }

    #[test]
    fn verify_fails_with_wrong_pk() {
        let (sk, pk) = keypair();
        let (_, wrong_pk) = keypair();
        let mut h = SignedHasher::new(&sk, &pk);
        h.update(b"bound content");
        let signed = h.finalize().unwrap();
        assert!(!signed.verify(&wrong_pk).unwrap());
    }

    #[test]
    fn verify_fails_on_tampered_digest() {
        let (sk, pk) = keypair();
        let mut h = SignedHasher::new(&sk, &pk);
        h.update(b"original");
        let mut signed = h.finalize().unwrap();
        signed.digest[0] ^= 0xFF;
        assert!(!signed.verify(&pk).unwrap());
    }

    #[test]
    fn chunked_updates_match_single_update() {
        let (sk, pk) = keypair();

        let mut h1 = SignedHasher::new(&sk, &pk);
        h1.update(b"part one ").update(b"part two ").update(b"part three");
        let d1 = h1.finalize().unwrap().digest;

        let mut h2 = SignedHasher::new(&sk, &pk);
        h2.update(b"part one part two part three");
        let d2 = h2.finalize().unwrap().digest;

        assert_eq!(d1, d2);
    }

    #[test]
    fn empty_input_works() {
        let (sk, pk) = keypair();
        let h = SignedHasher::new(&sk, &pk);
        let signed = h.finalize().expect("sign empty");
        let raw_empty: [u8; 32] = *blake3::hash(b"").as_bytes();
        assert_eq!(signed.digest, raw_empty);
        assert!(signed.verify(&pk).unwrap());
    }

    #[test]
    fn large_input_works() {
        let (sk, pk) = keypair();
        let blob = vec![0xABu8; 1024 * 1024]; // 1 MiB
        let mut h = SignedHasher::new(&sk, &pk);
        h.update(&blob);
        let signed = h.finalize().unwrap();
        let raw: [u8; 32] = *blake3::hash(&blob).as_bytes();
        assert_eq!(signed.digest, raw);
        assert!(signed.verify(&pk).unwrap());
    }
}
