pub mod manager;
pub mod packet;
pub mod routing;
pub mod interface;

use crate::{Result, IronTunnelError};
use tokio::net::{TcpStream, TcpListener};
use tokio::sync::{mpsc, Mutex};
use tokio::time::{Duration, Instant};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::collections::HashMap;
#[cfg(feature = "tun")]
use tun::{TunPacket, Device, Configuration};
use pnet::packet::ip::IpNextHeaderProtocols;
use pnet::packet::ipv4::{Ipv4Packet, MutableIpv4Packet};
use pnet::packet::tcp::{TcpPacket, MutableTcpPacket};
use pnet::packet::udp::{UdpPacket, MutableUdpPacket};
use bytes::{Bytes, BytesMut};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelConfig {
    pub interface_name: String,
    pub local_ip: IpAddr,
    pub remote_ip: IpAddr,
    pub subnet_mask: u8,
    pub mtu: u16,
    pub enable_compression: bool,
    pub enable_kill_switch: bool,
    pub dns_servers: Vec<IpAddr>,
}

impl Default for TunnelConfig {
    fn default() -> Self {
        Self {
            interface_name: "irontunnel0".to_string(),
            local_ip: IpAddr::V4(Ipv4Addr::new(10, 8, 0, 2)),
            remote_ip: IpAddr::V4(Ipv4Addr::new(10, 8, 0, 1)),
            subnet_mask: 24,
            mtu: 1500,
            enable_compression: true,
            enable_kill_switch: true,
            dns_servers: vec![
                IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)),
                IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
            ],
        }
    }
}

#[derive(Debug, Clone)]
pub struct TunnelPacket {
    pub data: Bytes,
    pub timestamp: Instant,
    pub source: SocketAddr,
    pub destination: SocketAddr,
    pub protocol: u8,
}

#[derive(Debug, Clone)]
pub struct ConnectionInfo {
    pub id: u64,
    pub peer_addr: SocketAddr,
    pub connected_at: Instant,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub last_activity: Instant,
}

pub struct TunnelManager {
    config: TunnelConfig,
    #[cfg(feature = "tun")]
    device: Option<Arc<Mutex<Box<dyn Device + Send>>>>,
    #[cfg(not(feature = "tun"))]
    device: Option<()>,
    connections: Arc<Mutex<HashMap<u64, ConnectionInfo>>>,
    packet_tx: mpsc::UnboundedSender<TunnelPacket>,
    packet_rx: Arc<Mutex<mpsc::UnboundedReceiver<TunnelPacket>>>,
    router: Arc<PacketRouter>,
    kill_switch_active: Arc<Mutex<bool>>,
    next_connection_id: Arc<Mutex<u64>>,
}

impl TunnelManager {
    pub fn new() -> Result<Self> {
        let config = TunnelConfig::default();
        let (packet_tx, packet_rx) = mpsc::unbounded_channel();
        let router = Arc::new(PacketRouter::new());
        
        Ok(Self {
            config,
            device: None,
            connections: Arc::new(Mutex::new(HashMap::new())),
            packet_tx,
            packet_rx: Arc::new(Mutex::new(packet_rx)),
            router,
            kill_switch_active: Arc::new(Mutex::new(false)),
            next_connection_id: Arc::new(Mutex::new(1)),
        })
    }
    
    pub fn with_config(config: TunnelConfig) -> Result<Self> {
        let (packet_tx, packet_rx) = mpsc::unbounded_channel();
        let router = Arc::new(PacketRouter::new());
        
        Ok(Self {
            config,
            device: None,
            connections: Arc::new(Mutex::new(HashMap::new())),
            packet_tx,
            packet_rx: Arc::new(Mutex::new(packet_rx)),
            router,
            kill_switch_active: Arc::new(Mutex::new(false)),
            next_connection_id: Arc::new(Mutex::new(1)),
        })
    }
    
    pub async fn create_interface(&mut self) -> Result<()> {
        #[cfg(feature = "tun")]
        {
            let mut config = Configuration::default();
            config
                .name(&self.config.interface_name)
                .address(self.config.local_ip)
                .netmask(self.calculate_netmask())
                .mtu(self.config.mtu as i32)
                .up();
            
            let device = tun::create(&config)
                .map_err(|e| IronTunnelError::Tunnel(format!("Failed to create TUN interface: {}", e)))?;
            
            self.device = Some(Arc::new(Mutex::new(Box::new(device))));
        }
        
        #[cfg(not(feature = "tun"))]
        {
            tracing::warn!("TUN interface creation skipped - feature not enabled");
            self.device = Some(());
        }
        
        if self.config.enable_kill_switch {
            self.setup_kill_switch().await?;
        }
        
        Ok(())
    }
    
    pub async fn start_packet_processing(&self) -> Result<()> {
        #[cfg(feature = "tun")]
        {
            let device = self.device.as_ref()
                .ok_or_else(|| IronTunnelError::Tunnel("TUN interface not created".to_string()))?
                .clone();
            
            let packet_tx = self.packet_tx.clone();
            let router = self.router.clone();
            let kill_switch = self.kill_switch_active.clone();
            
            tokio::spawn(async move {
                loop {
                    if *kill_switch.lock().await {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                    
                    let mut device_guard = device.lock().await;
                    let mut buf = [0u8; 1500];
                    
                    match device_guard.recv(&mut buf) {
                        Ok(size) => {
                            if let Ok(packet) = Self::parse_packet(&buf[..size]) {
                                if let Err(e) = packet_tx.send(packet) {
                                    tracing::error!("Failed to send packet to processing queue: {}", e);
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!("Failed to receive packet from TUN interface: {}", e);
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                    }
                }
            });
        }
        
        #[cfg(not(feature = "tun"))]
        {
            tracing::warn!("Packet processing skipped - TUN feature not enabled");
        }
        
        Ok(())
    }
    
    pub async fn send_packet(&self, data: &[u8]) -> Result<()> {
        if *self.kill_switch_active.lock().await {
            return Err(IronTunnelError::Tunnel("Kill switch is active".to_string()));
        }
        
        #[cfg(feature = "tun")]
        {
            let device = self.device.as_ref()
                .ok_or_else(|| IronTunnelError::Tunnel("TUN interface not created".to_string()))?;
            
            let mut device_guard = device.lock().await;
            device_guard.send(data)
                .map_err(|e| IronTunnelError::Tunnel(format!("Failed to send packet: {}", e)))?;
        }
        
        #[cfg(not(feature = "tun"))]
        {
            tracing::debug!("Mock packet send: {} bytes", data.len());
        }
        
        Ok(())
    }
    
    pub async fn add_connection(&self, peer_addr: SocketAddr) -> Result<u64> {
        let mut next_id = self.next_connection_id.lock().await;
        let connection_id = *next_id;
        *next_id += 1;
        
        let connection_info = ConnectionInfo {
            id: connection_id,
            peer_addr,
            connected_at: Instant::now(),
            bytes_sent: 0,
            bytes_received: 0,
            last_activity: Instant::now(),
        };
        
        let mut connections = self.connections.lock().await;
        connections.insert(connection_id, connection_info);
        
        Ok(connection_id)
    }
    
    pub async fn remove_connection(&self, connection_id: u64) -> Result<()> {
        let mut connections = self.connections.lock().await;
        connections.remove(&connection_id);
        Ok(())
    }
    
    pub async fn get_connection_stats(&self) -> Result<Vec<ConnectionInfo>> {
        let connections = self.connections.lock().await;
        Ok(connections.values().cloned().collect())
    }
    
    pub async fn activate_kill_switch(&self) -> Result<()> {
        *self.kill_switch_active.lock().await = true;
        
        #[cfg(target_os = "linux")]
        {
            self.block_all_traffic().await?;
        }
        
        Ok(())
    }
    
    pub async fn deactivate_kill_switch(&self) -> Result<()> {
        *self.kill_switch_active.lock().await = false;
        
        #[cfg(target_os = "linux")]
        {
            self.restore_traffic().await?;
        }
        
        Ok(())
    }
    
    fn parse_packet(data: &[u8]) -> Result<TunnelPacket> {
        if data.len() < 20 {
            return Err(IronTunnelError::Tunnel("Packet too short".to_string()));
        }
        
        let ip_packet = Ipv4Packet::new(data)
            .ok_or_else(|| IronTunnelError::Tunnel("Invalid IPv4 packet".to_string()))?;
        
        let source = SocketAddr::new(
            IpAddr::V4(ip_packet.get_source()),
            Self::extract_source_port(data, ip_packet.get_next_level_protocol())?,
        );
        
        let destination = SocketAddr::new(
            IpAddr::V4(ip_packet.get_destination()),
            Self::extract_dest_port(data, ip_packet.get_next_level_protocol())?,
        );
        
        Ok(TunnelPacket {
            data: Bytes::copy_from_slice(data),
            timestamp: Instant::now(),
            source,
            destination,
            protocol: ip_packet.get_next_level_protocol().0,
        })
    }
    
    fn extract_source_port(data: &[u8], protocol: IpNextHeaderProtocols) -> Result<u16> {
        match protocol {
            IpNextHeaderProtocols::Tcp => {
                let tcp_packet = TcpPacket::new(&data[20..])
                    .ok_or_else(|| IronTunnelError::Tunnel("Invalid TCP packet".to_string()))?;
                Ok(tcp_packet.get_source())
            }
            IpNextHeaderProtocols::Udp => {
                let udp_packet = UdpPacket::new(&data[20..])
                    .ok_or_else(|| IronTunnelError::Tunnel("Invalid UDP packet".to_string()))?;
                Ok(udp_packet.get_source())
            }
            _ => Ok(0),
        }
    }
    
    fn extract_dest_port(data: &[u8], protocol: IpNextHeaderProtocols) -> Result<u16> {
        match protocol {
            IpNextHeaderProtocols::Tcp => {
                let tcp_packet = TcpPacket::new(&data[20..])
                    .ok_or_else(|| IronTunnelError::Tunnel("Invalid TCP packet".to_string()))?;
                Ok(tcp_packet.get_destination())
            }
            IpNextHeaderProtocols::Udp => {
                let udp_packet = UdpPacket::new(&data[20..])
                    .ok_or_else(|| IronTunnelError::Tunnel("Invalid UDP packet".to_string()))?;
                Ok(udp_packet.get_destination())
            }
            _ => Ok(0),
        }
    }
    
    fn calculate_netmask(&self) -> IpAddr {
        let mask_bits = self.config.subnet_mask;
        let mask = !((1u32 << (32 - mask_bits)) - 1);
        IpAddr::V4(Ipv4Addr::from(mask))
    }
    
    async fn setup_kill_switch(&self) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            use tokio::process::Command;
            
            let output = Command::new("iptables")
                .args(&["-L", "OUTPUT", "-n"])
                .output()
                .await
                .map_err(|e| IronTunnelError::Tunnel(format!("Failed to check iptables: {}", e)))?;
            
            if !output.status.success() {
                return Err(IronTunnelError::Tunnel("iptables not available".to_string()));
            }
        }
        
        Ok(())
    }
    
    #[cfg(target_os = "linux")]
    async fn block_all_traffic(&self) -> Result<()> {
        use tokio::process::Command;
        
        Command::new("iptables")
            .args(&["-A", "OUTPUT", "!", "-o", &self.config.interface_name, "-j", "DROP"])
            .output()
            .await
            .map_err(|e| IronTunnelError::Tunnel(format!("Failed to activate kill switch: {}", e)))?;
        
        Ok(())
    }
    
    #[cfg(target_os = "linux")]
    async fn restore_traffic(&self) -> Result<()> {
        use tokio::process::Command;
        
        Command::new("iptables")
            .args(&["-D", "OUTPUT", "!", "-o", &self.config.interface_name, "-j", "DROP"])
            .output()
            .await
            .map_err(|e| IronTunnelError::Tunnel(format!("Failed to deactivate kill switch: {}", e)))?;
        
        Ok(())
    }
}

pub struct PacketRouter {
    routes: Arc<Mutex<HashMap<IpAddr, SocketAddr>>>,
}

impl PacketRouter {
    pub fn new() -> Self {
        Self {
            routes: Arc::new(Mutex::new(HashMap::new())),
        }
    }
    
    pub async fn add_route(&self, destination: IpAddr, gateway: SocketAddr) -> Result<()> {
        let mut routes = self.routes.lock().await;
        routes.insert(destination, gateway);
        Ok(())
    }
    
    pub async fn remove_route(&self, destination: &IpAddr) -> Result<()> {
        let mut routes = self.routes.lock().await;
        routes.remove(destination);
        Ok(())
    }
    
    pub async fn route_packet(&self, packet: &TunnelPacket) -> Result<Option<SocketAddr>> {
        let routes = self.routes.lock().await;
        Ok(routes.get(&packet.destination.ip()).copied())
    }
    
    pub async fn get_routes(&self) -> Result<HashMap<IpAddr, SocketAddr>> {
        let routes = self.routes.lock().await;
        Ok(routes.clone())
    }
}