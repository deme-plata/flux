// Mock OBFS4 implementation for testing
use crate::{Result, IronTunnelError};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce, Aead, NewAead};

pub struct Obfs4Obfuscator {
    key: [u8; 32],
    nonce_counter: u64,
}

impl Obfs4Obfuscator {
    pub fn new(cert: Option<&str>, _iat_mode: u32) -> Result<Self> {
        let key = if let Some(cert_str) = cert {
            Self::derive_key_from_cert(cert_str)?
        } else {
            Self::generate_random_key()?
        };
        
        Ok(Self {
            key,
            nonce_counter: 0,
        })
    }
    
    pub fn obfuscate(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        let key = Key::from_slice(&self.key);
        let cipher = ChaCha20Poly1305::new(key);
        
        let mut nonce_bytes = [0u8; 12];
        nonce_bytes[..8].copy_from_slice(&self.nonce_counter.to_le_bytes());
        let nonce = Nonce::from_slice(&nonce_bytes);
        
        self.nonce_counter += 1;
        
        let ciphertext = cipher.encrypt(nonce, data)
            .map_err(|_| IronTunnelError::Transport("Obfuscation failed".to_string()))?;
        
        let mut result = Vec::with_capacity(ciphertext.len() + 12);
        result.extend_from_slice(&nonce_bytes);
        result.extend_from_slice(&ciphertext);
        
        Ok(result)
    }
    
    pub fn deobfuscate(&self, data: &[u8]) -> Result<Vec<u8>> {
        if data.len() < 28 {
            return Err(IronTunnelError::Transport("Data too short for deobfuscation".to_string()));
        }
        
        let key = Key::from_slice(&self.key);
        let cipher = ChaCha20Poly1305::new(key);
        
        let nonce = Nonce::from_slice(&data[..12]);
        let ciphertext = &data[12..];
        
        let plaintext = cipher.decrypt(nonce, ciphertext)
            .map_err(|_| IronTunnelError::Transport("Deobfuscation failed".to_string()))?;
        
        Ok(plaintext)
    }
    
    fn derive_key_from_cert(cert: &str) -> Result<[u8; 32]> {
        let mut hasher = blake3::Hasher::new();
        hasher.update(cert.as_bytes());
        hasher.update(b"IRONTUNNEL_OBFS4_KEY");
        Ok(*hasher.finalize().as_bytes())
    }
    
    fn generate_random_key() -> Result<[u8; 32]> {
        let mut key = [0u8; 32];
        use rand::RngCore;
        rand::thread_rng().fill_bytes(&mut key);
        Ok(key)
    }
}