//! PQ-hybrid sealed box: **X25519 + real ML-KEM-1024** (FIPS 203, via
//! `pqcrypto-mlkem`/PQClean — explicitly NOT the `flux-kyberkem` stub).
//!
//! A quantum adversary that breaks X25519 still cannot read traffic without also
//! breaking ML-KEM-1024, and vice versa. The combiner (DeepSeek-reviewed):
//!
//!   key/nonce = BLAKE3.derive_key(CTX,  ss_x25519 ‖ ss_mlkem ‖ eph_pk ‖ recipient_x_pk ‖ ct_mlkem)
//!
//!   * **concatenation** (not XOR) of the two shared secrets preserves
//!     break-both security;
//!   * **binding `ct_mlkem`** into the KDF stops a re-encryption / KEM-ciphertext
//!     swap: any change to the ML-KEM ciphertext changes the derived key, so AEAD
//!     `open` fails (ML-KEM uses implicit rejection, so decapsulation of a mauled
//!     ciphertext yields a *different* shared secret rather than an error).
//!
//! A recipient publishes a [`HybridPublicKey`] = `x25519_pk ‖ ":" ‖ mlkem_ek` as
//! its address; only the holder of both secret keys can [`open_hybrid`].

use crate::{parse_pubkey_hex, PublicKey, SealedEnvelope, SecretError, SecretIdentity};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    ChaCha20Poly1305, Key, Nonce,
};
use pqcrypto_mlkem::mlkem1024;
use pqcrypto_traits::kem::{
    Ciphertext as _, PublicKey as _, SecretKey as _, SharedSecret as _,
};
use rand::rngs::OsRng;
use x25519_dalek::{EphemeralSecret, PublicKey as XPublicKey};
use zeroize::Zeroize;

/// Wire identifier for the PQ-hybrid suite.
pub const SUITE_HYBRID: &str = "x25519+mlkem1024-chacha20poly1305-blake3";
const KDF_CTX: &str = "flux-swarm-secret v1 hybrid x25519+mlkem1024 aead-key";
const NONCE_CTX: &str = "flux-swarm-secret v1 hybrid x25519+mlkem1024 aead-nonce";

/// A recipient's hybrid identity: X25519 (encryption) + ML-KEM-1024 (decapsulation).
pub struct HybridIdentity {
    x: SecretIdentity,
    kem_dk: mlkem1024::SecretKey,
    kem_ek: mlkem1024::PublicKey,
}

impl HybridIdentity {
    pub fn generate() -> Self {
        let x = SecretIdentity::generate();
        let (kem_ek, kem_dk) = mlkem1024::keypair();
        Self { x, kem_dk, kem_ek }
    }

    /// The publishable hybrid public key (the recipient's secret-bus address).
    pub fn public(&self) -> HybridPublicKey {
        HybridPublicKey {
            x: self.x.public_key(),
            kem_ek: self.kem_ek.as_bytes().to_vec(),
        }
    }

    pub fn public_hex(&self) -> String {
        self.public().to_hex()
    }
}

/// A recipient's hybrid public key: X25519 pub + ML-KEM-1024 encapsulation key.
#[derive(Debug, Clone)]
pub struct HybridPublicKey {
    pub x: PublicKey,
    pub kem_ek: Vec<u8>,
}

impl HybridPublicKey {
    /// `x25519_hex ":" mlkem_ek_hex`
    pub fn to_hex(&self) -> String {
        format!("{}:{}", hex::encode(self.x.to_bytes()), hex::encode(&self.kem_ek))
    }

    pub fn from_hex(s: &str) -> Result<Self, SecretError> {
        let (xh, kh) = s.split_once(':').ok_or(SecretError::Malformed)?;
        let x = parse_pubkey_hex(xh)?;
        let kem_ek = hex::decode(kh)?;
        // sanity: ML-KEM-1024 encapsulation key is 1568 bytes
        if mlkem1024::PublicKey::from_bytes(&kem_ek).is_err() {
            return Err(SecretError::Malformed);
        }
        Ok(Self { x, kem_ek })
    }
}

fn derive(
    ss_x: &[u8],
    ss_m: &[u8],
    eph_pk: &[u8; 32],
    recipient_x_pk: &[u8; 32],
    ct_mlkem: &[u8],
) -> ([u8; 32], [u8; 12]) {
    let mut km = Vec::with_capacity(32 + 32 + 32 + 32 + ct_mlkem.len());
    km.extend_from_slice(ss_x);
    km.extend_from_slice(ss_m);
    km.extend_from_slice(eph_pk);
    km.extend_from_slice(recipient_x_pk);
    km.extend_from_slice(ct_mlkem); // bind the KEM ciphertext (anti re-encryption)
    let key = blake3::derive_key(KDF_CTX, &km);
    let nonce_full = blake3::derive_key(NONCE_CTX, &km);
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&nonce_full[..12]);
    km.zeroize();
    (key, nonce)
}

/// Seal `plaintext` to a hybrid recipient. Confidential against an adversary
/// unless BOTH X25519 and ML-KEM-1024 are broken.
pub fn seal_hybrid(
    plaintext: &[u8],
    recipient: &HybridPublicKey,
) -> Result<SealedEnvelope, SecretError> {
    let eph_sk = EphemeralSecret::random_from_rng(OsRng);
    let eph_pk = XPublicKey::from(&eph_sk);
    let ss_x = eph_sk.diffie_hellman(&recipient.x); // consumes eph_sk → forward secrecy

    let kem_pk =
        mlkem1024::PublicKey::from_bytes(&recipient.kem_ek).map_err(|_| SecretError::Malformed)?;
    let (ss_m, ct_m) = mlkem1024::encapsulate(&kem_pk);

    let (mut key, nonce) = derive(
        ss_x.as_bytes(),
        ss_m.as_bytes(),
        eph_pk.as_bytes(),
        &recipient.x.to_bytes(),
        ct_m.as_bytes(),
    );
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: plaintext, aad: SUITE_HYBRID.as_bytes() })
        .expect("ChaCha20Poly1305 encrypt is infallible for valid key/nonce");
    key.zeroize();

    Ok(SealedEnvelope {
        v: 1,
        suite: SUITE_HYBRID.to_string(),
        eph_pk: hex::encode(eph_pk.to_bytes()),
        ct: hex::encode(ct),
        kem_ct: Some(hex::encode(ct_m.as_bytes())),
    })
}

/// Open a hybrid envelope. Errors: [`SecretError::Suite`] (not hybrid),
/// [`SecretError::Malformed`] (missing/!decodable KEM ciphertext), or
/// [`SecretError::Open`] (wrong recipient / tamper of either ciphertext).
pub fn open_hybrid(env: &SealedEnvelope, id: &HybridIdentity) -> Result<Vec<u8>, SecretError> {
    if env.suite != SUITE_HYBRID {
        return Err(SecretError::Suite(env.suite.clone()));
    }
    let eph_pk = parse_pubkey_hex(&env.eph_pk)?;
    let ct = hex::decode(&env.ct)?;
    let kem_ct_hex = env.kem_ct.as_ref().ok_or(SecretError::Malformed)?;
    let ct_m_bytes = hex::decode(kem_ct_hex)?;
    let ct_m = mlkem1024::Ciphertext::from_bytes(&ct_m_bytes).map_err(|_| SecretError::Malformed)?;

    let ss_x = id.x.dh(&eph_pk);
    let ss_m = mlkem1024::decapsulate(&ct_m, &id.kem_dk); // implicit rejection: never errors

    let (mut key, nonce) = derive(
        &ss_x,
        ss_m.as_bytes(),
        eph_pk.as_bytes(),
        &id.x.public_key().to_bytes(),
        &ct_m_bytes,
    );
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
    let pt = cipher
        .decrypt(Nonce::from_slice(&nonce), Payload { msg: &ct, aad: SUITE_HYBRID.as_bytes() })
        .map_err(|_| SecretError::Open);
    key.zeroize();
    pt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hybrid_roundtrip() {
        let bob = HybridIdentity::generate();
        let msg = b"PQ-hybrid: route QUG via pool 955c";
        let env = seal_hybrid(msg, &bob.public()).unwrap();
        assert_eq!(env.suite, SUITE_HYBRID);
        assert!(env.kem_ct.is_some());
        assert_eq!(open_hybrid(&env, &bob).unwrap(), msg);
    }

    #[test]
    fn wrong_recipient_cannot_open() {
        let bob = HybridIdentity::generate();
        let carol = HybridIdentity::generate();
        let env = seal_hybrid(b"for bob", &bob.public()).unwrap();
        // carol has the wrong X25519 AND wrong ML-KEM key → derived key differs → Open error
        assert!(matches!(open_hybrid(&env, &carol), Err(SecretError::Open)));
    }

    #[test]
    fn tampered_kem_ciphertext_is_rejected() {
        let bob = HybridIdentity::generate();
        let mut env = seal_hybrid(b"bind the KEM ct", &bob.public()).unwrap();
        let mut ctm = hex::decode(env.kem_ct.as_ref().unwrap()).unwrap();
        ctm[0] ^= 0x01;
        env.kem_ct = Some(hex::encode(ctm));
        // implicit rejection → different ss_m → different key → AEAD fails
        assert!(matches!(open_hybrid(&env, &bob), Err(SecretError::Open)));
    }

    #[test]
    fn tampered_aead_ciphertext_is_rejected() {
        let bob = HybridIdentity::generate();
        let mut env = seal_hybrid(b"x", &bob.public()).unwrap();
        let mut ct = hex::decode(&env.ct).unwrap();
        ct[0] ^= 0x01;
        env.ct = hex::encode(ct);
        assert!(matches!(open_hybrid(&env, &bob), Err(SecretError::Open)));
    }

    #[test]
    fn hybrid_pubkey_hex_roundtrips() {
        let bob = HybridIdentity::generate();
        let hexed = bob.public_hex();
        let parsed = HybridPublicKey::from_hex(&hexed).unwrap();
        assert_eq!(parsed.x.to_bytes(), bob.public().x.to_bytes());
        assert_eq!(parsed.kem_ek, bob.public().kem_ek);
        // and a message sealed to the parsed key still opens
        let env = seal_hybrid(b"via parsed addr", &parsed).unwrap();
        assert_eq!(open_hybrid(&env, &bob).unwrap(), b"via parsed addr");
    }

    #[test]
    fn x25519_only_envelope_is_not_opened_as_hybrid() {
        // A plain X25519 envelope (no kem_ct) must be rejected by the hybrid opener.
        let bob = HybridIdentity::generate();
        let plain = crate::seal(b"classic", &bob.public().x);
        assert!(matches!(open_hybrid(&plain, &bob), Err(SecretError::Suite(_))));
    }
}
