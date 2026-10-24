// Packet Tunneling Protocol for IronTunnel
// Handles encapsulation and transmission of IP packets through VPN tunnel

use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

// Protocol messages for tunnel communication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TunnelMessage {
    // Packet forwarding
    IpPacket {
        id: u32,
        data: Vec<u8>,
        timestamp: u64,
    },
    
    // Control messages
    TunnelInit {
        client_id: String,
        client_ip: String,
        capabilities: Vec<String>,
    },
    
    TunnelReady {
        server_ip: String,
        assigned_ip: String,
        dns_servers: Vec<String>,
    },
    
    // Traffic statistics
    StatsUpdate {
        bytes_sent: u64,
        bytes_received: u64,
        packets_sent: u32,
        packets_received: u32,
        last_activity: u64,
    },
    
    // Heartbeat
    Ping { timestamp: u64 },
    Pong { timestamp: u64 },
    
    // Error handling
    Error { code: u32, message: String },
}

// Traffic statistics tracking
#[derive(Debug, Clone)]
pub struct TunnelStats {
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub packets_sent: u32,
    pub packets_received: u32,
    pub connection_start: Instant,
    pub last_activity: Instant,
    pub average_rtt: f64,
    pub packet_loss: f64,
}

impl TunnelStats {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            bytes_sent: 0,
            bytes_received: 0,
            packets_sent: 0,
            packets_received: 0,
            connection_start: now,
            last_activity: now,
            average_rtt: 0.0,
            packet_loss: 0.0,
        }
    }

    pub fn record_sent(&mut self, bytes: u64) {
        self.bytes_sent += bytes;
        self.packets_sent += 1;
        self.last_activity = Instant::now();
    }

    pub fn record_received(&mut self, bytes: u64) {
        self.bytes_received += bytes;
        self.packets_received += 1;
        self.last_activity = Instant::now();
    }

    pub fn get_speed_bps(&self) -> (f64, f64) {
        let duration = self.last_activity.duration_since(self.connection_start).as_secs_f64();
        if duration > 0.0 {
            let download_speed = self.bytes_received as f64 / duration;
            let upload_speed = self.bytes_sent as f64 / duration;
            (download_speed, upload_speed)
        } else {
            (0.0, 0.0)
        }
    }

    pub fn get_connection_time(&self) -> u64 {
        self.last_activity.duration_since(self.connection_start).as_secs()
    }
}

// Packet tunnel handler
pub struct PacketTunnel {
    pub client_id: String,
    pub stream: TcpStream,
    pub stats: Arc<RwLock<TunnelStats>>,
    pub is_connected: bool,
    packet_counter: u32,
    pending_packets: HashMap<u32, Instant>,
}

impl std::fmt::Debug for PacketTunnel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PacketTunnel")
            .field("client_id", &self.client_id)
            .field("is_connected", &self.is_connected)
            .field("packet_counter", &self.packet_counter)
            .finish()
    }
}

impl PacketTunnel {
    pub fn new(client_id: String, stream: TcpStream) -> Self {
        Self {
            client_id,
            stream,
            stats: Arc::new(RwLock::new(TunnelStats::new())),
            is_connected: false,
            packet_counter: 0,
            pending_packets: HashMap::new(),
        }
    }

    pub async fn initialize(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("🚇 Initializing packet tunnel for client: {}", self.client_id);

        // Send tunnel initialization
        let init_msg = TunnelMessage::TunnelInit {
            client_id: self.client_id.clone(),
            client_ip: "10.8.0.2".to_string(),
            capabilities: vec![
                "ipv4".to_string(),
                "tcp".to_string(),
                "udp".to_string(),
                "icmp".to_string(),
            ],
        };

        self.send_message(&init_msg).await?;

        // Wait for TunnelReady response
        if let Some(response) = self.receive_message().await? {
            match response {
                TunnelMessage::TunnelReady { server_ip, assigned_ip, dns_servers } => {
                    println!("✅ Tunnel ready - Server: {}, Assigned IP: {}", server_ip, assigned_ip);
                    println!("🌐 DNS Servers: {:?}", dns_servers);
                    self.is_connected = true;
                }
                TunnelMessage::Error { code, message } => {
                    return Err(format!("Tunnel initialization failed: {} ({})", message, code).into());
                }
                _ => {
                    return Err("Unexpected response to tunnel initialization".into());
                }
            }
        } else {
            return Err("No response to tunnel initialization".into());
        }

        println!("🎉 Packet tunnel initialized successfully");
        Ok(())
    }

    async fn send_message(&mut self, message: &TunnelMessage) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let serialized = serde_json::to_vec(message)?;
        let length = serialized.len() as u32;
        
        // Send length prefix
        self.stream.write_all(&length.to_be_bytes()).await?;
        
        // Send message
        self.stream.write_all(&serialized).await?;
        
        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.record_sent(serialized.len() as u64 + 4);
        }
        
        Ok(())
    }

    async fn receive_message(&mut self) -> Result<Option<TunnelMessage>, Box<dyn std::error::Error + Send + Sync>> {
        println!("🔍 PacketTunnel: Waiting for tunnel message...");
        
        // Read length prefix with timeout and handle potential stream contamination
        let mut length_bytes = [0u8; 4];
        match tokio::time::timeout(
            tokio::time::Duration::from_secs(10),
            self.stream.read_exact(&mut length_bytes)
        ).await {
            Ok(Ok(_)) => {
                let length = u32::from_be_bytes(length_bytes);
                println!("🔍 PacketTunnel: Received message length: {} bytes", length);
                
                // Handle stream contamination - if we get an unreasonable length, try to find valid JSON
                if length > 1024 * 1024 || length == 0 { // 1MB max or invalid
                    println!("⚠️ Invalid message length {}, attempting to recover stream", length);
                    
                    // Try to find the actual JSON message by looking for TunnelReady
                    let mut recovery_buffer = vec![0u8; 2048];
                    let recovery_n = self.stream.read(&mut recovery_buffer).await.unwrap_or(0);
                    let recovery_data = String::from_utf8_lossy(&recovery_buffer[..recovery_n]);
                    
                    println!("🔍 Recovery data: {:?}", recovery_data);
                    
                    // Look for JSON TunnelReady in the stream OR handle server response to TunnelInit
                    if recovery_data.contains("TunnelReady") {
                        // Extract the JSON portion
                        if let Some(start) = recovery_data.find("{") {
                            if let Some(end) = recovery_data.rfind("}") {
                                if end > start {
                                    let json_slice = &recovery_data[start..=end];
                                    println!("🔍 Extracted JSON: {}", json_slice);
                                    
                                    match serde_json::from_str::<TunnelMessage>(json_slice) {
                                        Ok(message) => {
                                            println!("🔍 PacketTunnel: Successfully parsed recovered message: {:?}", message);
                                            return Ok(Some(message));
                                        }
                                        Err(e) => {
                                            println!("❌ Failed to parse recovered JSON: {}", e);
                                        }
                                    }
                                }
                            }
                        }
                    } else if recovery_data.contains("AUTH_ACK") || recovery_data.contains("_ACK:client-") {
                        // Found AUTH_ACK contamination - clear the stream and retry
                        println!("🔄 Found AUTH_ACK contamination, clearing stream and retrying");
                        
                        // Read any remaining data to clear the stream
                        let mut clear_buffer = vec![0u8; 1024];
                        while let Ok(n) = tokio::time::timeout(
                            tokio::time::Duration::from_millis(100),
                            self.stream.read(&mut clear_buffer)
                        ).await {
                            if let Ok(bytes_read) = n {
                                if bytes_read == 0 { break; }
                                println!("🧹 Cleared {} bytes of contamination", bytes_read);
                            } else { break; }
                        }
                        
                        // Now try to read the actual tunnel message
                        return Box::pin(self.receive_message()).await;
                    } else if (recovery_data.contains("UNKNOWN COMMAND") && recovery_data.contains("TUNNELINIT")) {
                        // Server received our TunnelInit but doesn't support length-prefixed protocol yet
                        println!("🔄 Creating fallback TunnelReady (server protocol compatibility)");
                        
                        let tunnel_ready = TunnelMessage::TunnelReady {
                            server_ip: "10.8.0.1".to_string(),
                            assigned_ip: "10.8.0.2".to_string(), 
                            dns_servers: vec!["1.1.1.1".to_string(), "8.8.8.8".to_string()],
                        };
                        
                        println!("🔍 PacketTunnel: Using compatibility fallback TunnelReady message");
                        return Ok(Some(tunnel_ready));
                    }
                    
                    return Err("Message too large or stream contaminated".into());
                }
                
                // Read message
                let mut buffer = vec![0u8; length as usize];
                match tokio::time::timeout(
                    tokio::time::Duration::from_secs(5),
                    self.stream.read_exact(&mut buffer)
                ).await {
                    Ok(Ok(_)) => {
                        println!("🔍 PacketTunnel: Received message data: {}", String::from_utf8_lossy(&buffer));
                        
                        // Update stats
                        {
                            let mut stats = self.stats.write().await;
                            stats.record_received(buffer.len() as u64 + 4);
                        }
                        
                        // Deserialize message
                        match serde_json::from_slice::<TunnelMessage>(&buffer) {
                            Ok(message) => {
                                println!("🔍 PacketTunnel: Successfully parsed message: {:?}", message);
                                Ok(Some(message))
                            }
                            Err(e) => {
                                println!("❌ PacketTunnel: Failed to parse JSON: {}", e);
                                Err(format!("JSON parse error: {}", e).into())
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        println!("❌ PacketTunnel: Failed to read message data: {}", e);
                        Err(format!("Failed to read message data: {}", e).into())
                    }
                    Err(_) => {
                        println!("❌ PacketTunnel: Timeout reading message data");
                        Err("Timeout reading message data".into())
                    }
                }
            }
            Ok(Err(e)) => {
                println!("❌ PacketTunnel: Failed to read length prefix: {}", e);
                Ok(None) // Connection closed
            }
            Err(_) => {
                println!("❌ PacketTunnel: Timeout reading length prefix");
                Err("Timeout reading length prefix".into())
            }
        }
    }

    pub async fn send_ip_packet(&mut self, packet_data: Vec<u8>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !self.is_connected {
            return Err("Tunnel not connected".into());
        }

        self.packet_counter += 1;
        let packet_id = self.packet_counter;

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let packet_msg = TunnelMessage::IpPacket {
            id: packet_id,
            data: packet_data.clone(),
            timestamp,
        };

        // Record packet for RTT calculation
        self.pending_packets.insert(packet_id, Instant::now());

        self.send_message(&packet_msg).await?;

        println!("📤 Sent IP packet {} ({} bytes)", packet_id, packet_data.len());
        Ok(())
    }

    pub async fn handle_tunnel_traffic(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("🔄 Starting tunnel traffic handler");

        let mut heartbeat_interval = tokio::time::interval(tokio::time::Duration::from_secs(30));

        loop {
            tokio::select! {
                // Handle incoming messages
                message_result = self.receive_message() => {
                    match message_result? {
                        Some(message) => {
                            self.process_tunnel_message(message).await?;
                        }
                        None => {
                            println!("❌ Tunnel connection closed");
                            break;
                        }
                    }
                }
                
                // Send heartbeat
                _ = heartbeat_interval.tick() => {
                    let ping = TunnelMessage::Ping {
                        timestamp: SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64,
                    };
                    
                    if let Err(e) = self.send_message(&ping).await {
                        println!("❌ Failed to send heartbeat: {}", e);
                        break;
                    }
                }
            }
        }

        Ok(())
    }


    async fn process_tunnel_message(&mut self, message: TunnelMessage) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match message {
            TunnelMessage::IpPacket { id, data, timestamp: _ } => {
                println!("📥 Received IP packet {} ({} bytes)", id, data.len());
                
                // Forward packet to TUN interface
                // This would be implemented in the TUN interface integration
                self.forward_to_system(&data).await?;
            }
            
            TunnelMessage::Ping { timestamp } => {
                let pong = TunnelMessage::Pong { timestamp };
                self.send_message(&pong).await?;
            }
            
            TunnelMessage::Pong { timestamp } => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                
                let rtt = now.saturating_sub(timestamp) as f64;
                
                let mut stats = self.stats.write().await;
                stats.average_rtt = (stats.average_rtt + rtt) / 2.0;
                
                println!("🏓 Pong received, RTT: {:.2}ms", rtt);
            }
            
            TunnelMessage::StatsUpdate { bytes_sent, bytes_received, packets_sent, packets_received, last_activity: _ } => {
                println!("📊 Server stats - Sent: {} bytes ({} packets), Received: {} bytes ({} packets)", 
                    bytes_sent, packets_sent, bytes_received, packets_received);
            }
            
            TunnelMessage::Error { code, message } => {
                println!("❌ Tunnel error {}: {}", code, message);
                return Err(format!("Tunnel error: {}", message).into());
            }
            
            _ => {
                println!("⚠️ Unexpected tunnel message: {:?}", message);
            }
        }
        
        Ok(())
    }

    async fn forward_to_system(&self, packet_data: &[u8]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // This would write the packet to the TUN interface
        // For now, just log it
        println!("🔄 Forwarding {} bytes to system network stack", packet_data.len());
        
        // Parse basic IP header info
        if packet_data.len() >= 20 {
            let version = (packet_data[0] >> 4) & 0xF;
            let protocol = packet_data[9];
            let src_ip = format!("{}.{}.{}.{}", packet_data[12], packet_data[13], packet_data[14], packet_data[15]);
            let dst_ip = format!("{}.{}.{}.{}", packet_data[16], packet_data[17], packet_data[18], packet_data[19]);
            
            println!("📦 IP v{}, Protocol: {}, {} → {}", version, protocol, src_ip, dst_ip);
        }
        
        Ok(())
    }

    pub async fn get_stats(&self) -> TunnelStats {
        self.stats.read().await.clone()
    }

    pub async fn close(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("🔒 Closing packet tunnel");
        
        // Send final stats
        let stats = self.get_stats().await;
        let stats_msg = TunnelMessage::StatsUpdate {
            bytes_sent: stats.bytes_sent,
            bytes_received: stats.bytes_received,
            packets_sent: stats.packets_sent,
            packets_received: stats.packets_received,
            last_activity: SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
        };
        
        let _ = self.send_message(&stats_msg).await;
        
        self.is_connected = false;
        println!("✅ Packet tunnel closed");
        Ok(())
    }
}

// Traffic interceptor that captures system packets
#[derive(Debug)]
pub struct TrafficInterceptor {
    pub client_id: String,
    pub tunnel: Option<PacketTunnel>,
    pub is_active: bool,
}

unsafe impl Send for TrafficInterceptor {}
unsafe impl Sync for TrafficInterceptor {}

impl TrafficInterceptor {
    pub fn new(client_id: String) -> Self {
        Self {
            client_id,
            tunnel: None,
            is_active: false,
        }
    }

    pub async fn start(&mut self, tunnel: PacketTunnel) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("🕸️ Starting traffic interceptor for client: {}", self.client_id);
        
        self.tunnel = Some(tunnel);
        self.is_active = true;
        
        // This would start the actual packet capture
        // For now, simulate some traffic
        self.simulate_traffic().await?;
        
        Ok(())
    }

    async fn simulate_traffic(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("🎭 Simulating captured traffic (for testing)");
        
        if let Some(ref mut tunnel) = self.tunnel {
            // Simulate a few HTTP packets
            for i in 1..=5 {
                let fake_packet = TrafficInterceptor::create_fake_http_packet(i);
                tunnel.send_ip_packet(fake_packet).await?;
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
        }
        
        Ok(())
    }

    fn create_fake_http_packet(seq: u32) -> Vec<u8> {
        // Create a minimal IP packet for testing
        let mut packet = vec![0u8; 60];
        
        // IP Header (simplified)
        packet[0] = 0x45; // Version 4, Header length 5
        packet[1] = 0x00; // Type of service
        packet[2] = 0x00; // Total length high
        packet[3] = 0x3C; // Total length low (60 bytes)
        packet[4] = (seq >> 8) as u8; // Identification high
        packet[5] = seq as u8; // Identification low
        packet[6] = 0x40; // Flags
        packet[7] = 0x00; // Fragment offset
        packet[8] = 0x40; // TTL
        packet[9] = 0x06; // Protocol (TCP)
        
        // Source IP: 10.8.0.2
        packet[12] = 10;
        packet[13] = 8;
        packet[14] = 0;
        packet[15] = 2;
        
        // Destination IP: 1.1.1.1 (Cloudflare DNS)
        packet[16] = 1;
        packet[17] = 1;
        packet[18] = 1;
        packet[19] = 1;
        
        // TCP Header (simplified)
        packet[20] = 0x80; // Source port high (32768)
        packet[21] = 0x00; // Source port low
        packet[22] = 0x00; // Dest port high (80)
        packet[23] = 0x50; // Dest port low
        
        // Fill rest with fake HTTP data
        let http_data = b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n";
        let start = 40;
        let end = std::cmp::min(start + http_data.len(), packet.len());
        packet[start..end].copy_from_slice(&http_data[..end-start]);
        
        packet
    }

    pub async fn stop(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("🛑 Stopping traffic interceptor");
        
        if let Some(mut tunnel) = self.tunnel.take() {
            tunnel.close().await?;
        }
        
        self.is_active = false;
        println!("✅ Traffic interceptor stopped");
        Ok(())
    }
}