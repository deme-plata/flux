pub mod client;
pub mod hidden_service;
pub mod proxy;

use crate::{Result, IronTunnelError, TorMode};
use tokio::net::{TcpStream, TcpListener};
use tokio::io::{AsyncRead, AsyncWrite};
use std::net::SocketAddr;
use std::sync::Arc;
// Mock Tor implementation for testing
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TorConfig {
    pub socks_port: u16,
    pub control_port: u16,
    pub data_directory: String,
    pub bridge_lines: Vec<String>,
    pub use_bridges: bool,
    pub enable_onion_services: bool,
    pub onion_service_port: u16,
    pub onion_service_dir: String,
}

impl Default for TorConfig {
    fn default() -> Self {
        Self {
            socks_port: 9050,
            control_port: 9051,
            data_directory: "/tmp/irontunnel_tor".to_string(),
            bridge_lines: vec![],
            use_bridges: false,
            enable_onion_services: false,
            onion_service_port: 1194,
            onion_service_dir: "/tmp/irontunnel_hs".to_string(),
        }
    }
}

pub struct TorManager {
    config: TorConfig,
    hidden_service: Option<HiddenService>,
    mode: TorMode,
}

impl TorManager {
    pub fn new() -> Result<Self> {
        let config = TorConfig::default();
        Ok(Self {
            config,
            hidden_service: None,
            mode: TorMode::Direct,
        })
    }
    
    pub fn with_config(config: TorConfig, mode: TorMode) -> Result<Self> {
        Ok(Self {
            config,
            hidden_service: None,
            mode,
        })
    }
    
    pub async fn initialize(&mut self) -> Result<()> {
        match self.mode {
            TorMode::Direct => {
            },
            TorMode::OverTor | TorMode::TorOverVpn | TorMode::Hybrid => {
                self.start_tor_client().await?;
                
                if self.config.enable_onion_services {
                    self.start_hidden_service().await?;
                }
            }
        }
        Ok(())
    }
    
    pub async fn connect_through_tor(&self, target: &str, port: u16) -> Result<TorStream> {
        // Mock Tor connection for testing
        let stream = TcpStream::connect(format!("{}:{}", target, port)).await
            .map_err(|e| IronTunnelError::Tor(format!("Failed to connect through Tor: {}", e)))?;
        
        Ok(TorStream::new(stream))
    }
    
    pub async fn create_onion_service(&mut self, local_port: u16) -> Result<String> {
        if !self.config.enable_onion_services {
            return Err(IronTunnelError::Tor("Onion services not enabled".to_string()));
        }
        
        let hs = HiddenService::new(&self.config.onion_service_dir, local_port).await?;
        let onion_address = hs.get_onion_address().await?;
        self.hidden_service = Some(hs);
        
        Ok(onion_address)
    }
    
    pub fn get_socks_proxy_addr(&self) -> SocketAddr {
        format!("127.0.0.1:{}", self.config.socks_port).parse().unwrap()
    }
    
    pub async fn is_tor_running(&self) -> bool {
        // Mock implementation
        false
    }
    
    async fn start_tor_client(&mut self) -> Result<()> {
        // Mock Tor client startup
        tracing::info!("Mock Tor client started");
        Ok(())
    }
    
    async fn start_hidden_service(&mut self) -> Result<()> {
        let hs = HiddenService::new(&self.config.onion_service_dir, self.config.onion_service_port).await?;
        self.hidden_service = Some(hs);
        Ok(())
    }
}

pub struct TorStream {
    inner: TcpStream,
}

impl TorStream {
    fn new(stream: TcpStream) -> Self {
        Self {
            inner: stream,
        }
    }
    
    pub async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        use tokio::io::AsyncReadExt;
        self.inner.read(buf).await
            .map_err(|e| IronTunnelError::Tor(format!("Tor stream read error: {}", e)))
    }
    
    pub async fn write(&mut self, buf: &[u8]) -> Result<usize> {
        use tokio::io::AsyncWriteExt;
        self.inner.write(buf).await
            .map_err(|e| IronTunnelError::Tor(format!("Tor stream write error: {}", e)))
    }
    
    pub async fn flush(&mut self) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        self.inner.flush().await
            .map_err(|e| IronTunnelError::Tor(format!("Tor stream flush error: {}", e)))
    }
}


pub struct HiddenService {
    service_dir: String,
    local_port: u16,
    onion_address: Option<String>,
}

impl HiddenService {
    pub async fn new(service_dir: &str, local_port: u16) -> Result<Self> {
        tokio::fs::create_dir_all(service_dir).await
            .map_err(|e| IronTunnelError::Tor(format!("Failed to create service directory: {}", e)))?;
        
        Ok(Self {
            service_dir: service_dir.to_string(),
            local_port,
            onion_address: None,
        })
    }
    
    pub async fn get_onion_address(&mut self) -> Result<String> {
        if let Some(ref address) = self.onion_address {
            return Ok(address.clone());
        }
        
        let hostname_file = format!("{}/hostname", self.service_dir);
        if tokio::fs::metadata(&hostname_file).await.is_ok() {
            let address = tokio::fs::read_to_string(&hostname_file).await
                .map_err(|e| IronTunnelError::Tor(format!("Failed to read hostname file: {}", e)))?
                .trim().to_string();
            self.onion_address = Some(address.clone());
            return Ok(address);
        }
        
        let generated_address = self.generate_onion_address().await?;
        self.onion_address = Some(generated_address.clone());
        Ok(generated_address)
    }
    
    async fn generate_onion_address(&self) -> Result<String> {
        use rand::RngCore;
        let mut rng = rand::thread_rng();
        let mut key_bytes = [0u8; 32];
        rng.fill_bytes(&mut key_bytes);
        
        let address = format!("{}.onion", 
            base64::encode_config(&key_bytes[..16], base64::URL_SAFE_NO_PAD));
        
        let hostname_file = format!("{}/hostname", self.service_dir);
        tokio::fs::write(&hostname_file, &address).await
            .map_err(|e| IronTunnelError::Tor(format!("Failed to write hostname file: {}", e)))?;
        
        Ok(address)
    }
}