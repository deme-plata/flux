// DNS Forwarder for IronTunnel
// Handles DNS queries and responses through the VPN tunnel

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::net::UdpSocket;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct DnsQuery {
    pub client_id: String,
    pub transaction_id: u16,
    pub client_addr: SocketAddr,
    pub query_time: Instant,
    pub query_data: Vec<u8>,
}

pub struct DnsForwarder {
    // DNS servers to forward to
    upstream_servers: Vec<SocketAddr>,
    // Pending DNS queries
    pending_queries: Arc<RwLock<HashMap<u16, DnsQuery>>>,
    // DNS cache
    cache: Arc<RwLock<HashMap<String, DnsCacheEntry>>>,
    // Local DNS socket
    dns_socket: Option<Arc<UdpSocket>>,
}

#[derive(Debug, Clone)]
struct DnsCacheEntry {
    response: Vec<u8>,
    expires: Instant,
}

impl DnsForwarder {
    pub async fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let forwarder = Self {
            upstream_servers: vec![
                SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), 53),     // Cloudflare
                SocketAddr::new(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), 53),     // Google
                SocketAddr::new(IpAddr::V4(Ipv4Addr::new(9, 9, 9, 9)), 53),     // Quad9
            ],
            pending_queries: Arc::new(RwLock::new(HashMap::new())),
            cache: Arc::new(RwLock::new(HashMap::new())),
            dns_socket: None,
        };

        Ok(forwarder)
    }

    pub async fn start(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Bind to local DNS port
        let socket = UdpSocket::bind("10.8.0.1:53").await?;
        self.dns_socket = Some(Arc::new(socket));
        
        // Start cache cleanup task
        self.start_cache_cleanup();
        
        println!("✅ DNS forwarder started on 10.8.0.1:53");
        Ok(())
    }

    // Handle DNS query from client
    pub async fn handle_dns_query(
        &self,
        client_id: &str,
        client_ip: Ipv4Addr,
        packet_data: &[u8]
    ) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        // Extract UDP payload (skip IP + UDP headers)
        if packet_data.len() < 28 {
            return Err("DNS packet too small".into());
        }
        
        let ip_header_len = ((packet_data[0] & 0x0F) * 4) as usize;
        let udp_start = ip_header_len;
        let dns_start = udp_start + 8; // UDP header is 8 bytes
        
        if packet_data.len() < dns_start + 12 {
            return Err("Invalid DNS query".into());
        }
        
        let dns_query = &packet_data[dns_start..];
        
        // Parse DNS header
        let transaction_id = u16::from_be_bytes([dns_query[0], dns_query[1]]);
        let query_name = self.extract_query_name(dns_query)?;
        
        println!("🌐 DNS query from {}: {} (ID: {})", client_id, query_name, transaction_id);
        
        // Check cache first
        if let Some(cached) = self.get_cached_response(&query_name).await {
            println!("💾 DNS cache hit for {}", query_name);
            return self.build_dns_response_packet(client_ip, transaction_id, &cached).await;
        }
        
        // Forward to upstream DNS
        let response = self.forward_dns_query(dns_query).await?;
        
        // Cache the response
        self.cache_dns_response(&query_name, &response).await;
        
        // Build response packet
        self.build_dns_response_packet(client_ip, transaction_id, &response).await
    }

    // Forward DNS query to upstream server
    async fn forward_dns_query(&self, query: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        
        // Try each upstream server
        for &server in &self.upstream_servers {
            match tokio::time::timeout(
                Duration::from_secs(2),
                self.query_upstream(&socket, query, server)
            ).await {
                Ok(Ok(response)) => return Ok(response),
                Ok(Err(e)) => println!("⚠️ DNS query to {} failed: {}", server, e),
                Err(_) => println!("⚠️ DNS query to {} timed out", server),
            }
        }
        
        Err("All DNS servers failed".into())
    }

    async fn query_upstream(
        &self,
        socket: &UdpSocket,
        query: &[u8],
        server: SocketAddr
    ) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        socket.send_to(query, server).await?;
        
        let mut buf = vec![0u8; 512];
        let (len, _) = socket.recv_from(&mut buf).await?;
        buf.truncate(len);
        
        Ok(buf)
    }

    // Extract domain name from DNS query
    fn extract_query_name(&self, dns_data: &[u8]) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        if dns_data.len() < 12 {
            return Err("DNS query too short".into());
        }
        
        let mut pos = 12; // Skip header
        let mut name = String::new();
        
        loop {
            if pos >= dns_data.len() {
                break;
            }
            
            let len = dns_data[pos] as usize;
            if len == 0 {
                break;
            }
            
            if !name.is_empty() {
                name.push('.');
            }
            
            pos += 1;
            if pos + len > dns_data.len() {
                break;
            }
            
            name.push_str(&String::from_utf8_lossy(&dns_data[pos..pos + len]));
            pos += len;
        }
        
        Ok(name)
    }

    // Build DNS response packet
    async fn build_dns_response_packet(
        &self,
        client_ip: Ipv4Addr,
        transaction_id: u16,
        dns_response: &[u8]
    ) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        // Build IP header
        let mut packet = vec![0u8; 20 + 8 + dns_response.len()]; // IP + UDP + DNS
        
        // IP header
        packet[0] = 0x45; // Version 4, IHL 5
        packet[1] = 0x00; // TOS
        let total_len = packet.len() as u16;
        packet[2] = (total_len >> 8) as u8;
        packet[3] = total_len as u8;
        packet[4] = 0x00; // ID
        packet[5] = 0x00;
        packet[6] = 0x40; // Flags (Don't Fragment)
        packet[7] = 0x00;
        packet[8] = 64;   // TTL
        packet[9] = 17;   // Protocol (UDP)
        // Checksum (calculate later)
        packet[12..16].copy_from_slice(&[10, 8, 0, 1]); // Src: 10.8.0.1
        packet[16..20].copy_from_slice(&client_ip.octets()); // Dst: client
        
        // UDP header
        packet[20] = 0x00; // Src port (53)
        packet[21] = 0x35;
        packet[22] = 0x00; // Dst port (will be set from original query)
        packet[23] = 0x35;
        let udp_len = (8 + dns_response.len()) as u16;
        packet[24] = (udp_len >> 8) as u8;
        packet[25] = udp_len as u8;
        // UDP checksum (optional for IPv4)
        packet[26] = 0x00;
        packet[27] = 0x00;
        
        // DNS response
        packet[28..].copy_from_slice(dns_response);
        
        // Fix transaction ID
        packet[28] = (transaction_id >> 8) as u8;
        packet[29] = transaction_id as u8;
        
        // Calculate IP checksum
        crate::packet_injector::recalculate_ip_checksum(&mut packet);
        
        Ok(packet)
    }

    // Cache DNS response
    async fn cache_dns_response(&self, domain: &str, response: &[u8]) {
        // Extract TTL from response (simplified - just use 5 minutes)
        let expires = Instant::now() + Duration::from_secs(300);
        
        let entry = DnsCacheEntry {
            response: response.to_vec(),
            expires,
        };
        
        let mut cache = self.cache.write().await;
        cache.insert(domain.to_string(), entry);
        
        println!("💾 Cached DNS response for {}", domain);
    }

    // Get cached response
    async fn get_cached_response(&self, domain: &str) -> Option<Vec<u8>> {
        let cache = self.cache.read().await;
        
        if let Some(entry) = cache.get(domain) {
            if entry.expires > Instant::now() {
                return Some(entry.response.clone());
            }
        }
        
        None
    }

    // Start cache cleanup task
    fn start_cache_cleanup(&self) {
        let cache = self.cache.clone();
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            
            loop {
                interval.tick().await;
                
                let mut cache_guard = cache.write().await;
                let now = Instant::now();
                
                cache_guard.retain(|domain, entry| {
                    if entry.expires > now {
                        true
                    } else {
                        println!("🗑️ Evicted DNS cache entry for {}", domain);
                        false
                    }
                });
            }
        });
    }

    // Handle DNS response from upstream
    pub async fn handle_dns_response(
        &self,
        transaction_id: u16,
        response_data: &[u8]
    ) -> Result<Option<(String, Vec<u8>)>, Box<dyn std::error::Error + Send + Sync>> {
        let queries = self.pending_queries.read().await;
        
        if let Some(query) = queries.get(&transaction_id) {
            println!("📨 DNS response for client {}", query.client_id);
            
            // Build response packet for client
            let client_ip = match query.client_addr.ip() {
                IpAddr::V4(ip) => ip,
                _ => return Err("IPv6 not supported".into()),
            };
            
            let packet = self.build_dns_response_packet(client_ip, transaction_id, response_data).await?;
            
            Ok(Some((query.client_id.clone(), packet)))
        } else {
            Ok(None)
        }
    }
}