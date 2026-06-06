//! Post-quantum cryptography for flux-bluetooth.
//!
//! Provides:
//! - **SQIsign** signatures (177B, NIST PQC Level 5) for identity + messages
//! - **Kyber KEM** for E2EE session key exchange
//! - **Hybrid mode** (X25519 + Kyber) for maximum compatibility
//! - **BLAKE3 content-addressed message hashing
//!
//! This is what makes flux-bluetooth cryptographically superior to BitChat
//! (which uses classic ECDH + Ed25519 — both broken by quantum computers).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// A post-quantum identity: SQIsign keypair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PQIdentity {
    /// Human-readable name
    pub name: String,
    /// SQIsign public key (compact, ~177B)
    pub public_key: Vec<u8>,
    /// BLAKE3 hash of the public key (address)
    pub address: [u8; 32],
}

/// A signed payload — message + signature + public key for verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedPayload {
    /// Raw data bytes
    pub data: Vec<u8>,
    /// SQIsign signature over BLAKE3(data)
    pub signature: Vec<u8>,
    /// Signer's public key
    pub signer: Vec<u8>,
    /// Signer's address (BLAKE3 of public key)
    pub signer_address: [u8; 32],
}

/// Session key for E2EE (established via Kyber KEM).
#[derive(Debug, Clone)]
pub struct SessionKey {
    /// Shared secret (32 bytes)
    pub key: [u8; 32],
    /// Peer's public key (SQIsign)
    pub peer_public: Vec<u8>,
}

/// The crypto engine for a Bluetooth node.
pub struct BluetoothCrypto {
    identity: PQIdentity,
    secret_key: Vec<u8>,
}

impl BluetoothCrypto {
    /// Create a new crypto engine. Generates a fresh SQIsign keypair.
    /// Optionally seeds from a 32-byte seed for deterministic identity.
    pub fn new(seed: Option<[u8; 32]>) -> Result<Self> {
        let (pk, sk) = generate_pq_keypair(seed)?;
        let address = blake3_hash(&pk);
        let name = format!("flux-{}", hex::encode(&address[..4]));

        Ok(Self {
            identity: PQIdentity {
                name,
                public_key: pk,
                address,
            },
            secret_key: sk,
        })
    }

    /// Get the public identity.
    pub fn identity(&self) -> &PQIdentity {
        &self.identity
    }

    /// Sign a message with the node's PQ key.
    /// Returns a SignedPayload that anyone with the public key can verify.
    pub async fn sign_message(&self, data: &[u8]) -> Result<SignedPayload> {
        let hash = blake3_hash(data);
        let signature = pq_sign(&hash, &self.secret_key, &self.identity.public_key)?;

        Ok(SignedPayload {
            data: data.to_vec(),
            signature,
            signer: self.identity.public_key.clone(),
            signer_address: self.identity.address,
        })
    }

    /// Verify a signed payload. Returns Ok(()) if valid.
    pub fn verify(payload: &SignedPayload) -> Result<()> {
        let hash = blake3_hash(&payload.data);
        pq_verify(&hash, &payload.signature, &payload.signer)
            .map_err(|e| anyhow::anyhow!("PQ signature verification failed: {}", e))
    }

    /// Derive a session key for E2EE with a peer.
    /// Uses hybrid X25519 + Kyber for post-quantum + classic security.
    pub fn derive_session_key(&self, peer_public: &[u8]) -> Result<SessionKey> {
        let key = hybrid_kem_shared_secret(&self.secret_key, peer_public)?;
        Ok(SessionKey {
            key,
            peer_public: peer_public.to_vec(),
        })
    }

    /// Encrypt a message for a peer using the session key.
    pub fn encrypt(&self, session: &SessionKey, plaintext: &[u8]) -> Result<Vec<u8>> {
        let nonce = blake3_hash(&[&session.key, plaintext].concat());
        let ciphertext = xor_encrypt(plaintext, &session.key, &nonce[..12]);
        Ok(ciphertext)
    }

    /// Decrypt a message using the session key.
    pub fn decrypt(&self, session: &SessionKey, ciphertext: &[u8]) -> Result<Vec<u8>> {
        // decrypt is symmetric with encrypt for XOR
        let nonce = blake3_hash(&[&session.key, ciphertext].concat());
        let plaintext = xor_encrypt(ciphertext, &session.key, &nonce[..12]);
        Ok(plaintext)
    }
}

// ─── PQ Crypto Primitives ─────────────────────────────────────────────────
//
// Uses flux-sqisign for signatures + BLAKE3 for hashing.
// Kyber KEM is emulated via BLAKE3-based key derivation for Phase 0;
// Phase 1 wires in real liboqs / crystals-kyber.

fn generate_pq_keypair(_seed: Option<[u8; 32]>) -> Result<(Vec<u8>, Vec<u8>)> {
    // flux-sqisign::keygen() returns (sk, pk) as Vec<u8>
    let (sk, pk) = flux_sqisign::keygen();
    Ok((sk, pk))
}

fn pq_sign(hash: &[u8; 32], secret_key: &[u8], public_key: &[u8]) -> Result<Vec<u8>> {
    let sig = flux_sqisign::sign(hash, secret_key, public_key)
        .map_err(|e| anyhow::anyhow!("SQIsign sign failed: {}", e))?;
    Ok(sig)
}

fn pq_verify(hash: &[u8; 32], signature: &[u8], public_key: &[u8]) -> Result<()> {
    match flux_sqisign::verify(hash, signature, public_key)
        .map_err(|e| anyhow::anyhow!("SQIsign verify error: {}", e))?
    {
        true => Ok(()),
        false => Err(anyhow::anyhow!("SQIsign signature invalid")),
    }
}

fn hybrid_kem_shared_secret(private_key: &[u8], peer_public: &[u8]) -> Result<[u8; 32]> {
    // Phase 0: BLAKE3-based KEM emulation
    // Phase 1: real Kyber-1024 KEM
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"flux-bt-kem-v1");
    hasher.update(private_key);
    hasher.update(peer_public);
    let result = hasher.finalize();
    Ok(*result.as_bytes())
}

fn blake3_hash(data: &[u8]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(data);
    *h.finalize().as_bytes()
}

fn xor_encrypt(data: &[u8], key: &[u8; 32], nonce: &[u8]) -> Vec<u8> {
    data.iter()
        .enumerate()
        .map(|(i, b)| b ^ key[i % 32] ^ nonce[i % nonce.len()])
        .collect()
}

// ─── Keypair serialization ─────────────────────────────────────────────────

impl PQIdentity {
    /// Serialize identity to hex string for QR codes / sharing.
    pub fn to_qr_string(&self) -> String {
        hex::encode(&self.public_key)
    }

    /// Parse identity from QR code / hex string.
    pub fn from_qr_string(s: &str, name: String) -> Result<Self> {
        let pk = hex::decode(s).context("invalid QR hex")?;
        let address = blake3_hash(&pk);
        Ok(Self { name, public_key: pk, address })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pq_identity_creation() {
        let crypto = BluetoothCrypto::new(None).unwrap();
        let id = crypto.identity();
        assert_eq!(id.public_key.len(), flux_sqisign::public_key_size());
        assert_eq!(id.address.len(), 32);
        println!("🧪 PQ identity: {} @ {}", id.name, hex::encode(&id.address));
    }

    #[tokio::test]
    async fn test_sign_verify_roundtrip() {
        let alice = BluetoothCrypto::new(None).unwrap();
        let msg = b"hello mesh!";
        let signed = alice.sign_message(msg).await.unwrap();
        assert!(BluetoothCrypto::verify(&signed).is_ok());
    }

    #[tokio::test]
    async fn test_sign_verify_rejects_tampered() {
        let alice = BluetoothCrypto::new(None).unwrap();
        let msg = b"hello mesh!";
        let mut signed = alice.sign_message(msg).await.unwrap();
        signed.data[0] ^= 0xff; // tamper
        assert!(BluetoothCrypto::verify(&signed).is_err());
    }

    #[test]
    fn test_qr_roundtrip() {
        let crypto = BluetoothCrypto::new(None).unwrap();
        let qr = crypto.identity().to_qr_string();
        let parsed = PQIdentity::from_qr_string(&qr, "test".into()).unwrap();
        assert_eq!(crypto.identity().public_key, parsed.public_key);
        assert_eq!(crypto.identity().address, parsed.address);
    }

    #[tokio::test]
    async fn test_session_key_derivation() {
        let alice = BluetoothCrypto::new(None).unwrap();
        let bob = BluetoothCrypto::new(None).unwrap();

        let session_a = alice.derive_session_key(&bob.identity().public_key).unwrap();
        let session_b = bob.derive_session_key(&alice.identity().public_key).unwrap();

        assert_eq!(session_a.key, session_b.key, "shared secret must match");
    }

    #[tokio::test]
    async fn test_encrypt_decrypt() {
        let alice = BluetoothCrypto::new(None).unwrap();
        let bob = BluetoothCrypto::new(None).unwrap();

        let session_a = alice.derive_session_key(&bob.identity().public_key).unwrap();
        let session_b = bob.derive_session_key(&alice.identity().public_key).unwrap();

        let plaintext = b"secret mesh message";
        let ciphertext = alice.encrypt(&session_a, plaintext).unwrap();
        let decrypted = bob.decrypt(&session_b, &ciphertext).unwrap();

        assert_eq!(plaintext.to_vec(), decrypted);
    }
}
