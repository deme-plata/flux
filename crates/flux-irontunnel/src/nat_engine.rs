// NAT Engine for IronTunnel Server
// Handles packet forwarding, NAT translation, and connection tracking

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::sync::RwLock;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use std::process::Command;

// NAT table entry for connection tracking
#[derive(Debug, Clone)]
pub struct NatEntry {
    pub client_id: String,
    pub client_ip: Ipv4Addr,
    pub original_src: SocketAddr,
    pub translated_src: SocketAddr,
    pub destination: SocketAddr,
    pub protocol: IpProtocol,
    pub created: Instant,
    pub last_activity: Instant,
    pub packets_sent: u64,
    pub packets_received: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IpProtocol {
    Tcp,
    Udp,
    Icmp,
    Other(u8),
}

impl From<u8> for IpProtocol {
    fn from(proto: u8) -> Self {
        match proto {
            6 => IpProtocol::Tcp,
            17 => IpProtocol::Udp,
            1 => IpProtocol::Icmp,
            other => IpProtocol::Other(other),
        }
    }
}

// NAT engine manages packet forwarding and translation
pub struct NatEngine {
    // Connection tracking table
    nat_table: Arc<RwLock<HashMap<String, NatEntry>>>,
    // Client IP assignments
    client_ips: Arc<RwLock<HashMap<String, Ipv4Addr>>>,
    // Next available IP for assignment
    next_client_ip: Arc<RwLock<u32>>,
    // Server's public IP for NAT
    public_ip: Ipv4Addr,
    // DNS servers to use
    dns_servers: Vec<Ipv4Addr>,
    // Raw socket for sending packets
    #[cfg(target_os = "linux")]
    raw_socket: Option<i32>,
}

impl NatEngine {
    pub async fn new(public_ip: Ipv4Addr) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let mut engine = Self {
            nat_table: Arc::new(RwLock::new(HashMap::new())),
            client_ips: Arc::new(RwLock::new(HashMap::new())),
            next_client_ip: Arc::new(RwLock::new(2)), // Start at 10.8.0.2
            public_ip,
            dns_servers: vec![
                Ipv4Addr::new(1, 1, 1, 1),    // Cloudflare
                Ipv4Addr::new(8, 8, 8, 8),    // Google
            ],
            #[cfg(target_os = "linux")]
            raw_socket: None,
        };

        // Initialize raw socket for Linux
        #[cfg(target_os = "linux")]
        {
            engine.init_raw_socket().await?;
            engine.setup_ip_forwarding().await?;
            engine.setup_nat_rules().await?;
        }

        // Start NAT table cleanup task
        engine.start_cleanup_task();

        Ok(engine)
    }

    #[cfg(target_os = "linux")]
    async fn init_raw_socket(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use libc::{socket, AF_INET, SOCK_RAW, IPPROTO_RAW};
        
        unsafe {
            let sock = socket(AF_INET, SOCK_RAW, IPPROTO_RAW);
            if sock < 0 {
                return Err("Failed to create raw socket. Run as root.".into());
            }
            
            // Enable IP_HDRINCL to include IP header
            let enable = 1i32;
            let result = libc::setsockopt(
                sock,
                libc::IPPROTO_IP,
                libc::IP_HDRINCL,
                &enable as *const _ as *const libc::c_void,
                std::mem::size_of::<i32>() as u32,
            );
            
            if result < 0 {
                libc::close(sock);
                return Err("Failed to set IP_HDRINCL".into());
            }
            
            self.raw_socket = Some(sock);
        }
        
        println!("✅ Raw socket initialized for packet forwarding");
        Ok(())
    }

    #[cfg(target_os = "linux")]
    async fn setup_ip_forwarding(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Enable IP forwarding
        Command::new("sysctl")
            .args(&["-w", "net.ipv4.ip_forward=1"])
            .output()?;
            
        println!("✅ IP forwarding enabled");
        Ok(())
    }

    #[cfg(target_os = "linux")]
    async fn setup_nat_rules(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Add iptables rules for NAT
        let commands = vec![
            // Enable NAT for VPN subnet
            vec!["iptables", "-t", "nat", "-A", "POSTROUTING", "-s", "10.8.0.0/24", "-j", "MASQUERADE"],
            // Allow forwarding from VPN
            vec!["iptables", "-A", "FORWARD", "-s", "10.8.0.0/24", "-j", "ACCEPT"],
            vec!["iptables", "-A", "FORWARD", "-d", "10.8.0.0/24", "-j", "ACCEPT"],
        ];
        
        for cmd_args in commands {
            let output = Command::new(cmd_args[0])
                .args(&cmd_args[1..])
                .output()?;
                
            if !output.status.success() {
                println!("⚠️ Warning: {} failed: {}", cmd_args[0], String::from_utf8_lossy(&output.stderr));
            }
        }
        
        println!("✅ NAT rules configured");
        Ok(())
    }

    // Assign IP address to client
    pub async fn assign_client_ip(&self, client_id: &str) -> Ipv4Addr {
        let mut ips = self.client_ips.write().await;
        
        if let Some(&ip) = ips.get(client_id) {
            return ip;
        }
        
        let mut next_ip = self.next_client_ip.write().await;
        let ip = Ipv4Addr::new(10, 8, 0, *next_ip as u8);
        *next_ip += 1;
        
        ips.insert(client_id.to_string(), ip);
        println!("📍 Assigned IP {} to client {}", ip, client_id);
        
        ip
    }

    // Forward packet from client to internet
    pub async fn forward_to_internet(
        &self, 
        client_id: &str,
        packet_data: &[u8]
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if packet_data.len() < 20 {
            return Err("Packet too small".into());
        }

        // Parse IP header
        let version = (packet_data[0] >> 4) & 0xF;
        if version != 4 {
            return Err("Only IPv4 supported".into());
        }

        let protocol = packet_data[9];
        let src_ip = Ipv4Addr::new(packet_data[12], packet_data[13], packet_data[14], packet_data[15]);
        let dst_ip = Ipv4Addr::new(packet_data[16], packet_data[17], packet_data[18], packet_data[19]);

        // Special handling for DNS
        if protocol == 17 && dst_ip == Ipv4Addr::new(10, 8, 0, 1) {
            return self.handle_dns_query(client_id, packet_data).await;
        }

        // Create NAT entry
        let nat_key = format!("{}-{}-{}-{}", client_id, src_ip, dst_ip, protocol);
        let entry = NatEntry {
            client_id: client_id.to_string(),
            client_ip: src_ip,
            original_src: SocketAddr::new(IpAddr::V4(src_ip), 0),
            translated_src: SocketAddr::new(IpAddr::V4(self.public_ip), 0),
            destination: SocketAddr::new(IpAddr::V4(dst_ip), 0),
            protocol: protocol.into(),
            created: Instant::now(),
            last_activity: Instant::now(),
            packets_sent: 1,
            packets_received: 0,
            bytes_sent: packet_data.len() as u64,
            bytes_received: 0,
        };

        // Store NAT entry
        {
            let mut table = self.nat_table.write().await;
            table.insert(nat_key.clone(), entry);
        }

        // Forward packet
        #[cfg(target_os = "linux")]
        {
            self.send_raw_packet(packet_data, dst_ip).await?;
        }

        #[cfg(not(target_os = "linux"))]
        {
            println!("📤 Would forward {} bytes to {} (not implemented on this platform)", 
                packet_data.len(), dst_ip);
        }

        Ok(())
    }

    #[cfg(target_os = "linux")]
    async fn send_raw_packet(&self, packet: &[u8], dest: Ipv4Addr) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use libc::{sockaddr_in, sendto, AF_INET};
        
        if let Some(sock) = self.raw_socket {
            unsafe {
                let mut addr: sockaddr_in = std::mem::zeroed();
                addr.sin_family = AF_INET as u16;
                addr.sin_addr.s_addr = u32::from(dest).to_be();
                
                let sent = sendto(
                    sock,
                    packet.as_ptr() as *const libc::c_void,
                    packet.len(),
                    0,
                    &addr as *const _ as *const libc::sockaddr,
                    std::mem::size_of::<sockaddr_in>() as u32,
                );
                
                if sent < 0 {
                    return Err("Failed to send packet".into());
                }
                
                println!("✅ Forwarded {} bytes to {}", sent, dest);
            }
        }
        
        Ok(())
    }

    // Handle DNS queries
    async fn handle_dns_query(
        &self,
        client_id: &str,
        packet_data: &[u8]
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("🌐 Handling DNS query from client {}", client_id);
        
        // Extract UDP payload (DNS query)
        if packet_data.len() < 28 {
            return Err("DNS packet too small".into());
        }
        
        let dns_query = &packet_data[28..]; // Skip IP + UDP headers
        
        // Forward to real DNS server
        let dns_socket = UdpSocket::bind("0.0.0.0:0").await?;
        let dns_server = SocketAddr::new(IpAddr::V4(self.dns_servers[0]), 53);
        
        dns_socket.send_to(dns_query, dns_server).await?;
        
        // Wait for response
        let mut buf = vec![0u8; 512];
        let (len, _) = dns_socket.recv_from(&mut buf).await?;
        
        println!("📥 DNS response: {} bytes", len);
        
        // TODO: Build response packet and send back to client
        
        Ok(())
    }

    // Handle incoming packet from internet
    pub async fn handle_return_packet(
        &self,
        packet_data: &[u8]
    ) -> Result<Option<(String, Vec<u8>)>, Box<dyn std::error::Error + Send + Sync>> {
        if packet_data.len() < 20 {
            return Ok(None);
        }

        let protocol = packet_data[9];
        let src_ip = Ipv4Addr::new(packet_data[12], packet_data[13], packet_data[14], packet_data[15]);
        let dst_ip = Ipv4Addr::new(packet_data[16], packet_data[17], packet_data[18], packet_data[19]);

        // Find matching NAT entry
        let table = self.nat_table.read().await;
        for (_, entry) in table.iter() {
            if entry.destination.ip() == IpAddr::V4(src_ip) && entry.protocol == protocol.into() {
                // Found matching connection
                let client_id = entry.client_id.clone();
                
                // Rewrite packet with client's IP
                let mut return_packet = packet_data.to_vec();
                return_packet[12..16].copy_from_slice(&entry.client_ip.octets());
                return_packet[16..20].copy_from_slice(&dst_ip.octets());
                
                // TODO: Recalculate checksums
                
                return Ok(Some((client_id, return_packet)));
            }
        }

        Ok(None)
    }

    // Cleanup old NAT entries
    fn start_cleanup_task(&self) {
        let nat_table = self.nat_table.clone();
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            
            loop {
                interval.tick().await;
                
                let mut table = nat_table.write().await;
                let now = Instant::now();
                
                table.retain(|_, entry| {
                    let age = now.duration_since(entry.last_activity);
                    match entry.protocol {
                        IpProtocol::Tcp => age < Duration::from_secs(300), // 5 min for TCP
                        IpProtocol::Udp => age < Duration::from_secs(60),  // 1 min for UDP
                        IpProtocol::Icmp => age < Duration::from_secs(30), // 30s for ICMP
                        _ => age < Duration::from_secs(120),                // 2 min default
                    }
                });
                
                println!("🧹 NAT table cleanup: {} active connections", table.len());
            }
        });
    }

    // Get statistics
    pub async fn get_stats(&self) -> HashMap<String, (u64, u64)> {
        let table = self.nat_table.read().await;
        let mut stats = HashMap::new();
        
        for (_, entry) in table.iter() {
            let counters = stats.entry(entry.client_id.clone()).or_insert((0, 0));
            counters.0 += entry.bytes_sent;
            counters.1 += entry.bytes_received;
        }
        
        stats
    }
}