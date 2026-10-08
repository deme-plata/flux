use thiserror::Error;

pub type Result<T> = std::result::Result<T, IronTunnelError>;

#[derive(Error, Debug)]
pub enum IronTunnelError {
    #[error("Cryptographic operation failed: {0}")]
    Crypto(String),
    
    #[error("Zero-knowledge proof error: {0}")]
    ZkProof(String),
    
    #[error("Tor integration error: {0}")]
    Tor(String),
    
    #[error("Tunnel management error: {0}")]
    Tunnel(String),
    
    #[error("Network error: {0}")]
    Network(String),
    
    #[error("Configuration error: {0}")]
    Config(String),
    
    #[error("Authentication failed: {0}")]
    Auth(String),
    
    #[error("Transport error: {0}")]
    Transport(String),
    
    #[error("Memory protection error: {0}")]
    Memory(String),
    
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}