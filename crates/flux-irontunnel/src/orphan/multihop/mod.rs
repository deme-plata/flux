pub mod chain;
pub mod routing;
pub mod manager;

use crate::{Result, IronTunnelError, TorMode, Transport};
use tokio::net::TcpStream;
use std::net::{SocketAddr, IpAddr};
use std::sync::Arc;
use std::collections::VecDeque;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiHopConfig {
    pub enabled: bool,
    pub max_hops: usize,
    pub hop_selection_strategy: HopSelectionStrategy,
    pub entry_countries: Vec<String>,
    pub exit_countries: Vec<String>,
    pub avoid_countries: Vec<String>,
    pub min_latency_ms: Option<u64>,
    pub max_latency_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HopSelectionStrategy {
    Random,
    LatencyOptimized,
    GeographicDiversity,
    SecurityFocused,
    Custom(Vec<String>), // Specific server IDs
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VpnServer {
    pub id: String,
    pub name: String,
    pub country: String,
    pub city: String,
    pub ip_address: IpAddr,
    pub port: u16,
    pub load_percentage: f32,
    pub latency_ms: Option<u64>,
    pub features: ServerFeatures,
    pub protocols: Vec<VpnProtocol>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerFeatures {
    pub supports_p2p: bool,
    pub supports_streaming: bool,
    pub supports_tor: bool,
    pub double_vpn_capable: bool,
    pub dedicated_ip: bool,
    pub obfuscated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VpnProtocol {
    OpenVpn,
    WireGuard,
    IKEv2,
    IronTunnel,
}

#[derive(Debug, Clone)]
pub struct HopChain {
    pub id: Uuid,
    pub hops: VecDeque<VpnServer>,
    pub created_at: std::time::Instant,
    pub total_latency_ms: u64,
    pub security_score: f32,
    pub connections: Vec<Option<TcpStream>>,
}

impl Default for MultiHopConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_hops: 3,
            hop_selection_strategy: HopSelectionStrategy::SecurityFocused,
            entry_countries: vec![],
            exit_countries: vec![],
            avoid_countries: vec!["CN".to_string(), "RU".to_string(), "IR".to_string()],
            min_latency_ms: None,
            max_latency_ms: Some(500),
        }
    }
}

pub struct MultiHopManager {
    config: MultiHopConfig,
    available_servers: Vec<VpnServer>,
    active_chains: Vec<HopChain>,
    current_chain: Option<HopChain>,
}

impl MultiHopManager {
    pub fn new(config: MultiHopConfig) -> Self {
        Self {
            config,
            available_servers: vec![],
            active_chains: vec![],
            current_chain: None,
        }
    }
    
    pub async fn load_servers(&mut self) -> Result<()> {
        self.available_servers = self.fetch_server_list().await?;
        self.update_server_metrics().await?;
        Ok(())
    }
    
    pub async fn create_hop_chain(&mut self) -> Result<HopChain> {
        if !self.config.enabled {
            return Err(IronTunnelError::Config("Multi-hop is disabled".to_string()));
        }
        
        let servers = self.select_servers_for_chain().await?;
        let chain_id = Uuid::new_v4();
        
        let total_latency = servers.iter()
            .map(|s| s.latency_ms.unwrap_or(100))
            .sum();
        
        let security_score = self.calculate_security_score(&servers);
        
        let mut chain = HopChain {
            id: chain_id,
            hops: servers.into(),
            created_at: std::time::Instant::now(),
            total_latency_ms: total_latency,
            security_score,
            connections: vec![],
        };
        
        chain.connections.resize(chain.hops.len(), None);
        
        self.active_chains.push(chain.clone());
        Ok(chain)
    }
    
    pub async fn establish_chain(&mut self, chain: &mut HopChain) -> Result<()> {
        tracing::info!("Establishing multi-hop chain with {} hops", chain.hops.len());
        
        for (i, server) in chain.hops.iter().enumerate() {
            tracing::debug!("Connecting to hop {}: {} ({})", i + 1, server.name, server.country);
            
            let connection = self.connect_to_server(server, i == 0).await?;
            chain.connections[i] = Some(connection);
            
            if i > 0 {
                self.setup_hop_forwarding(chain, i).await?;
            }
        }
        
        self.current_chain = Some(chain.clone());
        tracing::info!("Multi-hop chain established successfully");
        
        Ok(())
    }
    
    pub async fn send_through_chain(&self, data: &[u8]) -> Result<()> {
        let chain = self.current_chain.as_ref()
            .ok_or_else(|| IronTunnelError::Network("No active hop chain".to_string()))?;
        
        if let Some(Some(ref mut connection)) = chain.connections.first() {
            use tokio::io::AsyncWriteExt;
            connection.write_all(data).await
                .map_err(|e| IronTunnelError::Network(format!("Failed to send through chain: {}", e)))?;
        }
        
        Ok(())
    }
    
    pub async fn rotate_exit_node(&mut self) -> Result<()> {
        if let Some(ref mut chain) = self.current_chain {
            let new_exit = self.select_exit_server().await?;
            
            if let Some(old_connection) = chain.connections.last_mut() {
                if let Some(stream) = old_connection.take() {
                    use tokio::io::AsyncWriteExt;
                    let _ = stream.shutdown().await;
                }
            }
            
            let new_connection = self.connect_to_server(&new_exit, false).await?;
            *chain.hops.back_mut().unwrap() = new_exit;
            *chain.connections.last_mut().unwrap() = Some(new_connection);
            
            self.setup_hop_forwarding(chain, chain.hops.len() - 1).await?;
            
            tracing::info!("Exit node rotated successfully");
        }
        
        Ok(())
    }
    
    pub fn get_chain_info(&self) -> Option<ChainInfo> {
        self.current_chain.as_ref().map(|chain| ChainInfo {
            id: chain.id,
            hop_count: chain.hops.len(),
            total_latency_ms: chain.total_latency_ms,
            security_score: chain.security_score,
            countries: chain.hops.iter().map(|h| h.country.clone()).collect(),
            established_at: chain.created_at,
        })
    }
    
    async fn fetch_server_list(&self) -> Result<Vec<VpnServer>> {
        // Mock server data - in production this would fetch from API
        Ok(vec![
            VpnServer {
                id: "us-east-1".to_string(),
                name: "New York #1".to_string(),
                country: "US".to_string(),
                city: "New York".to_string(),
                ip_address: "198.51.100.1".parse().unwrap(),
                port: 1194,
                load_percentage: 45.2,
                latency_ms: Some(25),
                features: ServerFeatures {
                    supports_p2p: true,
                    supports_streaming: true,
                    supports_tor: true,
                    double_vpn_capable: true,
                    dedicated_ip: false,
                    obfuscated: false,
                },
                protocols: vec![VpnProtocol::OpenVpn, VpnProtocol::WireGuard, VpnProtocol::IronTunnel],
            },
            VpnServer {
                id: "de-fra-1".to_string(),
                name: "Frankfurt #1".to_string(),
                country: "DE".to_string(),
                city: "Frankfurt".to_string(),
                ip_address: "203.0.113.1".parse().unwrap(),
                port: 1194,
                load_percentage: 32.1,
                latency_ms: Some(45),
                features: ServerFeatures {
                    supports_p2p: true,
                    supports_streaming: false,
                    supports_tor: true,
                    double_vpn_capable: true,
                    dedicated_ip: false,
                    obfuscated: true,
                },
                protocols: vec![VpnProtocol::OpenVpn, VpnProtocol::WireGuard, VpnProtocol::IronTunnel],
            },
            VpnServer {
                id: "jp-tok-1".to_string(),
                name: "Tokyo #1".to_string(),
                country: "JP".to_string(),
                city: "Tokyo".to_string(),
                ip_address: "192.0.2.1".parse().unwrap(),
                port: 1194,
                load_percentage: 28.7,
                latency_ms: Some(120),
                features: ServerFeatures {
                    supports_p2p: false,
                    supports_streaming: true,
                    supports_tor: true,
                    double_vpn_capable: true,
                    dedicated_ip: true,
                    obfuscated: false,
                },
                protocols: vec![VpnProtocol::OpenVpn, VpnProtocol::WireGuard],
            },
        ])
    }
    
    async fn update_server_metrics(&mut self) -> Result<()> {
        for server in &mut self.available_servers {
            server.latency_ms = self.measure_latency(&server.ip_address).await.ok();
            server.load_percentage = self.get_server_load(&server.id).await.unwrap_or(50.0);
        }
        Ok(())
    }
    
    async fn select_servers_for_chain(&self) -> Result<Vec<VpnServer>> {
        let hop_count = std::cmp::min(self.config.max_hops, 3);
        let mut selected = Vec::new();
        
        match &self.config.hop_selection_strategy {
            HopSelectionStrategy::Random => {
                selected = self.select_random_servers(hop_count);
            }
            HopSelectionStrategy::LatencyOptimized => {
                selected = self.select_low_latency_servers(hop_count);
            }
            HopSelectionStrategy::GeographicDiversity => {
                selected = self.select_geographically_diverse_servers(hop_count);
            }
            HopSelectionStrategy::SecurityFocused => {
                selected = self.select_security_focused_servers(hop_count);
            }
            HopSelectionStrategy::Custom(server_ids) => {
                selected = self.select_custom_servers(server_ids);
            }
        }
        
        if selected.len() < 2 {
            return Err(IronTunnelError::Config("Insufficient servers for multi-hop".to_string()));
        }
        
        Ok(selected)
    }
    
    fn select_random_servers(&self, count: usize) -> Vec<VpnServer> {
        use rand::seq::SliceRandom;
        let mut rng = rand::thread_rng();
        
        let mut servers = self.available_servers.clone();
        servers.shuffle(&mut rng);
        servers.into_iter().take(count).collect()
    }
    
    fn select_low_latency_servers(&self, count: usize) -> Vec<VpnServer> {
        let mut servers = self.available_servers.clone();
        servers.sort_by_key(|s| s.latency_ms.unwrap_or(1000));
        servers.into_iter().take(count).collect()
    }
    
    fn select_geographically_diverse_servers(&self, count: usize) -> Vec<VpnServer> {
        let mut selected = Vec::new();
        let mut used_countries = std::collections::HashSet::new();
        
        for server in &self.available_servers {
            if !used_countries.contains(&server.country) && selected.len() < count {
                selected.push(server.clone());
                used_countries.insert(server.country.clone());
            }
        }
        
        // Fill remaining slots if needed
        for server in &self.available_servers {
            if selected.len() >= count {
                break;
            }
            if !selected.iter().any(|s| s.id == server.id) {
                selected.push(server.clone());
            }
        }
        
        selected
    }
    
    fn select_security_focused_servers(&self, count: usize) -> Vec<VpnServer> {
        let mut servers: Vec<_> = self.available_servers.iter()
            .filter(|s| !self.config.avoid_countries.contains(&s.country))
            .filter(|s| s.features.double_vpn_capable)
            .cloned()
            .collect();
        
        servers.sort_by(|a, b| {
            let score_a = self.calculate_server_security_score(a);
            let score_b = self.calculate_server_security_score(b);
            score_b.partial_cmp(&score_a).unwrap_or(std::cmp::Ordering::Equal)
        });
        
        servers.into_iter().take(count).collect()
    }
    
    fn select_custom_servers(&self, server_ids: &[String]) -> Vec<VpnServer> {
        server_ids.iter()
            .filter_map(|id| self.available_servers.iter().find(|s| &s.id == id))
            .cloned()
            .collect()
    }
    
    async fn connect_to_server(&self, server: &VpnServer, is_entry: bool) -> Result<TcpStream> {
        let address = SocketAddr::new(server.ip_address, server.port);
        
        let stream = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            TcpStream::connect(address)
        ).await
        .map_err(|_| IronTunnelError::Network("Connection timeout".to_string()))?
        .map_err(|e| IronTunnelError::Network(format!("Connection failed: {}", e)))?;
        
        Ok(stream)
    }
    
    async fn setup_hop_forwarding(&self, _chain: &HopChain, _hop_index: usize) -> Result<()> {
        // TODO: Implement hop forwarding logic
        Ok(())
    }
    
    async fn select_exit_server(&self) -> Result<VpnServer> {
        let exit_servers: Vec<_> = self.available_servers.iter()
            .filter(|s| {
                if !self.config.exit_countries.is_empty() {
                    self.config.exit_countries.contains(&s.country)
                } else {
                    true
                }
            })
            .cloned()
            .collect();
        
        if exit_servers.is_empty() {
            return Err(IronTunnelError::Config("No suitable exit servers found".to_string()));
        }
        
        use rand::seq::SliceRandom;
        let mut rng = rand::thread_rng();
        Ok(exit_servers.choose(&mut rng).unwrap().clone())
    }
    
    async fn measure_latency(&self, _ip: &IpAddr) -> Result<u64> {
        // Mock latency measurement
        Ok(rand::random::<u64>() % 200 + 10)
    }
    
    async fn get_server_load(&self, _server_id: &str) -> Result<f32> {
        // Mock server load
        Ok(rand::random::<f32>() * 100.0)
    }
    
    fn calculate_security_score(&self, servers: &[VpnServer]) -> f32 {
        let mut score = 0.0;
        
        // Geographic diversity
        let unique_countries: std::collections::HashSet<_> = servers.iter().map(|s| &s.country).collect();
        score += unique_countries.len() as f32 * 10.0;
        
        // Feature support
        for server in servers {
            if server.features.supports_tor { score += 5.0; }
            if server.features.obfuscated { score += 3.0; }
            if server.features.double_vpn_capable { score += 2.0; }
        }
        
        // Jurisdiction diversity
        let avoid_penalty = servers.iter()
            .filter(|s| self.config.avoid_countries.contains(&s.country))
            .count() as f32 * -15.0;
        
        score + avoid_penalty
    }
    
    fn calculate_server_security_score(&self, server: &VpnServer) -> f32 {
        let mut score = 0.0;
        
        if server.features.supports_tor { score += 10.0; }
        if server.features.obfuscated { score += 8.0; }
        if server.features.double_vpn_capable { score += 6.0; }
        if server.features.dedicated_ip { score += 4.0; }
        
        if self.config.avoid_countries.contains(&server.country) {
            score -= 20.0;
        }
        
        // Lower load is better for security
        score -= server.load_percentage * 0.1;
        
        score
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainInfo {
    pub id: Uuid,
    pub hop_count: usize,
    pub total_latency_ms: u64,
    pub security_score: f32,
    pub countries: Vec<String>,
    pub established_at: std::time::Instant,
}