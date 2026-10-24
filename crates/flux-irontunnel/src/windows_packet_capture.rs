// Windows Packet Capture using Raw Sockets
// This module implements real packet capture on Windows without external dependencies

use std::net::Ipv4Addr;
use std::sync::Arc;
use tokio::sync::Mutex;
use crate::packet_tunnel::PacketTunnel;
use crate::packet_injector::PacketInjector;

#[cfg(target_os = "windows")]
use winapi::{
    shared::{
        minwindef::{DWORD},
        ws2def::{AF_INET, SOCK_RAW, IPPROTO_RAW, SOCKADDR, SOCKADDR_IN},
    },
    um::{
        winsock2::{
            socket, bind, ioctlsocket, recv, closesocket,
            WSAStartup, WSACleanup, WSAGetLastError,
            SOCKET, FIONBIO,
            WSADATA, INVALID_SOCKET,
        },
    },
};

pub struct WindowsPacketCapture {
    #[cfg(target_os = "windows")]
    raw_socket: Option<SOCKET>,
    #[cfg(not(target_os = "windows"))]
    raw_socket: Option<i32>,
    capture_active: bool,
    packet_tunnel: Option<Arc<Mutex<PacketTunnel>>>,
    packet_injector: Option<Arc<Mutex<PacketInjector>>>,
}

#[cfg(target_os = "windows")]
unsafe impl Send for WindowsPacketCapture {}
#[cfg(target_os = "windows")]
unsafe impl Sync for WindowsPacketCapture {}

impl WindowsPacketCapture {
    pub fn new() -> Self {
        Self {
            raw_socket: None,
            capture_active: false,
            packet_tunnel: None,
            packet_injector: None,
        }
    }

    #[cfg(target_os = "windows")]
    pub async fn start_capture(&mut self, packet_tunnel: Arc<Mutex<PacketTunnel>>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("🔍 Starting Windows packet capture with raw sockets");
        
        self.packet_tunnel = Some(packet_tunnel);
        
        unsafe {
            // Initialize Winsock
            let mut wsa_data: WSADATA = std::mem::zeroed();
            let result = WSAStartup(0x0202, &mut wsa_data);
            if result != 0 {
                return Err(format!("WSAStartup failed: {}", result).into());
            }

            // Create raw socket for IP packets
            let sock = socket(AF_INET as i32, SOCK_RAW as i32, IPPROTO_RAW as i32);
            if sock == INVALID_SOCKET {
                let error = WSAGetLastError();
                WSACleanup();
                return Err(format!("Failed to create raw socket (error {}). Note: Requires admin privileges.", error).into());
            }

            // Bind to local interface
            let mut addr: SOCKADDR_IN = std::mem::zeroed();
            addr.sin_family = AF_INET as u16;
            addr.sin_port = 0;
            // Set to INADDR_ANY (0.0.0.0)
            unsafe {
                *addr.sin_addr.S_un.S_addr_mut() = 0; // Any interface
            }
            
            let bind_result = bind(sock, &addr as *const _ as *const SOCKADDR, std::mem::size_of::<SOCKADDR_IN>() as i32);
            if bind_result != 0 {
                let error = WSAGetLastError();
                closesocket(sock);
                WSACleanup();
                return Err(format!("Bind failed: {}", error).into());
            }

            // Set socket to receive all packets (promiscuous mode)
            let mut mode: DWORD = 1; // RCVALL_ON
            let opt_result = ioctlsocket(sock, SIO_RCVALL as i32, &mut mode);
            if opt_result != 0 {
                let error = WSAGetLastError();
                closesocket(sock);
                WSACleanup();
                return Err(format!("Failed to set promiscuous mode: {}", error).into());
            }

            // Set non-blocking mode
            let mut nonblocking: u32 = 1;
            ioctlsocket(sock, FIONBIO, &mut nonblocking);

            self.raw_socket = Some(sock);
            self.capture_active = true;

            // Spawn packet capture task
            self.spawn_capture_task(sock).await;
        }

        println!("✅ Windows packet capture started");
        Ok(())
    }

    #[cfg(target_os = "windows")]
    async fn spawn_capture_task(&self, sock: SOCKET) {
        let packet_tunnel = self.packet_tunnel.clone();
        
        tokio::spawn(async move {
            let mut buffer = vec![0u8; 65536]; // Max IP packet size
            
            loop {
                unsafe {
                    let received = recv(sock, buffer.as_mut_ptr() as *mut _, buffer.len() as i32, 0);
                    
                    if received > 0 {
                        let packet_data = buffer[..received as usize].to_vec();
                        
                        // Check if this is outbound traffic that should go through VPN
                        if should_capture_packet(&packet_data) {
                            println!("📦 Captured packet: {} bytes", received);
                            
                            // Forward to VPN tunnel
                            if let Some(ref tunnel) = packet_tunnel {
                                let mut tunnel_guard = tunnel.lock().await;
                                if let Err(e) = tunnel_guard.send_ip_packet(packet_data).await {
                                    println!("❌ Failed to forward packet: {}", e);
                                }
                            }
                        }
                    } else if received < 0 {
                        let error = WSAGetLastError();
                        if error != 10035 { // WSAEWOULDBLOCK
                            println!("❌ Recv error: {}", error);
                            break;
                        }
                    }
                    
                    // Small delay to prevent CPU spinning
                    tokio::time::sleep(tokio::time::Duration::from_micros(100)).await;
                }
            }
            
            println!("📦 Packet capture task ended");
        });
    }

    #[cfg(not(target_os = "windows"))]
    pub async fn start_capture(&mut self, _packet_tunnel: Arc<Mutex<PacketTunnel>>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Err("Raw socket capture only supported on Windows".into())
    }

    pub async fn stop_capture(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("🛑 Stopping packet capture");
        
        #[cfg(target_os = "windows")]
        unsafe {
            if let Some(sock) = self.raw_socket.take() {
                closesocket(sock);
                WSACleanup();
            }
        }
        
        self.capture_active = false;
        self.packet_tunnel = None;
        
        println!("✅ Packet capture stopped");
        Ok(())
    }
}

// Check if packet should be captured and sent through VPN
fn should_capture_packet(packet: &[u8]) -> bool {
    if packet.len() < 20 {
        return false; // Not a valid IP packet
    }
    
    // Parse IP header
    let version = (packet[0] >> 4) & 0xF;
    if version != 4 {
        return false; // Only handle IPv4 for now
    }
    
    // Get destination IP
    let dst_ip = Ipv4Addr::new(packet[16], packet[17], packet[18], packet[19]);
    
    // Don't capture local/VPN traffic to prevent loops
    if dst_ip.is_loopback() || dst_ip.is_private() {
        return false;
    }
    
    // Capture all other outbound traffic
    true
}

// SIO_RCVALL constant for Windows
#[cfg(target_os = "windows")]
const SIO_RCVALL: DWORD = 0x98000001;

// Alternative implementation using WinPcap/Npcap if available
pub struct WinPcapCapture {
    device_name: String,
    capture_handle: Option<*mut std::ffi::c_void>,
}

impl WinPcapCapture {
    pub fn new(device_name: String) -> Self {
        Self {
            device_name,
            capture_handle: None,
        }
    }
    
    pub async fn start_capture(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // This would use WinPcap/Npcap APIs if available
        // For now, return an error indicating it's not implemented
        Err("WinPcap capture not implemented - use WindowsPacketCapture instead".into())
    }
}