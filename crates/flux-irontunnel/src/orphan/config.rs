use serde::{Deserialize, Serialize};
use std::path::Path;
use crate::{Result, IronTunnelError, TorMode, Transport, AuthMethod};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub listen: String,
    pub port: u16,
    pub mode: String,
    pub max_clients: usize,
    pub authentication: AuthConfig,
    pub tor: Option<TorConfig>,
    pub security: SecurityConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub zk_params: String,
    pub allow_anonymous: bool,
    pub session_timeout: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TorConfig {
    pub enable_hidden_service: bool,
    pub hs_port: u16,
    pub hs_dir: String,
    pub control_port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    pub enable_kill_switch: bool,
    pub key_rotation_interval: u64,
    pub max_connection_time: u64,
    pub enable_pfs: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen: "0.0.0.0".to_string(),
            port: 1194,
            mode: "hybrid".to_string(),
            max_clients: 100,
            authentication: AuthConfig {
                zk_params: "/etc/irontunnel/zk_params.bin".to_string(),
                allow_anonymous: false,
                session_timeout: 3600,
            },
            tor: Some(TorConfig {
                enable_hidden_service: true,
                hs_port: 1194,
                hs_dir: "/var/lib/tor/irontunnel/".to_string(),
                control_port: 9051,
            }),
            security: SecurityConfig {
                enable_kill_switch: true,
                key_rotation_interval: 3600,
                max_connection_time: 86400,
                enable_pfs: true,
            },
        }
    }
}

impl ServerConfig {
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| IronTunnelError::Config(format!("Failed to read config file: {}", e)))?;
        
        toml::from_str(&content)
            .map_err(|e| IronTunnelError::Config(format!("Failed to parse config: {}", e)))
    }
    
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let content = toml::to_string_pretty(self)
            .map_err(|e| IronTunnelError::Config(format!("Failed to serialize config: {}", e)))?;
        
        std::fs::write(path, content)
            .map_err(|e| IronTunnelError::Config(format!("Failed to write config file: {}", e)))?;
        
        Ok(())
    }
}