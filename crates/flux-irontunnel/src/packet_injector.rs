// Packet Injector for Windows
// Handles bidirectional packet capture and injection

use std::sync::Arc;
use tokio::sync::Mutex;
use std::collections::HashMap;
use std::net::Ipv4Addr;

#[cfg(target_os = "windows")]
use winapi::{
    shared::{
        minwindef::DWORD,
        ws2def::{AF_INET, SOCK_RAW, IPPROTO_RAW, SOCKADDR, SOCKADDR_IN},
    },
    um::{
        winsock2::{
            socket, sendto, closesocket, WSAGetLastError, SOCKET, INVALID_SOCKET,
        },
    },
};

pub struct PacketInjector {
    #[cfg(target_os = "windows")]
    inject_socket: Option<SOCKET>,
    #[cfg(not(target_os = "windows"))]
    inject_socket: Option<i32>,
    // Track injected packets to avoid re-capturing them
    injected_packets: Arc<Mutex<HashMap<u64, std::time::Instant>>>,
}

impl PacketInjector {
    pub fn new() -> Self {
        Self {
            inject_socket: None,
            injected_packets: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[cfg(target_os = "windows")]
    pub async fn initialize(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        unsafe {
            // Create raw socket for injection
            let sock = socket(AF_INET as i32, SOCK_RAW as i32, IPPROTO_RAW as i32);
            if sock == INVALID_SOCKET {
                let error = WSAGetLastError();
                return Err(format!("Failed to create injection socket (error {})", error).into());
            }

            // Enable IP_HDRINCL
            let enable = 1i32;
            let result = winapi::um::winsock2::setsockopt(
                sock,
                winapi::shared::ws2def::IPPROTO_IP as i32,
                winapi::shared::ws2ipdef::IP_HDRINCL,
                &enable as *const _ as *const i8,
                std::mem::size_of::<i32>() as i32,
            );

            if result != 0 {
                closesocket(sock);
                return Err("Failed to set IP_HDRINCL".into());
            }

            self.inject_socket = Some(sock);
        }

        // Start cleanup task for injected packet tracking
        self.start_cleanup_task();

        println!("✅ Packet injector initialized");
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    pub async fn initialize(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Linux implementation would use raw sockets similarly
        Err("Packet injection not implemented for this platform".into())
    }

    // Inject packet back into the network stack
    pub async fn inject_packet(&self, packet_data: &[u8]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if packet_data.len() < 20 {
            return Err("Packet too small".into());
        }

        // Calculate packet hash to track it
        let packet_hash = self.calculate_packet_hash(packet_data);
        
        // Mark as injected to avoid re-capture
        {
            let mut injected = self.injected_packets.lock().await;
            injected.insert(packet_hash, std::time::Instant::now());
        }

        #[cfg(target_os = "windows")]
        {
            self.inject_windows_packet(packet_data).await?;
        }

        #[cfg(not(target_os = "windows"))]
        {
            println!("📥 Would inject {} bytes (not implemented)", packet_data.len());
        }

        Ok(())
    }

    #[cfg(target_os = "windows")]
    async fn inject_windows_packet(&self, packet_data: &[u8]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Some(sock) = self.inject_socket {
            unsafe {
                // Get destination from packet
                let dst_ip = Ipv4Addr::new(
                    packet_data[16], packet_data[17], 
                    packet_data[18], packet_data[19]
                );

                let mut addr: SOCKADDR_IN = std::mem::zeroed();
                addr.sin_family = AF_INET as u16;
                addr.sin_port = 0;
                *addr.sin_addr.S_un.S_addr_mut() = u32::from(dst_ip).to_be();

                let sent = sendto(
                    sock,
                    packet_data.as_ptr() as *const i8,
                    packet_data.len() as i32,
                    0,
                    &addr as *const _ as *const SOCKADDR,
                    std::mem::size_of::<SOCKADDR_IN>() as i32,
                );

                if sent < 0 {
                    let error = WSAGetLastError();
                    return Err(format!("Failed to inject packet (error {})", error).into());
                }

                println!("💉 Injected {} bytes to {}", sent, dst_ip);
            }
        }

        Ok(())
    }

    // Check if a packet was recently injected (to avoid loops)
    pub async fn is_injected_packet(&self, packet_data: &[u8]) -> bool {
        let hash = self.calculate_packet_hash(packet_data);
        let injected = self.injected_packets.lock().await;
        
        if let Some(timestamp) = injected.get(&hash) {
            // Consider packets injected within last 100ms
            timestamp.elapsed() < std::time::Duration::from_millis(100)
        } else {
            false
        }
    }

    fn calculate_packet_hash(&self, packet: &[u8]) -> u64 {
        // Simple hash based on key packet fields
        if packet.len() >= 20 {
            let mut hash = 0u64;
            hash ^= (packet[9] as u64) << 56;  // Protocol
            hash ^= u32::from_be_bytes([packet[12], packet[13], packet[14], packet[15]]) as u64; // Src IP
            hash ^= (u32::from_be_bytes([packet[16], packet[17], packet[18], packet[19]]) as u64) << 32; // Dst IP
            
            // Include ports for TCP/UDP
            if packet.len() >= 24 && (packet[9] == 6 || packet[9] == 17) {
                let ihl = (packet[0] & 0x0F) as usize * 4;
                if packet.len() >= ihl + 4 {
                    hash ^= (u16::from_be_bytes([packet[ihl], packet[ihl + 1]]) as u64) << 16; // Src port
                    hash ^= u16::from_be_bytes([packet[ihl + 2], packet[ihl + 3]]) as u64; // Dst port
                }
            }
            
            hash
        } else {
            0
        }
    }

    fn start_cleanup_task(&self) {
        let injected = self.injected_packets.clone();
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
            
            loop {
                interval.tick().await;
                
                let mut map = injected.lock().await;
                let now = std::time::Instant::now();
                
                // Remove entries older than 1 second
                map.retain(|_, timestamp| {
                    now.duration_since(*timestamp) < std::time::Duration::from_secs(1)
                });
            }
        });
    }

    pub async fn shutdown(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        #[cfg(target_os = "windows")]
        {
            if let Some(sock) = self.inject_socket.take() {
                unsafe {
                    closesocket(sock);
                }
            }
        }
        
        println!("🛑 Packet injector shutdown");
        Ok(())
    }
}

// Helper to recalculate IP checksum after modification
pub fn recalculate_ip_checksum(packet: &mut [u8]) {
    if packet.len() < 20 {
        return;
    }
    
    // Clear existing checksum
    packet[10] = 0;
    packet[11] = 0;
    
    // Calculate IP header length
    let ihl = (packet[0] & 0x0F) as usize * 4;
    
    // Calculate checksum
    let mut sum = 0u32;
    for i in (0..ihl).step_by(2) {
        let word = u16::from_be_bytes([packet[i], packet[i + 1]]) as u32;
        sum += word;
    }
    
    // Add carry bits
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    
    // One's complement
    let checksum = !sum as u16;
    packet[10] = (checksum >> 8) as u8;
    packet[11] = checksum as u8;
}

// Helper to recalculate TCP checksum
pub fn recalculate_tcp_checksum(packet: &mut [u8]) {
    if packet.len() < 20 || packet[9] != 6 {
        return;
    }
    
    let ihl = (packet[0] & 0x0F) as usize * 4;
    if packet.len() < ihl + 20 {
        return;
    }
    
    // Clear existing checksum
    packet[ihl + 16] = 0;
    packet[ihl + 17] = 0;
    
    // Pseudo header
    let mut sum = 0u32;
    
    // Source IP
    sum += u16::from_be_bytes([packet[12], packet[13]]) as u32;
    sum += u16::from_be_bytes([packet[14], packet[15]]) as u32;
    
    // Destination IP
    sum += u16::from_be_bytes([packet[16], packet[17]]) as u32;
    sum += u16::from_be_bytes([packet[18], packet[19]]) as u32;
    
    // Protocol + TCP length
    sum += 6u32; // TCP protocol
    sum += (packet.len() - ihl) as u32;
    
    // TCP header + data
    let tcp_data = &packet[ihl..];
    for i in (0..tcp_data.len()).step_by(2) {
        if i + 1 < tcp_data.len() {
            sum += u16::from_be_bytes([tcp_data[i], tcp_data[i + 1]]) as u32;
        } else {
            sum += (tcp_data[i] as u32) << 8;
        }
    }
    
    // Add carry bits
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    
    // One's complement
    let checksum = !sum as u16;
    packet[ihl + 16] = (checksum >> 8) as u8;
    packet[ihl + 17] = checksum as u8;
}