use thiserror::Error;

pub type Result<T> = std::result::Result<T, IronTunnelError>;

#[derive(Error, Debug)]
pub enum IronTunnelError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("Cryptographic error: {0}")]
    Crypto(String),

    #[error("Zero-knowledge proof error: {0}")]
    ZkProof(String),

    #[error("Tor error: {0}")]
    Tor(String),

    #[error("Tunnel error: {0}")]
    Tunnel(String),

    #[error("DNS error: {0}")]
    Dns(String),

    #[error("Authentication error: {0}")]
    Auth(String),

    #[error("Transport error: {0}")]
    Transport(String),

    #[error("WireGuard error: {0}")]
    WireGuard(String),

    #[error("Multi-hop error: {0}")]
    MultiHop(String),

    #[error("Split tunnel error: {0}")]
    SplitTunnel(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Internal error: {0}")]
    Internal(String),
    
    #[error("API error: {0}")]
    ApiError(String),
    
    #[error("Network error: {0}")]
    NetworkError(String),
    
    #[error("Parse error: {0}")]
    ParseError(String),
    
    #[error("Auth error: {0}")]
    AuthError(String),
}

impl IronTunnelError {
    pub fn is_recoverable(&self) -> bool {
        match self {
            IronTunnelError::Network(_) |
            IronTunnelError::NetworkError(_) |
            IronTunnelError::Dns(_) |
            IronTunnelError::Tor(_) |
            IronTunnelError::ApiError(_) => true,
            _ => false,
        }
    }

    pub fn error_code(&self) -> u32 {
        match self {
            IronTunnelError::Config(_) => 1000,
            IronTunnelError::Network(_) => 2000,
            IronTunnelError::Crypto(_) => 3000,
            IronTunnelError::ZkProof(_) => 4000,
            IronTunnelError::Tor(_) => 5000,
            IronTunnelError::Tunnel(_) => 6000,
            IronTunnelError::Dns(_) => 7000,
            IronTunnelError::Auth(_) => 8000,
            IronTunnelError::Transport(_) => 9000,
            IronTunnelError::WireGuard(_) => 10000,
            IronTunnelError::MultiHop(_) => 11000,
            IronTunnelError::SplitTunnel(_) => 12000,
            IronTunnelError::Io(_) => 13000,
            IronTunnelError::Serialization(_) => 14000,
            IronTunnelError::Parse(_) => 15000,
            IronTunnelError::Internal(_) => 16000,
            IronTunnelError::ApiError(_) => 17000,
            IronTunnelError::NetworkError(_) => 18000,
            IronTunnelError::ParseError(_) => 19000,
            IronTunnelError::AuthError(_) => 20000,
        }
    }
}