// Traffic Forwarder - Bridges TUN interface with VPN tunnel
// This is the missing piece that actually routes traffic through the VPN

use crate::tun_interface::TunManager;
use crate::packet_tunnel::PacketTunnel;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct TrafficForwarder {
    pub tun_manager: Arc<Mutex<TunManager>>,
    pub packet_tunnel: Arc<Mutex<PacketTunnel>>,
    pub is_running: bool,
}

impl TrafficForwarder {
    pub fn new(tun_manager: TunManager, packet_tunnel: PacketTunnel) -> Self {
        Self {
            tun_manager: Arc::new(Mutex::new(tun_manager)),
            packet_tunnel: Arc::new(Mutex::new(packet_tunnel)),
            is_running: false,
        }
    }

    pub async fn start_forwarding(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("🔄 Starting traffic forwarder");
        self.is_running = true;

        let tun_manager = self.tun_manager.clone();
        let packet_tunnel = self.packet_tunnel.clone();

        // Spawn traffic forwarding task
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(10));
            
            loop {
                interval.tick().await;

                // Try to read packet from TUN interface
                let tun_guard = tun_manager.lock().await;
                
                if let Some(ref interface) = tun_guard.interface {
                    match interface.read_packet().await {
                        Ok(packet) if !packet.is_empty() => {
                            println!("📥 Received packet from TUN: {} bytes", packet.len());
                            drop(tun_guard); // Release TUN lock
                            
                            // Forward packet through VPN tunnel
                            let mut tunnel_guard = packet_tunnel.lock().await;
                            if let Err(e) = tunnel_guard.send_ip_packet(packet).await {
                                println!("❌ Failed to forward packet: {}", e);
                            }
                        }
                        Ok(_) => {
                            // No packet available, continue
                        }
                        Err(e) => {
                            println!("❌ TUN read error: {}", e);
                        }
                    }
                } else {
                    // No TUN interface, wait a bit
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                }
            }
        });

        println!("✅ Traffic forwarder started");
        Ok(())
    }

    pub fn stop_forwarding(&mut self) {
        println!("🛑 Stopping traffic forwarder");
        self.is_running = false;
    }
}

// Windows-specific traffic capture using WinDivert or similar
#[cfg(target_os = "windows")]
pub struct WindowsTrafficCapture {
    pub is_active: bool,
}

#[cfg(target_os = "windows")]
impl WindowsTrafficCapture {
    pub fn new() -> Self {
        Self {
            is_active: false,
        }
    }

    pub async fn start_capture(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("🕸️ Starting Windows traffic capture");
        
        // TODO: Implement actual WinDivert or similar packet capture
        // For now, simulate some traffic to show the analytics
        self.simulate_traffic().await?;
        
        self.is_active = true;
        Ok(())
    }

    async fn simulate_traffic(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("🎭 Simulating traffic for demonstration");
        
        // This would be replaced with actual packet capture
        // For now, just generate some fake traffic to show in analytics
        
        Ok(())
    }
}