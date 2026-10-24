// IronTunnel REST API Server
// Provides real-time data endpoints for GUI clients

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use sysinfo::System;

#[cfg(feature = "server-api")]
use warp::{Filter, Reply};

// Local server config to avoid circular dependency
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalServerConfig {
    pub max_clients: usize,
    pub enable_zk_proofs: bool,
    pub enable_wireguard: bool,
    pub enable_multihop: bool,
    pub enable_dns_filtering: bool,
    pub enable_kill_switch: bool,
}

// API Response types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
    pub timestamp: u64,
}

impl<T> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        }
    }

    pub fn error(message: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(message),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerStatus {
    pub online: bool,
    pub uptime_seconds: u64,
    pub version: String,
    pub active_clients: usize,
    pub max_clients: usize,
    pub cpu_usage: f32,
    pub memory_usage: f32,
    pub network_in_bytes: u64,
    pub network_out_bytes: u64,
    pub features: ServerFeatures,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerFeatures {
    pub zk_proofs: bool,
    pub wireguard: bool,
    pub multihop: bool,
    pub dns_protection: bool,
    pub kill_switch: bool,
    pub split_tunneling: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionStatus {
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
pub struct RealTimeStats {
    pub download_speed_bps: u64,
    pub upload_speed_bps: u64,
    pub ping_ms: u32,
    pub total_downloaded: u64,
    pub total_uploaded: u64,
    pub connection_time_seconds: u64,
    pub packet_loss_percent: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerHealth {
    pub status: String, // "healthy", "degraded", "unhealthy"
    pub response_time_ms: u32,
    pub load_percent: f32,
    pub available_bandwidth_mbps: u32,
    pub concurrent_connections: usize,
    pub error_rate_percent: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiServerInfo {
    pub id: String,
    pub name: String,
    pub country: String,
    pub city: String,
    pub ip_address: String,
    pub port: u16,
    pub health: ServerHealth,
    pub protocols: Vec<String>,
    pub features: Vec<String>,
}

// Client tracking for real statistics
#[derive(Debug, Clone)]
pub struct ClientStats {
    pub id: String,
    pub connected_since: Instant,
    pub last_activity: Instant,
    pub bytes_sent: Arc<RwLock<u64>>,
    pub bytes_received: Arc<RwLock<u64>>,
    pub last_ping: Arc<RwLock<Option<Duration>>>,
    pub ip_address: Option<SocketAddr>,
}

impl ClientStats {
    pub fn new(id: String, ip_address: Option<SocketAddr>) -> Self {
        let now = Instant::now();
        Self {
            id,
            connected_since: now,
            last_activity: now,
            bytes_sent: Arc::new(RwLock::new(0)),
            bytes_received: Arc::new(RwLock::new(0)),
            last_ping: Arc::new(RwLock::new(None)),
            ip_address,
        }
    }

    pub async fn update_bytes_sent(&self, bytes: u64) {
        let mut sent = self.bytes_sent.write().await;
        *sent += bytes;
    }

    pub async fn update_bytes_received(&self, bytes: u64) {
        let mut received = self.bytes_received.write().await;
        *received += bytes;
    }

    pub async fn update_ping(&self, ping: Duration) {
        let mut last_ping = self.last_ping.write().await;
        *last_ping = Some(ping);
    }

    pub async fn get_connection_status(&self) -> ConnectionStatus {
        let bytes_sent = *self.bytes_sent.read().await;
        let bytes_received = *self.bytes_received.read().await;
        let last_ping = *self.last_ping.read().await;

        ConnectionStatus {
            client_id: self.id.clone(),
            connected: true,
            ip_address: self.ip_address.map(|addr| addr.ip().to_string()),
            connected_since: Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
                    - self.connected_since.elapsed().as_secs(),
            ),
            bytes_sent,
            bytes_received,
            last_ping_ms: last_ping.map(|d| d.as_millis() as u32),
            protocol: "IronTunnel".to_string(),
            server_location: "Primary Server".to_string(),
        }
    }

    pub async fn get_realtime_stats(&self) -> RealTimeStats {
        let bytes_sent = *self.bytes_sent.read().await;
        let bytes_received = *self.bytes_received.read().await;
        let last_ping = *self.last_ping.read().await;
        let connection_time = self.connected_since.elapsed().as_secs();

        // Calculate speeds based on time window (rough estimate)
        let time_factor = if connection_time > 0 { connection_time } else { 1 };
        let download_speed = (bytes_received * 8) / time_factor; // Convert to bits per second
        let upload_speed = (bytes_sent * 8) / time_factor;

        RealTimeStats {
            download_speed_bps: download_speed,
            upload_speed_bps: upload_speed,
            ping_ms: last_ping.map(|d| d.as_millis() as u32).unwrap_or(0),
            total_downloaded: bytes_received,
            total_uploaded: bytes_sent,
            connection_time_seconds: connection_time,
            packet_loss_percent: 0.0, // TODO: Implement packet loss tracking
        }
    }
}

// API Server implementation
pub struct RestApiServer {
    pub clients: Arc<RwLock<HashMap<String, ClientStats>>>,
    pub server_start_time: Instant,
    pub server_config: LocalServerConfig,
}

impl RestApiServer {
    pub fn new(max_clients: usize) -> Self {
        let server_config = LocalServerConfig {
            max_clients,
            enable_zk_proofs: true,
            enable_wireguard: true,
            enable_multihop: true,
            enable_dns_filtering: true,
            enable_kill_switch: true,
        };
        Self {
            clients: Arc::new(RwLock::new(HashMap::new())),
            server_start_time: Instant::now(),
            server_config,
        }
    }

    pub async fn add_client(&self, client_id: String, ip_address: Option<SocketAddr>) {
        let mut clients = self.clients.write().await;
        clients.insert(client_id.clone(), ClientStats::new(client_id, ip_address));
    }

    pub async fn remove_client(&self, client_id: &str) {
        let mut clients = self.clients.write().await;
        clients.remove(client_id);
    }

    pub async fn update_client_traffic(&self, client_id: &str, bytes_sent: u64, bytes_received: u64) {
        let clients = self.clients.read().await;
        if let Some(client) = clients.get(client_id) {
            let mut sent = client.bytes_sent.write().await;
            let mut received = client.bytes_received.write().await;
            *sent += bytes_sent;
            *received += bytes_received;
        }
    }

    pub async fn update_client_ping(&self, client_id: &str, ping: Duration) {
        let clients = self.clients.read().await;
        if let Some(client) = clients.get(client_id) {
            client.update_ping(ping).await;
        }
    }

    pub async fn get_server_status(&self) -> ServerStatus {
        let clients = self.clients.read().await;
        let uptime = self.server_start_time.elapsed().as_secs();
        
        // Get system info
        let mut sys = System::new_all();
        sys.refresh_all();
        
        // flux port: sysinfo 0.30 API. Memory is exact; CPU is a single sample, so the
        // first call after start can read 0 (sysinfo needs two refreshes to diff).
        let cpu_usage = sys.global_cpu_info().cpu_usage();
        let memory_usage = if sys.total_memory() > 0 {
            (sys.used_memory() as f64 / sys.total_memory() as f64 * 100.0) as f32
        } else {
            0.0
        };

        ServerStatus {
            online: true,
            uptime_seconds: uptime,
            version: "0.0.2-alpha".to_string(),
            active_clients: clients.len(),
            max_clients: self.server_config.max_clients,
            cpu_usage,
            memory_usage,
            network_in_bytes: 0, // TODO: Implement network monitoring
            network_out_bytes: 0,
            features: ServerFeatures {
                zk_proofs: self.server_config.enable_zk_proofs,
                wireguard: self.server_config.enable_wireguard,
                multihop: self.server_config.enable_multihop,
                dns_protection: self.server_config.enable_dns_filtering,
                kill_switch: self.server_config.enable_kill_switch,
                split_tunneling: true, // Always available
            },
        }
    }

    pub async fn get_connection_status(&self, client_id: &str) -> Option<ConnectionStatus> {
        let clients = self.clients.read().await;
        if let Some(client) = clients.get(client_id) {
            Some(client.get_connection_status().await)
        } else {
            None
        }
    }

    pub async fn get_realtime_stats(&self, client_id: &str) -> Option<RealTimeStats> {
        let clients = self.clients.read().await;
        if let Some(client) = clients.get(client_id) {
            Some(client.get_realtime_stats().await)
        } else {
            None
        }
    }

    pub async fn get_server_health(&self) -> ServerHealth {
        let clients = self.clients.read().await;
        let load = (clients.len() as f32 / self.server_config.max_clients as f32) * 100.0;
        
        let status = if load < 70.0 {
            "healthy"
        } else if load < 90.0 {
            "degraded"
        } else {
            "unhealthy"
        };

        ServerHealth {
            status: status.to_string(),
            response_time_ms: 15, // TODO: Measure actual response time
            load_percent: load,
            available_bandwidth_mbps: 1000, // TODO: Measure actual bandwidth
            concurrent_connections: clients.len(),
            error_rate_percent: 0.1, // TODO: Track actual error rate
        }
    }

    pub async fn get_servers_list(&self) -> Vec<ApiServerInfo> {
        // For now, return the current server as the primary server
        let health = self.get_server_health().await;
        
        vec![ApiServerInfo {
            id: "primary-server".to_string(),
            name: "Primary Server".to_string(),
            country: "Global".to_string(),
            city: "Primary Location".to_string(),
            ip_address: "161.35.219.10".to_string(),
            port: 1194,
            health,
            protocols: vec!["IronTunnel".to_string(), "WireGuard".to_string()],
            features: vec![
                "Zero-Knowledge Proofs".to_string(),
                "ChaCha20-Poly1305".to_string(),
                "DNS Protection".to_string(),
                "Kill Switch".to_string(),
            ],
        }]
    }

    #[cfg(feature = "server-api")]
    pub async fn start_api_server(&self, port: u16) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let api_server = Arc::new(self.clone());
        
        // CORS headers
        let cors = warp::cors()
            .allow_any_origin()
            .allow_headers(vec!["content-type", "authorization", "x-api-key"])
            .allow_methods(vec!["GET", "POST", "PUT", "DELETE", "OPTIONS"]);

        // Health check endpoint
        let health = warp::path("health")
            .and(warp::get())
            .map(|| {
                warp::reply::json(&ApiResponse::success("OK"))
            });

        // Server status endpoint
        let api_server_clone = api_server.clone();
        let status = warp::path!("api" / "v1" / "status")
            .and(warp::get())
            .and_then(move || {
                let api_server = api_server_clone.clone();
                async move {
                    let status = api_server.get_server_status().await;
                    Ok::<_, warp::Rejection>(warp::reply::json(&ApiResponse::success(status)))
                }
            });

        // Connection status endpoint
        let api_server_clone = api_server.clone();
        let connection = warp::path!("api" / "v1" / "connection" / String)
            .and(warp::get())
            .and_then(move |client_id: String| {
                let api_server = api_server_clone.clone();
                async move {
                    if let Some(status) = api_server.get_connection_status(&client_id).await {
                        Ok::<_, warp::Rejection>(warp::reply::json(&ApiResponse::success(status)))
                    } else {
                        Ok(warp::reply::json(&ApiResponse::<()>::error("Client not found".to_string())))
                    }
                }
            });

        // Real-time stats endpoint
        let api_server_clone = api_server.clone();
        let stats = warp::path!("api" / "v1" / "stats" / String)
            .and(warp::get())
            .and_then(move |client_id: String| {
                let api_server = api_server_clone.clone();
                async move {
                    if let Some(stats) = api_server.get_realtime_stats(&client_id).await {
                        Ok::<_, warp::Rejection>(warp::reply::json(&ApiResponse::success(stats)))
                    } else {
                        Ok(warp::reply::json(&ApiResponse::<()>::error("Client not found".to_string())))
                    }
                }
            });

        // Servers list endpoint
        let api_server_clone = api_server.clone();
        let servers = warp::path!("api" / "v1" / "servers")
            .and(warp::get())
            .and_then(move || {
                let api_server = api_server_clone.clone();
                async move {
                    let servers = api_server.get_servers_list().await;
                    Ok::<_, warp::Rejection>(warp::reply::json(&ApiResponse::success(servers)))
                }
            });

        // Server health endpoint
        let api_server_clone = api_server.clone();
        let server_health = warp::path!("api" / "v1" / "servers" / "health")
            .and(warp::get())
            .and_then(move || {
                let api_server = api_server_clone.clone();
                async move {
                    let health = api_server.get_server_health().await;
                    Ok::<_, warp::Rejection>(warp::reply::json(&ApiResponse::success(health)))
                }
            });

        // Combine all routes
        let routes = health
            .or(status)
            .or(connection)
            .or(stats)
            .or(servers)
            .or(server_health)
            .with(cors)
            .recover(handle_rejection);

        println!("🚀 REST API server starting on port {}", port);
        warp::serve(routes)
            .run(([0, 0, 0, 0], port))
            .await;

        Ok(())
    }
}

impl Clone for RestApiServer {
    fn clone(&self) -> Self {
        Self {
            clients: self.clients.clone(),
            server_start_time: self.server_start_time,
            server_config: self.server_config.clone(),
        }
    }
}

#[cfg(feature = "server-api")]
async fn handle_rejection(err: warp::Rejection) -> Result<impl Reply, std::convert::Infallible> {
    let (code, message) = if err.is_not_found() {
        (warp::http::StatusCode::NOT_FOUND, "Not Found")
    } else if let Some(_) = err.find::<warp::filters::body::BodyDeserializeError>() {
        (warp::http::StatusCode::BAD_REQUEST, "Invalid JSON")
    } else {
        (warp::http::StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error")
    };

    Ok(warp::reply::with_status(
        warp::reply::json(&ApiResponse::<()>::error(message.to_string())),
        code,
    ))
}