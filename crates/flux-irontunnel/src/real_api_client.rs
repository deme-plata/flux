// Real API Client for fetching data from IronTunnel REST API
// Replaces mock data with real server statistics

use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealConnectionStats {
    pub download_speed_bps: u64,
    pub upload_speed_bps: u64,
    pub ping_ms: u32,
    pub total_downloaded: u64,
    pub total_uploaded: u64,
    pub connection_time_seconds: u64,
    pub packet_loss_percent: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealServerStatus {
    pub online: bool,
    pub uptime_seconds: u64,
    pub version: String,
    pub active_clients: usize,
    pub max_clients: usize,
    pub cpu_usage: f32,
    pub memory_usage: f32,
    pub network_in_bytes: u64,
    pub network_out_bytes: u64,
    pub features: RealServerFeatures,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealServerFeatures {
    pub zk_proofs: bool,
    pub wireguard: bool,
    pub multihop: bool,
    pub dns_protection: bool,
    pub kill_switch: bool,
    pub split_tunneling: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealConnectionStatus {
    pub client_id: String,
    pub connected: bool,
    pub ip_address: Option<String>,
    pub connected_since: Option<u64>,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub last_ping_ms: Option<u32>,
    pub protocol: String,
    pub server_location: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
    pub timestamp: u64,
}

#[derive(Clone)]
pub struct RealApiClient {
    pub base_url: String,
    pub client_id: String,
    pub timeout: Duration,
}

impl RealApiClient {
    pub fn new(server_host: &str, _server_port: u16, client_id: String) -> Self {
        Self {
            base_url: format!("http://{}:{}", server_host, 28080), // REST API port
            client_id,
            timeout: Duration::from_secs(5),
        }
    }

    // Fallback for when API is not available - return sensible defaults
    fn get_fallback_stats() -> RealConnectionStats {
        RealConnectionStats {
            download_speed_bps: 0,
            upload_speed_bps: 0,
            ping_ms: 0,
            total_downloaded: 0,
            total_uploaded: 0,
            connection_time_seconds: 0,
            packet_loss_percent: 0.0,
        }
    }

    fn get_fallback_server_status() -> RealServerStatus {
        RealServerStatus {
            online: false,
            uptime_seconds: 0,
            version: "Unknown".to_string(),
            active_clients: 0,
            max_clients: 1000,
            cpu_usage: 0.0,
            memory_usage: 0.0,
            network_in_bytes: 0,
            network_out_bytes: 0,
            features: RealServerFeatures {
                zk_proofs: true,
                wireguard: true,
                multihop: true,
                dns_protection: true,
                kill_switch: true,
                split_tunneling: true,
            },
        }
    }

    fn get_fallback_connection_status(&self) -> RealConnectionStatus {
        RealConnectionStatus {
            client_id: self.client_id.clone(),
            connected: false,
            ip_address: None,
            connected_since: None,
            bytes_sent: 0,
            bytes_received: 0,
            last_ping_ms: None,
            protocol: "IronTunnel".to_string(),
            server_location: "Unknown".to_string(),
        }
    }

    // Fetch real-time connection statistics
    pub async fn get_connection_stats(&self) -> RealConnectionStats {
        #[cfg(feature = "api")]
        {
            let url = format!("{}/api/v1/stats/{}", self.base_url, self.client_id);
            
            match reqwest::Client::new()
                .get(&url)
                .timeout(self.timeout)
                .send()
                .await
            {
                Ok(response) => {
                    if response.status().is_success() {
                        match response.json::<ApiResponse<RealConnectionStats>>().await {
                            Ok(api_response) => {
                                if api_response.success {
                                    if let Some(stats) = api_response.data {
                                        return stats;
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("🔧 Failed to parse stats response: {}", e);
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("🔧 API not available yet ({}:28080) - using fallback data", self.base_url);
                    eprintln!("💡 Server may need deployment with REST API enabled");
                }
            }
        }

        // Return fallback data when API is not available
        Self::get_fallback_stats()
    }

    // Fetch server status and health
    pub async fn get_server_status(&self) -> RealServerStatus {
        #[cfg(feature = "api")]
        {
            let url = format!("{}/api/v1/status", self.base_url);
            
            match reqwest::Client::new()
                .get(&url)
                .timeout(self.timeout)
                .send()
                .await
            {
                Ok(response) => {
                    if response.status().is_success() {
                        match response.json::<ApiResponse<RealServerStatus>>().await {
                            Ok(api_response) => {
                                if api_response.success {
                                    if let Some(status) = api_response.data {
                                        return status;
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("🔧 Failed to parse server status: {}", e);
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("🔧 Failed to fetch server status: {}", e);
                }
            }
        }

        // Return fallback data when API is not available
        Self::get_fallback_server_status()
    }

    // Check real connection status
    pub async fn get_connection_status(&self) -> RealConnectionStatus {
        #[cfg(feature = "api")]
        {
            let url = format!("{}/api/v1/connection/{}", self.base_url, self.client_id);
            
            match reqwest::Client::new()
                .get(&url)
                .timeout(self.timeout)
                .send()
                .await
            {
                Ok(response) => {
                    if response.status().is_success() {
                        match response.json::<ApiResponse<RealConnectionStatus>>().await {
                            Ok(api_response) => {
                                if api_response.success {
                                    if let Some(status) = api_response.data {
                                        return status;
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("🔧 Failed to parse connection status: {}", e);
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("🔧 Failed to fetch connection status: {}", e);
                }
            }
        }

        // Return fallback data when API is not available
        self.get_fallback_connection_status()
    }

    // Simple health check to verify API is reachable
    pub async fn health_check(&self) -> bool {
        #[cfg(feature = "api")]
        {
            let url = format!("{}/health", self.base_url);
            
            match reqwest::Client::new()
                .get(&url)
                .timeout(Duration::from_secs(2))
                .send()
                .await
            {
                Ok(response) => response.status().is_success(),
                Err(_) => false,
            }
        }

        #[cfg(not(feature = "api"))]
        false
    }

    // Format speed for display
    pub fn format_speed(bps: u64) -> String {
        if bps == 0 {
            return "0 B/s".to_string();
        }

        let mbps = bps as f64 / (1024.0 * 1024.0);
        if mbps >= 1.0 {
            format!("{:.1} MB/s", mbps)
        } else {
            let kbps = bps as f64 / 1024.0;
            if kbps >= 1.0 {
                format!("{:.1} KB/s", kbps)
            } else {
                format!("{} B/s", bps)
            }
        }
    }

    // Format bytes for display
    pub fn format_bytes(bytes: u64) -> String {
        if bytes == 0 {
            return "0 B".to_string();
        }

        let gb = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
        if gb >= 1.0 {
            format!("{:.2} GB", gb)
        } else {
            let mb = bytes as f64 / (1024.0 * 1024.0);
            if mb >= 1.0 {
                format!("{:.1} MB", mb)
            } else {
                let kb = bytes as f64 / 1024.0;
                format!("{:.1} KB", kb)
            }
        }
    }

    // Format connection time
    pub fn format_connection_time(seconds: u64) -> String {
        if seconds == 0 {
            return "00:00:00".to_string();
        }

        let hours = seconds / 3600;
        let minutes = (seconds % 3600) / 60;
        let secs = seconds % 60;
        format!("{:02}:{:02}:{:02}", hours, minutes, secs)
    }
}