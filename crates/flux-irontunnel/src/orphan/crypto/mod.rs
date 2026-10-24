pub mod engine;
pub mod session;
pub mod keys;

use crate::{Result, IronTunnelError};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce, aead::{Aead, NewAead}};
use x25519_dalek::{EphemeralSecret, PublicKey, SharedSecret};
use ed25519_dalek::{Keypair, PublicKey as Ed25519PublicKey, Signature, Signer, Verifier};
use blake3::Hasher;
use ring::aead::{self, LessSafeKey, UnboundKey};
use zeroize::{Zeroize, ZeroizeOnDrop};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct SessionKey {
    key: [u8; 32],
    nonce_counter: u64,
    created_at: u64,
    expires_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyExchangeMessage {
    pub public_key: [u8; 32],
    pub signature: [u8; 64],
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedPacket {
    pub ciphertext: Vec<u8>,
    pub nonce: [u8; 12],
    pub tag: [u8; 16],
}

pub struct CryptoEngine {
    signing_key: Keypair,
    current_session: Option<SessionKey>,
    peer_public_key: Option<Ed25519PublicKey>,
}

impl CryptoEngine {
    pub fn new() -> Result<Self> {
        let mut csprng = rand::rngs::OsRng;
        let signing_key = Keypair::generate(&mut csprng);
        
        Ok(Self {
            signing_key,
            current_session: None,
            peer_public_key: None,
        })
    }
    
    pub fn generate_ephemeral_keypair() -> (EphemeralSecret, PublicKey) {
        let secret = EphemeralSecret::new(rand::rngs::OsRng);
        let public = PublicKey::from(&secret);
        (secret, public)
    }
    
    pub fn perform_key_exchange(&mut self, peer_public: &[u8; 32]) -> Result<KeyExchangeMessage> {
        let (ephemeral_secret, ephemeral_public) = Self::generate_ephemeral_keypair();
        let peer_public_key = PublicKey::from(*peer_public);
        let shared_secret = ephemeral_secret.diffie_hellman(&peer_public_key);
        
        let session_key = self.derive_session_key(shared_secret.as_bytes())?;
        self.current_session = Some(session_key);
        
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        let message_to_sign = self.create_key_exchange_message(ephemeral_public.as_bytes(), timestamp)?;
        let signature = self.signing_key.sign(&message_to_sign);
        
        Ok(KeyExchangeMessage {
            public_key: *ephemeral_public.as_bytes(),
            signature: signature.to_bytes(),
            timestamp,
        })
    }
    
    pub fn verify_key_exchange(&self, message: &KeyExchangeMessage, peer_public_key: &Ed25519PublicKey) -> Result<bool> {
        let current_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        if current_time - message.timestamp > 300 {
            return Ok(false);
        }
        
        let message_to_verify = self.create_key_exchange_message(&message.public_key, message.timestamp)?;
        let signature = Signature::from_bytes(&message.signature)
            .map_err(|_| IronTunnelError::Crypto("Invalid signature format".to_string()))?;
        
        Ok(peer_public_key.verify(&message_to_verify, &signature).is_ok())
    }
    
    pub fn encrypt_packet(&mut self, plaintext: &[u8]) -> Result<EncryptedPacket> {
        let session = self.current_session.as_mut()
            .ok_or_else(|| IronTunnelError::Crypto("No active session".to_string()))?;
        
        if self.is_session_expired(session) {
            return Err(IronTunnelError::Crypto("Session expired".to_string()));
        }
        
        let key = Key::from_slice(&session.key);
        let cipher = ChaCha20Poly1305::new(key);
        
        let nonce_bytes = self.generate_nonce(session)?;
        let nonce = Nonce::from_slice(&nonce_bytes);
        
        let ciphertext = cipher.encrypt(nonce, plaintext)
            .map_err(|_| IronTunnelError::Crypto("Encryption failed".to_string()))?;
        
        session.nonce_counter += 1;
        
        Ok(EncryptedPacket {
            ciphertext: ciphertext[..ciphertext.len() - 16].to_vec(),
            nonce: nonce_bytes,
            tag: ciphertext[ciphertext.len() - 16..].try_into().unwrap(),
        })
    }
    
    pub fn decrypt_packet(&mut self, packet: &EncryptedPacket) -> Result<Vec<u8>> {
        let session = self.current_session.as_ref()
            .ok_or_else(|| IronTunnelError::Crypto("No active session".to_string()))?;
        
        if self.is_session_expired(session) {
            return Err(IronTunnelError::Crypto("Session expired".to_string()));
        }
        
        let key = Key::from_slice(&session.key);
        let cipher = ChaCha20Poly1305::new(key);
        let nonce = Nonce::from_slice(&packet.nonce);
        
        let mut ciphertext_with_tag = packet.ciphertext.clone();
        ciphertext_with_tag.extend_from_slice(&packet.tag);
        
        let plaintext = cipher.decrypt(nonce, ciphertext_with_tag.as_slice())
            .map_err(|_| IronTunnelError::Crypto("Decryption failed".to_string()))?;
        
        Ok(plaintext)
    }
    
    pub fn rotate_session_key(&mut self) -> Result<()> {
        if let Some(ref mut session) = self.current_session {
            let new_key_material = self.generate_key_material()?;
            session.key.copy_from_slice(&new_key_material);
            session.created_at = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            session.expires_at = session.created_at + 3600;
            session.nonce_counter = 0;
        }
        Ok(())
    }
    
    pub fn get_public_key(&self) -> Ed25519PublicKey {
        self.signing_key.public
    }
    
    fn derive_session_key(&self, shared_secret: &[u8]) -> Result<SessionKey> {
        let mut hasher = Hasher::new();
        hasher.update(shared_secret);
        hasher.update(b"IRONTUNNEL_SESSION_KEY");
        let hash = hasher.finalize();
        
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        Ok(SessionKey {
            key: *hash.as_bytes(),
            nonce_counter: 0,
            created_at,
            expires_at: created_at + 3600,
        })
    }
    
    fn create_key_exchange_message(&self, public_key: &[u8; 32], timestamp: u64) -> Result<Vec<u8>> {
        let mut message = Vec::new();
        message.extend_from_slice(public_key);
        message.extend_from_slice(&timestamp.to_le_bytes());
        message.extend_from_slice(b"IRONTUNNEL_KEY_EXCHANGE");
        Ok(message)
    }
    
    fn generate_nonce(&self, session: &SessionKey) -> Result<[u8; 12]> {
        let mut nonce = [0u8; 12];
        nonce[..8].copy_from_slice(&session.nonce_counter.to_le_bytes());
        nonce[8..].copy_from_slice(&session.created_at.to_le_bytes()[..4]);
        Ok(nonce)
    }
    
    fn is_session_expired(&self, session: &SessionKey) -> bool {
        let current_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        current_time > session.expires_at
    }
    
    fn generate_key_material(&self) -> Result<[u8; 32]> {
        let mut key_material = [0u8; 32];
        use rand::RngCore;
        rand::thread_rng().fill_bytes(&mut key_material);
        Ok(key_material)
    }
}

impl Drop for CryptoEngine {
    fn drop(&mut self) {
        if let Some(ref mut session) = self.current_session {
            session.zeroize();
        }
    }
}

pub fn secure_compare(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    
    let mut result = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        result |= x ^ y;
    }
    result == 0
}

pub fn hash_password(password: &[u8], salt: &[u8]) -> [u8; 32] {
    let mut hasher = Hasher::new();
    hasher.update(password);
    hasher.update(salt);
    hasher.update(b"IRONTUNNEL_PASSWORD_HASH");
    *hasher.finalize().as_bytes()
}