// TUN Interface Manager for IronTunnel
// Handles creation and management of virtual network interfaces for traffic tunneling

use std::net::{IpAddr, Ipv4Addr};
use std::process::Command;

#[cfg(target_os = "linux")]
use std::os::unix::io::{AsRawFd, RawFd};

#[cfg(target_os = "windows")]
use std::os::windows::io::{AsRawHandle, RawHandle};

// Cross-platform TUN interface
#[derive(Debug)]
pub struct TunInterface {
    pub name: String,
    pub ip_address: IpAddr,
    pub netmask: IpAddr,
    pub mtu: u16,
    #[cfg(target_os = "linux")]
    pub fd: Option<RawFd>,
    #[cfg(target_os = "windows")]
    pub handle: Option<usize>, // Using usize instead of RawHandle for thread safety
    pub is_up: bool,
}

// Manual Send/Sync implementation for thread safety
unsafe impl Send for TunInterface {}
unsafe impl Sync for TunInterface {}

impl TunInterface {
    pub fn new(name: String, ip: IpAddr, netmask: IpAddr) -> Self {
        Self {
            name,
            ip_address: ip,
            netmask,
            mtu: 1500,
            #[cfg(target_os = "linux")]
            fd: None,
            #[cfg(target_os = "windows")]
            handle: None,
            is_up: false,
        }
    }

    #[cfg(target_os = "linux")]
    pub async fn create(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use std::fs::OpenOptions;
        use std::os::unix::fs::OpenOptionsExt;

        println!("🔧 Creating TUN interface: {}", self.name);

        // Open TUN device
        let tun_file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open("/dev/net/tun")?;

        self.fd = Some(tun_file.as_raw_fd());

        // Configure interface using ip command
        self.configure_interface().await?;
        
        println!("✅ TUN interface {} created successfully", self.name);
        self.is_up = true;
        Ok(())
    }

    #[cfg(target_os = "windows")]
    pub async fn create(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("🔧 Creating TUN interface on Windows: {}", self.name);
        
        // Use netsh to create a virtual interface
        let output = Command::new("netsh")
            .args(&[
                "interface", "ip", "set", "interface", 
                &self.name, "admin=enabled"
            ])
            .output()?;

        if !output.status.success() {
            // Try to create using route command
            self.create_windows_route_interface().await?;
        }

        self.configure_interface().await?;
        
        println!("✅ TUN interface {} created successfully", self.name);
        self.is_up = true;
        Ok(())
    }

    #[cfg(target_os = "windows")]
    async fn create_windows_route_interface(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Create a loopback-based tunnel interface for Windows
        let output = Command::new("route")
            .args(&["add", "0.0.0.0", "mask", "0.0.0.0", &self.ip_address.to_string()])
            .output()?;

        if !output.status.success() {
            return Err(format!("Failed to create Windows route interface: {}", 
                String::from_utf8_lossy(&output.stderr)).into());
        }

        Ok(())
    }

    async fn configure_interface(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        #[cfg(target_os = "linux")]
        {
            // Set IP address
            let output = Command::new("ip")
                .args(&[
                    "addr", "add", 
                    &format!("{}/{}", self.ip_address, self.get_prefix_length()),
                    "dev", &self.name
                ])
                .output()?;

            if !output.status.success() {
                return Err(format!("Failed to set IP address: {}", 
                    String::from_utf8_lossy(&output.stderr)).into());
            }

            // Bring interface up
            let output = Command::new("ip")
                .args(&["link", "set", "dev", &self.name, "up"])
                .output()?;

            if !output.status.success() {
                return Err(format!("Failed to bring interface up: {}", 
                    String::from_utf8_lossy(&output.stderr)).into());
            }

            // Set MTU
            let output = Command::new("ip")
                .args(&[
                    "link", "set", "dev", &self.name, 
                    "mtu", &self.mtu.to_string()
                ])
                .output()?;

            if !output.status.success() {
                println!("⚠️ Warning: Failed to set MTU: {}", 
                    String::from_utf8_lossy(&output.stderr));
            }
        }

        #[cfg(target_os = "windows")]
        {
            // Configure Windows interface using netsh
            let output = Command::new("netsh")
                .args(&[
                    "interface", "ip", "set", "address",
                    &self.name, "static", 
                    &self.ip_address.to_string(),
                    &self.netmask.to_string()
                ])
                .output()?;

            if !output.status.success() {
                println!("⚠️ Warning: Failed to configure Windows interface: {}", 
                    String::from_utf8_lossy(&output.stderr));
            }
        }

        Ok(())
    }

    fn get_prefix_length(&self) -> u8 {
        // Convert netmask to prefix length (CIDR notation)
        match self.netmask {
            IpAddr::V4(mask) => {
                let mask_bits = u32::from(mask);
                mask_bits.count_ones() as u8
            }
            IpAddr::V6(_) => 64, // Default for IPv6
        }
    }

    pub async fn read_packet(&self) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        #[cfg(target_os = "linux")]
        {
            if let Some(fd) = self.fd {
                let mut buffer = vec![0u8; 1500];
                
                // Read from TUN device (this is a simplified example)
                // In reality, you'd use async I/O
                use std::io::Read;
                use std::fs::File;
                use std::os::unix::io::FromRawFd;
                
                unsafe {
                    let mut file = File::from_raw_fd(fd);
                    let n = file.read(&mut buffer)?;
                    buffer.truncate(n);
                    std::mem::forget(file); // Don't close the fd
                }
                
                Ok(buffer)
            } else {
                Err("TUN interface not initialized".into())
            }
        }

        #[cfg(target_os = "windows")]
        {
            // Windows TUN packet reading would require WinTun library or similar
            // For now, return empty packet
            Ok(vec![])
        }
    }

    pub async fn write_packet(&self, packet: &[u8]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        #[cfg(target_os = "linux")]
        {
            if let Some(fd) = self.fd {
                use std::io::Write;
                use std::fs::File;
                use std::os::unix::io::FromRawFd;
                
                unsafe {
                    let mut file = File::from_raw_fd(fd);
                    file.write_all(packet)?;
                    std::mem::forget(file); // Don't close the fd
                }
                
                Ok(())
            } else {
                Err("TUN interface not initialized".into())
            }
        }

        #[cfg(target_os = "windows")]
        {
            // Windows packet writing implementation
            println!("📤 Would write {} bytes to Windows TUN interface", packet.len());
            Ok(())
        }
    }

    pub async fn destroy(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("🗑️ Destroying TUN interface: {}", self.name);

        #[cfg(target_os = "linux")]
        {
            // Bring interface down
            let _ = Command::new("ip")
                .args(&["link", "set", "dev", &self.name, "down"])
                .output();

            // Remove interface
            let _ = Command::new("ip")
                .args(&["link", "delete", &self.name])
                .output();
        }

        #[cfg(target_os = "windows")]
        {
            // Clean up Windows interface
            let _ = Command::new("netsh")
                .args(&["interface", "ip", "set", "interface", &self.name, "admin=disabled"])
                .output();
        }

        self.is_up = false;
        println!("✅ TUN interface destroyed");
        Ok(())
    }
}

// Route manager for directing traffic through VPN
#[derive(Debug)]
pub struct RouteManager {
    pub original_routes: Vec<Route>,
    pub vpn_routes: Vec<Route>,
}

unsafe impl Send for RouteManager {}
unsafe impl Sync for RouteManager {}

#[derive(Debug, Clone)]
pub struct Route {
    pub destination: String,
    pub gateway: String,
    pub interface: String,
    pub metric: u32,
}

impl RouteManager {
    pub fn new() -> Self {
        Self {
            original_routes: Vec::new(),
            vpn_routes: Vec::new(),
        }
    }

    pub async fn backup_routes(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("💾 Backing up current routing table");

        #[cfg(target_os = "linux")]
        {
            let output = Command::new("ip")
                .args(&["route", "show"])
                .output()?;

            let routes_text = String::from_utf8_lossy(&output.stdout);
            for line in routes_text.lines() {
                if let Some(route) = self.parse_linux_route(line) {
                    self.original_routes.push(route);
                }
            }
        }

        #[cfg(target_os = "windows")]
        {
            let output = Command::new("route")
                .args(&["print", "-4"])
                .output()?;

            let routes_text = String::from_utf8_lossy(&output.stdout);
            for line in routes_text.lines() {
                if let Some(route) = self.parse_windows_route(line) {
                    self.original_routes.push(route);
                }
            }
        }

        println!("✅ Backed up {} routes", self.original_routes.len());
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn parse_linux_route(&self, line: &str) -> Option<Route> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 5 && parts[1] == "via" {
            Some(Route {
                destination: parts[0].to_string(),
                gateway: parts[2].to_string(),
                interface: parts[4].to_string(),
                metric: 100,
            })
        } else {
            None
        }
    }

    #[cfg(target_os = "windows")]
    fn parse_windows_route(&self, line: &str) -> Option<Route> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 4 {
            Some(Route {
                destination: parts[0].to_string(),
                gateway: parts[2].to_string(),
                interface: parts[3].to_string(),
                metric: 1,
            })
        } else {
            None
        }
    }

    pub async fn setup_vpn_routes(&mut self, tun_interface: &str, vpn_gateway: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("🛣️ Setting up VPN routes");

        #[cfg(target_os = "linux")]
        {
            // Add route for all traffic through VPN
            let output = Command::new("ip")
                .args(&[
                    "route", "add", "0.0.0.0/1", 
                    "dev", tun_interface,
                    "metric", "1"
                ])
                .output()?;

            if output.status.success() {
                self.vpn_routes.push(Route {
                    destination: "0.0.0.0/1".to_string(),
                    gateway: vpn_gateway.to_string(),
                    interface: tun_interface.to_string(),
                    metric: 1,
                });
            }

            // Add route for second half of internet
            let output = Command::new("ip")
                .args(&[
                    "route", "add", "128.0.0.0/1", 
                    "dev", tun_interface,
                    "metric", "1"
                ])
                .output()?;

            if output.status.success() {
                self.vpn_routes.push(Route {
                    destination: "128.0.0.0/1".to_string(),
                    gateway: vpn_gateway.to_string(),
                    interface: tun_interface.to_string(),
                    metric: 1,
                });
            }
        }

        #[cfg(target_os = "windows")]
        {
            // Add Windows VPN routes
            let output = Command::new("route")
                .args(&[
                    "add", "0.0.0.0", "mask", "128.0.0.0", 
                    vpn_gateway, "metric", "1"
                ])
                .output()?;

            if output.status.success() {
                self.vpn_routes.push(Route {
                    destination: "0.0.0.0".to_string(),
                    gateway: vpn_gateway.to_string(),
                    interface: tun_interface.to_string(),
                    metric: 1,
                });
            }

            let output = Command::new("route")
                .args(&[
                    "add", "128.0.0.0", "mask", "128.0.0.0", 
                    vpn_gateway, "metric", "1"
                ])
                .output()?;

            if output.status.success() {
                self.vpn_routes.push(Route {
                    destination: "128.0.0.0".to_string(),
                    gateway: vpn_gateway.to_string(),
                    interface: tun_interface.to_string(),
                    metric: 1,
                });
            }
        }

        println!("✅ Set up {} VPN routes", self.vpn_routes.len());
        Ok(())
    }

    pub async fn restore_routes(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("🔄 Restoring original routes");

        // Remove VPN routes
        for route in &self.vpn_routes {
            #[cfg(target_os = "linux")]
            {
                let _ = Command::new("ip")
                    .args(&["route", "del", &route.destination])
                    .output();
            }

            #[cfg(target_os = "windows")]
            {
                let _ = Command::new("route")
                    .args(&["delete", &route.destination])
                    .output();
            }
        }

        self.vpn_routes.clear();
        println!("✅ Routes restored");
        Ok(())
    }
}

// Main TUN manager that coordinates everything
#[derive(Debug)]
pub struct TunManager {
    pub interface: Option<TunInterface>,
    pub route_manager: RouteManager,
    pub is_active: bool,
}

unsafe impl Send for TunManager {}
unsafe impl Sync for TunManager {}

impl TunManager {
    pub fn new() -> Self {
        Self {
            interface: None,
            route_manager: RouteManager::new(),
            is_active: false,
        }
    }

    pub async fn create_tunnel(&mut self, client_id: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("🚇 Creating VPN tunnel for client: {}", client_id);

        // Create TUN interface
        let interface_name = format!("iron{}", &client_id[client_id.len()-4..]);
        let vpn_ip = IpAddr::V4(Ipv4Addr::new(10, 8, 0, 2));
        let netmask = IpAddr::V4(Ipv4Addr::new(255, 255, 255, 0));

        let mut tun = TunInterface::new(interface_name.clone(), vpn_ip, netmask);
        tun.create().await?;

        // Backup and modify routes
        self.route_manager.backup_routes().await?;
        self.route_manager.setup_vpn_routes(&interface_name, "10.8.0.1").await?;

        self.interface = Some(tun);
        self.is_active = true;

        println!("✅ VPN tunnel created successfully");
        Ok(())
    }

    pub async fn destroy_tunnel(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("🗑️ Destroying VPN tunnel");

        // Restore routes
        self.route_manager.restore_routes().await?;

        // Destroy interface
        if let Some(mut interface) = self.interface.take() {
            interface.destroy().await?;
        }

        self.is_active = false;
        println!("✅ VPN tunnel destroyed");
        Ok(())
    }

    pub async fn process_packet(&self, packet: Vec<u8>) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        // Process and potentially modify packets
        // This is where you'd implement encryption, compression, etc.
        
        println!("📦 Processing packet: {} bytes", packet.len());
        Ok(packet)
    }
}