pub mod tunnel;
pub mod keys;
pub mod noise;

use crate::{Result, IronTunnelError};
// Mock WireGuard implementation for testing
use x25519_dalek::{EphemeralSecret, PublicKey as X25519PublicKey, StaticSecret};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use std::net::SocketAddr;
use std::sync::Arc;
use std::collections::HashMap;
use bytes::{Bytes, BytesMut};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireGuardConfig {
    pub private_key: String,
    pub public_key: String,
    pub listen_port: u16,
    pub peers: Vec<WireGuardPeer>,
    pub interface: WireGuardInterface,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireGuardPeer {
    pub public_key: String,
    pub endpoint: Option<SocketAddr>,
    pub allowed_ips: Vec<String>,
    pub persistent_keepalive: Option<u16>,
    pub preshared_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireGuardInterface {
    pub address: String,
    pub dns: Vec<String>,
    pub mtu: u16,
}

impl Default for WireGuardConfig {
    fn default() -> Self {
        Self {
            private_key: String::new(),
            public_key: String::new(),
            listen_port: 51820,
            peers: vec![],
            interface: WireGuardInterface {
                address: "10.0.0.2/24".to_string(),
                dns: vec!["1.1.1.1".to_string(), "8.8.8.8".to_string()],
                mtu: 1420,
            },
        }
    }
}

#[derive(Debug, Clone, Zeroize, ZeroizeOnDrop)]
pub struct WireGuardKeys {
    private_key: [u8; 32],
    public_key: [u8; 32],
}

impl WireGuardKeys {
    pub fn generate() -> Result<Self> {
        use rand::RngCore;
        let mut private_key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut private_key);
        
        let secret = StaticSecret::from(private_key);
        let public_key = X25519PublicKey::from(&secret);
        
        Ok(Self {
            private_key,
            public_key: *public_key.as_bytes(),
        })
    }
    
    pub fn from_private_key(private_key_str: &str) -> Result<Self> {
        let private_key_bytes = base64::decode(private_key_str)
            .map_err(|e| IronTunnelError::Config(format!("Invalid private key: {}", e)))?;
        
        if private_key_bytes.len() != 32 {
            return Err(IronTunnelError::Config("Private key must be 32 bytes".to_string()));
        }
        
        let mut private_key = [0u8; 32];
        private_key.copy_from_slice(&private_key_bytes);
        
        let secret = StaticSecret::from(private_key);
        let public_key = X25519PublicKey::from(&secret);
        
        Ok(Self {
            private_key,
            public_key: *public_key.as_bytes(),
        })
    }
    
    pub fn private_key_base64(&self) -> String {
        base64::encode(&self.private_key)
    }
    
    pub fn public_key_base64(&self) -> String {
        base64::encode(&self.public_key)
    }
    
    pub fn private_key(&self) -> &[u8; 32] {
        &self.private_key
    }
    
    pub fn public_key(&self) -> &[u8; 32] {
        &self.public_key
    }
}

pub struct WireGuardTunnel {
    config: WireGuardConfig,
    keys: WireGuardKeys,
    socket: Arc<UdpSocket>,
    packet_tx: mpsc::UnboundedSender<WireGuardPacket>,
    packet_rx: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<WireGuardPacket>>>,
}

#[derive(Debug, Clone)]
pub struct WireGuardPacket {
    pub data: Bytes,
    pub peer: SocketAddr,
    pub is_incoming: bool,
}

impl WireGuardTunnel {
    pub async fn new(config: WireGuardConfig) -> Result<Self> {
        let keys = if config.private_key.is_empty() {
            WireGuardKeys::generate()?
        } else {
            WireGuardKeys::from_private_key(&config.private_key)?
        };
        
        let socket = UdpSocket::bind(format!("0.0.0.0:{}", config.listen_port)).await
            .map_err(|e| IronTunnelError::Network(format!("Failed to bind UDP socket: {}", e)))?;
        
        let (packet_tx, packet_rx) = mpsc::unbounded_channel();
        
        Ok(Self {
            config,
            keys,
            socket: Arc::new(socket),
            packet_tx,
            packet_rx: Arc::new(tokio::sync::Mutex::new(packet_rx)),
        })
    }
    
    pub async fn start(&mut self) -> Result<()> {
        self.setup_peers().await?;
        self.start_packet_handler().await?;
        self.start_keepalive_handler().await?;
        
        Ok(())
    }
    
    pub async fn send_packet(&mut self, data: &[u8], peer: SocketAddr) -> Result<()> {
        // Mock WireGuard packet sending
        self.socket.send_to(data, peer).await
            .map_err(|e| IronTunnelError::Network(format!("Failed to send packet: {}", e)))?;
        
        Ok(())
    }
    
    pub async fn receive_packet(&mut self) -> Result<Option<WireGuardPacket>> {
        let mut packet_rx = self.packet_rx.lock().await;
        Ok(packet_rx.recv().await)
    }
    
    pub fn get_config(&self) -> &WireGuardConfig {
        &self.config
    }
    
    pub fn get_public_key(&self) -> String {
        self.keys.public_key_base64()
    }
    
    async fn setup_peers(&mut self) -> Result<()> {
        // Mock peer setup for testing
        for peer in &self.config.peers {
            let _peer_public_key = base64::decode(&peer.public_key)
                .map_err(|e| IronTunnelError::Config(format!("Invalid peer public key: {}", e)))?;
            
            tracing::debug!("Setting up peer with endpoint: {:?}", peer.endpoint);
        }
        
        Ok(())
    }
    
    
    async fn start_packet_handler(&self) -> Result<()> {
        let socket = self.socket.clone();
        let packet_tx = self.packet_tx.clone();
        
        tokio::spawn(async move {
            let mut buffer = [0u8; 1500];
            
            loop {
                match socket.recv_from(&mut buffer).await {
                    Ok((size, peer)) => {
                        let packet = WireGuardPacket {
                            data: Bytes::copy_from_slice(&buffer[..size]),
                            peer,
                            is_incoming: true,
                        };
                        
                        if let Err(e) = packet_tx.send(packet) {
                            tracing::error!("Failed to send packet to handler: {}", e);
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::error!("Failed to receive UDP packet: {}", e);
                        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                    }
                }
            }
        });
        
        Ok(())
    }
    
    async fn start_keepalive_handler(&self) -> Result<()> {
        let socket = self.socket.clone();
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(25));
            
            loop {
                interval.tick().await;
                // TODO: Implement keepalive logic
            }
        });
        
        Ok(())
    }
}

pub fn generate_keypair() -> Result<(String, String)> {
    let keys = WireGuardKeys::generate()?;
    Ok((keys.private_key_base64(), keys.public_key_base64()))
}

pub fn public_key_from_private(private_key: &str) -> Result<String> {
    let keys = WireGuardKeys::from_private_key(private_key)?;
    Ok(keys.public_key_base64())
}