// IronTunnel API Client - Production Server Communication

use serde::{Deserialize, Serialize};
use std::time::Duration;
use crate::Result;

#[cfg(feature = "api")]
use reqwest;

// API Configuration
pub struct ApiConfig {
    pub base_url: String,
    pub api_key: Option<String>,
    pub timeout: Duration,
    pub retry_attempts: u32,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.irontunnel.com/v1".to_string(),
            api_key: None,
            timeout: Duration::from_secs(10),
            retry_attempts: 3,
        }
    }
}

// Server information from API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub id: String,
    pub hostname: String,
    pub ip_address: String,
    pub ipv6_address: Option<String>,
    pub port: u16,
    pub location: ServerLocation,
    pub features: Vec<String>,
    pub protocols: Vec<String>,
    pub public_key: String,
    pub certificate: Option<String>,
    pub status: ServerStatus,
    pub metrics: ServerMetrics,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerLocation {
    pub country: String,
    pub country_code: String,
    pub city: String,
    pub region: String,
    pub continent: String,
    pub latitude: f64,
    pub longitude: f64,
    pub timezone: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerStatus {
    pub online: bool,
    pub maintenance: bool,
    pub load_percentage: f32,
    pub current_users: u32,
    pub max_users: u32,
    pub bandwidth_used: u64,
    pub bandwidth_limit: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerMetrics {
    pub average_ping: u32,
    pub packet_loss: f32,
    pub uptime_percentage: f32,
    pub last_updated: String,
}

// Authentication response
#[derive(Debug, Serialize, Deserialize)]
pub struct AuthResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
    pub token_type: String,
    pub scope: Vec<String>,
}

// ZK proof submission
#[derive(Debug, Serialize, Deserialize)]
pub struct ZkAuthRequest {
    pub identity_commitment: Vec<u8>,
    pub proof: Vec<u8>,
    pub challenge: Vec<u8>,
    pub timestamp: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ZkAuthResponse {
    pub session_id: String,
    pub server_public_key: Vec<u8>,
    pub encrypted_config: Vec<u8>,
    pub valid_until: u64,
}

// Connection statistics
#[derive(Debug, Serialize, Deserialize)]
pub struct ConnectionStats {
    pub session_id: String,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub packets_sent: u64,
    pub packets_received: u64,
    pub connection_time: u64,
    pub average_speed: f64,
    pub current_protocol: String,
}

// API Client implementation
pub struct ApiClient {
    config: ApiConfig,
    #[cfg(feature = "api")]
    client: reqwest::Client,
}

#[cfg(feature = "api")]
impl ApiClient {
    pub fn new(config: ApiConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .user_agent("IronTunnel-Client/1.0")
            .build()
            .map_err(|e| crate::IronTunnelError::NetworkError(e.to_string()))?;
        
        Ok(Self { config, client })
    }
    
    // Get list of available servers
    pub async fn get_servers(&self) -> Result<Vec<ServerInfo>> {
        let url = format!("{}/servers", self.config.base_url);
        
        let mut request = self.client.get(&url);
        if let Some(api_key) = &self.config.api_key {
            request = request.header("X-API-Key", api_key);
        }
        
        let response = request
            .send()
            .await
            .map_err(|e| crate::IronTunnelError::NetworkError(e.to_string()))?;
        
        if !response.status().is_success() {
            return Err(crate::IronTunnelError::ApiError(
                format!("Server returned status: {}", response.status())
            ));
        }
        
        let servers = response
            .json::<Vec<ServerInfo>>()
            .await
            .map_err(|e| crate::IronTunnelError::ParseError(e.to_string()))?;
        
        Ok(servers)
    }
    
    // Get optimal server based on location
    pub async fn get_optimal_server(&self, user_ip: Option<&str>) -> Result<ServerInfo> {
        let mut url = format!("{}/servers/optimal", self.config.base_url);
        if let Some(ip) = user_ip {
            url.push_str(&format!("?ip={}", ip));
        }
        
        let mut request = self.client.get(&url);
        if let Some(api_key) = &self.config.api_key {
            request = request.header("X-API-Key", api_key);
        }
        
        let response = request
            .send()
            .await
            .map_err(|e| crate::IronTunnelError::NetworkError(e.to_string()))?;
        
        if !response.status().is_success() {
            return Err(crate::IronTunnelError::ApiError(
                format!("Failed to get optimal server: {}", response.status())
            ));
        }
        
        let server = response
            .json::<ServerInfo>()
            .await
            .map_err(|e| crate::IronTunnelError::ParseError(e.to_string()))?;
        
        Ok(server)
    }
    
    // Authenticate with ZK proof
    pub async fn authenticate_zk(&self, request: ZkAuthRequest) -> Result<ZkAuthResponse> {
        let url = format!("{}/auth/zk", self.config.base_url);
        
        let response = self.client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| crate::IronTunnelError::NetworkError(e.to_string()))?;
        
        if !response.status().is_success() {
            return Err(crate::IronTunnelError::AuthError(
                "ZK authentication failed".to_string()
            ));
        }
        
        let auth_response = response
            .json::<ZkAuthResponse>()
            .await
            .map_err(|e| crate::IronTunnelError::ParseError(e.to_string()))?;
        
        Ok(auth_response)
    }
    
    // Submit connection statistics
    pub async fn submit_stats(&self, stats: ConnectionStats) -> Result<()> {
        let url = format!("{}/stats", self.config.base_url);
        
        let mut request = self.client.post(&url).json(&stats);
        if let Some(api_key) = &self.config.api_key {
            request = request.header("X-API-Key", api_key);
        }
        
        let response = request
            .send()
            .await
            .map_err(|e| crate::IronTunnelError::NetworkError(e.to_string()))?;
        
        if !response.status().is_success() {
            return Err(crate::IronTunnelError::ApiError(
                format!("Failed to submit stats: {}", response.status())
            ));
        }
        
        Ok(())
    }
    
    // Check server health
    pub async fn check_server_health(&self, server_id: &str) -> Result<bool> {
        let url = format!("{}/servers/{}/health", self.config.base_url, server_id);
        
        let mut request = self.client.get(&url);
        if let Some(api_key) = &self.config.api_key {
            request = request.header("X-API-Key", api_key);
        }
        
        match request.send().await {
            Ok(response) => Ok(response.status().is_success()),
            Err(_) => Ok(false),
        }
    }
    
    // Get server certificate
    pub async fn get_server_certificate(&self, server_id: &str) -> Result<String> {
        let url = format!("{}/servers/{}/certificate", self.config.base_url, server_id);
        
        let mut request = self.client.get(&url);
        if let Some(api_key) = &self.config.api_key {
            request = request.header("X-API-Key", api_key);
        }
        
        let response = request
            .send()
            .await
            .map_err(|e| crate::IronTunnelError::NetworkError(e.to_string()))?;
        
        if !response.status().is_success() {
            return Err(crate::IronTunnelError::ApiError(
                format!("Failed to get certificate: {}", response.status())
            ));
        }
        
        let cert = response
            .text()
            .await
            .map_err(|e| crate::IronTunnelError::NetworkError(e.to_string()))?;
        
        Ok(cert)
    }
}

// Non-API implementation for when reqwest is not available
#[cfg(not(feature = "api"))]
impl ApiClient {
    pub fn new(config: ApiConfig) -> Result<Self> {
        Ok(Self { config })
    }
    
    // Return single real server when API is not available
    pub async fn get_servers(&self) -> Result<Vec<ServerInfo>> {
        let server_host = std::env::var("IRONTUNNEL_SERVER")
            .unwrap_or_else(|_| "161.35.219.10".to_string());
        let server_port = std::env::var("IRONTUNNEL_PORT")
            .unwrap_or_else(|_| "1194".to_string())
            .parse::<u16>()
            .unwrap_or(1194);
            
        Ok(vec![
            ServerInfo {
                id: "primary".to_string(),
                hostname: server_host.clone(),
                ip_address: server_host.clone(),
                ipv6_address: None,
                port: server_port,
                location: ServerLocation {
                    country: "United States".to_string(),
                    country_code: "US".to_string(),
                    city: "New York".to_string(),
                    region: "NY".to_string(),
                    continent: "North America".to_string(),
                    latitude: 40.7128,
                    longitude: -74.0060,
                    timezone: "America/New_York".to_string(),
                },
                features: vec![
                    "wireguard".to_string(), 
                    "zk-stark".to_string(),
                    "multihop".to_string(),
                    "dns-protection".to_string(),
                ],
                protocols: vec!["wireguard".to_string()],
                public_key: "development-key".to_string(),
                certificate: None,
                status: ServerStatus {
                    online: true,
                    maintenance: false,
                    load_percentage: 0.0,
                    current_users: 0,
                    max_users: 100,
                    bandwidth_used: 0,
                    bandwidth_limit: 1024 * 1024 * 1024,
                },
                metrics: ServerMetrics {
                    average_ping: 0,
                    packet_loss: 0.0,
                    uptime_percentage: 100.0,
                    last_updated: chrono::Utc::now().to_rfc3339(),
                },
                tags: vec!["production".to_string(), "primary".to_string()],
            },
        ])
    }
    
    pub async fn get_optimal_server(&self, _user_ip: Option<&str>) -> Result<ServerInfo> {
        self.get_servers().await.map(|mut servers| servers.pop().unwrap())
    }
    
    pub async fn authenticate_zk(&self, _request: ZkAuthRequest) -> Result<ZkAuthResponse> {
        Ok(ZkAuthResponse {
            session_id: "dev-session".to_string(),
            server_public_key: vec![0; 32],
            encrypted_config: vec![],
            valid_until: 0,
        })
    }
    
    pub async fn submit_stats(&self, _stats: ConnectionStats) -> Result<()> {
        Ok(())
    }
    
    pub async fn check_server_health(&self, _server_id: &str) -> Result<bool> {
        Ok(true)
    }
    
    pub async fn get_server_certificate(&self, _server_id: &str) -> Result<String> {
        Ok("DEVELOPMENT CERTIFICATE".to_string())
    }
}