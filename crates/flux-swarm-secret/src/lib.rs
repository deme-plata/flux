//! # flux-swarm-secret — real sealed-envelope encryption for the swarm bus
//!
//! Today's swarm bus (`flux-swarm-tools`) writes **plaintext** JSON lines to
//! `/tmp/flux-swarm-messages.jsonl`: single machine, poll-only, unencrypted.
//! This crate makes a message body **secret** with genuine, audited primitives:
//!
//!   sender:    eph = X25519 ephemeral keypair (fresh PER MESSAGE → forward secrecy)
//!              ss  = X25519(eph_sk, recipient_pk)
//!              key = BLAKE3.derive_key(KDF_CTX,  ss || eph_pk || recipient_pk)
//!              nce = BLAKE3.derive_key(NONCE_CTX, ss || eph_pk || recipient_pk)[..12]
//!              ct  = ChaCha20Poly1305(key).encrypt(nce, plaintext, aad = suite)
//!   recipient: ss  = X25519(recipient_sk, eph_pk)  → same key/nce → decrypt
//!
//! Properties (classical leg, shipping today):
//!   * confidentiality + integrity (AEAD; any bit-flip → `Open` error)
//!   * forward secrecy: the ephemeral secret is consumed and dropped per message,
//!     so compromising a recipient's long-term key does NOT decrypt past traffic
//!   * key binding: both public keys are folded into the KDF, so an envelope
//!     cannot be re-targeted at a different recipient/identity
//!
//! HONEST SCOPE: this is **anonymous** encryption (sealed-box). Anyone who knows
//! the recipient's public key can `seal` to it — you get confidentiality + integrity,
//! NOT proof of *who* sent it. Sender authentication (an Ed25519 signature over the
//! envelope, binding the sender's swarm identity) is the planned companion slice; do
//! not treat a decrypted message as authenticated until that lands.
//!
//! ## PQ-hybrid (next slice — and an HONEST WARNING)
//! The intended shape is hybrid: fold an **ML-KEM-1024** shared secret into the same
//! KDF so a quantum break of X25519 alone does not read traffic. The correct combiner
//! (DeepSeek-reviewed) is: **concatenate** `ss_mlkem` after `ss_x25519` in the KDF
//! input AND **bind the ML-KEM ciphertext** (`ct_mlkem`) into it too, so an attacker
//! cannot swap the ML-KEM component (re-encryption attack). Concatenation — not XOR —
//! preserves break-both security: an attacker must break X25519 AND ML-KEM to recover
//! the key. This leg MUST use a real ML-KEM crate (`pqcrypto-mlkem` / `ml-kem`). The
//! in-tree `flux-kyberkem` crate is a STUB — its `encapsulate` returns an all-zero
//! shared secret and ignores the public key, giving every peer the same null key and
//! ZERO secrecy. Do NOT wire it.

pub mod auth;
pub mod bus;
#[cfg(feature = "pq")]
pub mod hybrid;
pub mod replay;

use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    ChaCha20Poly1305, Key, Nonce,
};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use x25519_dalek::{EphemeralSecret, PublicKey, StaticSecret};
use zeroize::Zeroize;

const KDF_CTX: &str = "flux-swarm-secret v1 sealed-box x25519 aead-key";
const NONCE_CTX: &str = "flux-swarm-secret v1 sealed-box x25519 aead-nonce";

/// Wire identifier for the cryptographic suite of an envelope. Lets a future
/// PQ-hybrid envelope coexist on the same bus without ambiguity.
pub const SUITE_X25519: &str = "x25519-chacha20poly1305-blake3";

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("AEAD open failed (wrong key, tampered ciphertext, or wrong recipient)")]
    Open,
    #[error("bad hex: {0}")]
    Hex(#[from] hex::FromHexError),
    #[error("malformed public key: expected 32 bytes, got {0}")]
    BadPubKey(usize),
    #[error("unsupported suite: {0}")]
    Suite(String),
    #[error("malformed authenticated payload (too short for sender key + signature)")]
    Malformed,
    #[error("sender authentication failed (forged signature, wrong recipient binding, or altered body)")]
    AuthFailed,
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
}

/// A swarm agent's long-term key material. The public half is what other agents
/// seal to; it doubles as the agent's routable secret-bus address.
pub struct SecretIdentity {
    sk: StaticSecret,
    pk: PublicKey,
}

impl SecretIdentity {
    /// Generate a fresh identity from the OS CSPRNG.
    pub fn generate() -> Self {
        let sk = StaticSecret::random_from_rng(OsRng);
        let pk = PublicKey::from(&sk);
        Self { sk, pk }
    }

    /// Restore an identity from 32 secret-key bytes (e.g. from a sealed keystore).
    pub fn from_sk_bytes(bytes: [u8; 32]) -> Self {
        let sk = StaticSecret::from(bytes);
        let pk = PublicKey::from(&sk);
        Self { sk, pk }
    }

    /// The public key other agents seal to.
    pub fn public_key(&self) -> PublicKey {
        self.pk
    }

    /// Hex of the public key — the agent's secret-bus address.
    pub fn public_hex(&self) -> String {
        hex::encode(self.pk.to_bytes())
    }

    /// Export the secret-key bytes (caller is responsible for sealing at rest).
    pub fn to_sk_bytes(&self) -> [u8; 32] {
        self.sk.to_bytes()
    }
}

/// A sealed message. JSON-line friendly so it drops straight into the existing
/// `flux-swarm-messages.jsonl` bus as an opaque `payload`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SealedEnvelope {
    /// Envelope format version.
    pub v: u8,
    /// Cryptographic suite ([`SUITE_X25519`] today).
    pub suite: String,
    /// Ephemeral X25519 public key (hex) — fresh per message.
    pub eph_pk: String,
    /// AEAD ciphertext + 16-byte Poly1305 tag (hex).
    pub ct: String,
    /// ML-KEM-1024 encapsulation ciphertext (hex). Present ONLY for the PQ-hybrid
    /// suite; omitted on the wire for the X25519-only suite (backward compatible).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kem_ct: Option<String>,
}

/// Parse a 32-byte X25519 public key from hex.
pub fn parse_pubkey_hex(s: &str) -> Result<PublicKey, SecretError> {
    let raw = hex::decode(s)?;
    let arr: [u8; 32] = raw
        .as_slice()
        .try_into()
        .map_err(|_| SecretError::BadPubKey(raw.len()))?;
    Ok(PublicKey::from(arr))
}

fn derive_key_nonce(ss: &[u8; 32], eph_pk: &[u8; 32], recipient_pk: &[u8; 32]) -> ([u8; 32], [u8; 12]) {
    // Bind the shared secret to BOTH public keys so an envelope can't be
    // re-pointed at a different recipient or spoof the ephemeral.
    let mut km = Vec::with_capacity(96);
    km.extend_from_slice(ss);
    km.extend_from_slice(eph_pk);
    km.extend_from_slice(recipient_pk);
    let key = blake3::derive_key(KDF_CTX, &km);
    let nonce_full = blake3::derive_key(NONCE_CTX, &km);
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&nonce_full[..12]);
    km.zeroize();
    (key, nonce)
}

/// Seal `plaintext` to `recipient_pk`. Anyone can call this (sealed-box style);
/// only the holder of the matching secret key can open it.
pub fn seal(plaintext: &[u8], recipient_pk: &PublicKey) -> SealedEnvelope {
    let eph_sk = EphemeralSecret::random_from_rng(OsRng);
    let eph_pk = PublicKey::from(&eph_sk);
    let shared = eph_sk.diffie_hellman(recipient_pk); // consumes eph_sk → forward secrecy
    let (mut key, nonce) = derive_key_nonce(shared.as_bytes(), eph_pk.as_bytes(), &recipient_pk.to_bytes());

    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
    let ct = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload { msg: plaintext, aad: SUITE_X25519.as_bytes() },
        )
        .expect("ChaCha20Poly1305 encrypt is infallible for valid key/nonce");
    key.zeroize();

    SealedEnvelope {
        v: 1,
        suite: SUITE_X25519.to_string(),
        eph_pk: hex::encode(eph_pk.to_bytes()),
        ct: hex::encode(ct),
        kem_ct: None,
    }
}

impl SecretIdentity {
    /// X25519 DH with another public key → 32 shared bytes. Crate-internal so the
    /// PQ-hybrid module can reuse the encryption identity without exposing the sk.
    pub(crate) fn dh(&self, their_public: &PublicKey) -> [u8; 32] {
        self.sk.diffie_hellman(their_public).to_bytes()
    }
}

/// Open a sealed envelope with the recipient identity. Returns [`SecretError::Open`]
/// on any tamper / wrong-key / wrong-recipient.
pub fn open(env: &SealedEnvelope, id: &SecretIdentity) -> Result<Vec<u8>, SecretError> {
    if env.suite != SUITE_X25519 {
        return Err(SecretError::Suite(env.suite.clone()));
    }
    let eph_pk = parse_pubkey_hex(&env.eph_pk)?;
    let ct = hex::decode(&env.ct)?;
    let shared = id.sk.diffie_hellman(&eph_pk);
    let (mut key, nonce) =
        derive_key_nonce(shared.as_bytes(), eph_pk.as_bytes(), &id.pk.to_bytes());

    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
    let pt = cipher
        .decrypt(
            Nonce::from_slice(&nonce),
            Payload { msg: &ct, aad: SUITE_X25519.as_bytes() },
        )
        .map_err(|_| SecretError::Open);
    key.zeroize();
    pt
}

/// Convenience: seal to a hex public-key address, returning a JSON string ready
/// to drop into the swarm bus `payload`.
pub fn seal_to_hex(plaintext: &[u8], recipient_pk_hex: &str) -> Result<String, SecretError> {
    let pk = parse_pubkey_hex(recipient_pk_hex)?;
    Ok(serde_json::to_string(&seal(plaintext, &pk))?)
}

/// Convenience: open a JSON envelope string with the recipient identity.
pub fn open_from_json(json: &str, id: &SecretIdentity) -> Result<Vec<u8>, SecretError> {
    let env: SealedEnvelope = serde_json::from_str(json)?;
    open(&env, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let bob = SecretIdentity::generate();
        let msg = b"route QUG -> rocky LP pool 955c; sigil rc9 cut pending";
        let env = seal(msg, &bob.public_key());
        assert_eq!(open(&env, &bob).unwrap(), msg);
    }

    #[test]
    fn wrong_recipient_cannot_open() {
        let bob = SecretIdentity::generate();
        let carol = SecretIdentity::generate();
        let env = seal(b"for bob only", &bob.public_key());
        assert!(matches!(open(&env, &carol), Err(SecretError::Open)));
    }

    #[test]
    fn tampered_ciphertext_is_rejected() {
        let bob = SecretIdentity::generate();
        let mut env = seal(b"untampered", &bob.public_key());
        // flip one nibble of the ciphertext hex
        let mut bytes = hex::decode(&env.ct).unwrap();
        bytes[0] ^= 0x01;
        env.ct = hex::encode(bytes);
        assert!(matches!(open(&env, &bob), Err(SecretError::Open)));
    }

    #[test]
    fn tampered_ephemeral_key_is_rejected() {
        let bob = SecretIdentity::generate();
        let mut env = seal(b"bind check", &bob.public_key());
        let mut ek = hex::decode(&env.eph_pk).unwrap();
        ek[0] ^= 0x01;
        env.eph_pk = hex::encode(ek);
        assert!(matches!(open(&env, &bob), Err(SecretError::Open)));
    }

    #[test]
    fn forward_secrecy_fresh_ephemeral_each_message() {
        let bob = SecretIdentity::generate();
        let a = seal(b"same plaintext", &bob.public_key());
        let b = seal(b"same plaintext", &bob.public_key());
        // fresh ephemeral per message → different eph_pk and different ciphertext
        assert_ne!(a.eph_pk, b.eph_pk);
        assert_ne!(a.ct, b.ct);
        // but both still decrypt to the same plaintext
        assert_eq!(open(&a, &bob).unwrap(), open(&b, &bob).unwrap());
    }

    #[test]
    fn empty_plaintext_roundtrips() {
        let bob = SecretIdentity::generate();
        let env = seal(b"", &bob.public_key());
        assert_eq!(open(&env, &bob).unwrap(), b"");
    }

    #[test]
    fn json_bus_roundtrip_via_hex_address() {
        let bob = SecretIdentity::generate();
        let json = seal_to_hex(b"over the wire", &bob.public_hex()).unwrap();
        assert_eq!(open_from_json(&json, &bob).unwrap(), b"over the wire");
    }

    #[test]
    fn identity_persists_across_sk_bytes() {
        let id = SecretIdentity::generate();
        let restored = SecretIdentity::from_sk_bytes(id.to_sk_bytes());
        assert_eq!(id.public_hex(), restored.public_hex());
        let env = seal(b"persist", &id.public_key());
        assert_eq!(open(&env, &restored).unwrap(), b"persist");
    }
}
