//! BLAKE3 keyed-hash with a 32-byte key derived from a SQIsign signature.
//!
//! Key derivation uses BLAKE3's `derive_key` with a domain-separation context,
//! so a key derived for flux-sigil keyed hashing cannot collide with a key derived
//! by any other consumer of BLAKE3 KDF, even with the same input signature.

const KDF_CONTEXT: &str = "flux-sigil 2026-05-29 sqisign-signature-to-key v1";

#[derive(Clone)]
pub struct SigilKey {
    key: [u8; 32],
}

impl SigilKey {
    /// Derive a 32-byte BLAKE3 keyed-hash key from a SQIsign signature.
    ///
    /// Any byte slice is accepted; typical input is a 177-byte SQIsign
    /// signature from `flux_sqisign::sign`.
    pub fn from_sqisign_signature(sig: &[u8]) -> Self {
        Self { key: blake3::derive_key(KDF_CONTEXT, sig) }
    }

    /// Construct a SigilKey directly from 32 bytes of keying material —
    /// for callers who already have an externally-derived key.
    pub fn from_raw_key(key: [u8; 32]) -> Self {
        Self { key }
    }

    /// Compute the BLAKE3 keyed hash of `msg` under this key.
    pub fn hash(&self, msg: &[u8]) -> [u8; 32] {
        *blake3::keyed_hash(&self.key, msg).as_bytes()
    }

    /// Expose the raw 32-byte key (for serialization or cross-crate use).
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.key
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_sig(seed: u8) -> Vec<u8> {
        (0..177u8).map(|i| i.wrapping_add(seed)).collect()
    }

    #[test]
    fn derive_is_deterministic() {
        let sig = fake_sig(0);
        let k1 = SigilKey::from_sqisign_signature(&sig);
        let k2 = SigilKey::from_sqisign_signature(&sig);
        assert_eq!(k1.as_bytes(), k2.as_bytes());
    }

    #[test]
    fn different_sigs_yield_different_keys() {
        let k1 = SigilKey::from_sqisign_signature(&fake_sig(0));
        let k2 = SigilKey::from_sqisign_signature(&fake_sig(1));
        assert_ne!(k1.as_bytes(), k2.as_bytes());
    }

    #[test]
    fn hash_is_32_bytes() {
        let k = SigilKey::from_sqisign_signature(&fake_sig(42));
        assert_eq!(k.hash(b"hello").len(), 32);
    }

    #[test]
    fn hash_is_deterministic() {
        let k = SigilKey::from_sqisign_signature(&fake_sig(7));
        assert_eq!(k.hash(b"flux-sigil"), k.hash(b"flux-sigil"));
    }

    #[test]
    fn different_keys_yield_different_hashes() {
        let k1 = SigilKey::from_sqisign_signature(&fake_sig(0));
        let k2 = SigilKey::from_sqisign_signature(&fake_sig(1));
        assert_ne!(k1.hash(b"same message"), k2.hash(b"same message"));
    }

    #[test]
    fn different_messages_yield_different_hashes() {
        let k = SigilKey::from_sqisign_signature(&fake_sig(0));
        assert_ne!(k.hash(b"a"), k.hash(b"b"));
    }

    #[test]
    fn empty_message_works() {
        let k = SigilKey::from_sqisign_signature(&fake_sig(0));
        let _ = k.hash(b"");
    }

    #[test]
    fn from_raw_key_matches_direct_blake3() {
        let raw = [7u8; 32];
        let k = SigilKey::from_raw_key(raw);
        let direct = *blake3::keyed_hash(&raw, b"verify").as_bytes();
        assert_eq!(k.hash(b"verify"), direct);
    }

    #[test]
    fn end_to_end_with_real_sqisign() {
        let (sk, pk) = flux_sqisign::keygen();
        let sig = flux_sqisign::sign(b"hello world", &sk, &pk).expect("sign");
        assert_eq!(sig.len(), flux_sqisign::signature_size());
        let k = SigilKey::from_sqisign_signature(&sig);
        let h = k.hash(b"flux-sigil keyed hash");
        assert_eq!(h.len(), 32);
        // Same sig + same msg → same hash, even after roundtripping through SQIsign
        assert_eq!(k.hash(b"flux-sigil keyed hash"), h);
    }
}
