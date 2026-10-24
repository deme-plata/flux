// IP Pool Manager for IronTunnel
// Handles dynamic IP address assignment for VPN clients

use std::net::Ipv4Addr;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct IpAssignment {
    pub client_id: String,
    pub ip_address: Ipv4Addr,
    pub assigned_at: Instant,
    pub last_activity: Instant,
    pub lease_duration: Duration,
}

pub struct IpPool {
    // Network configuration
    network: Ipv4Addr,
    netmask: Ipv4Addr,
    gateway: Ipv4Addr,
    
    // IP assignments
    assignments: Arc<RwLock<HashMap<String, IpAssignment>>>,
    used_ips: Arc<RwLock<HashSet<Ipv4Addr>>>,
    
    // Pool configuration
    start_ip: u32,
    end_ip: u32,
    next_ip: Arc<RwLock<u32>>,
    
    // DNS servers
    dns_servers: Vec<Ipv4Addr>,
    
    // Lease settings
    default_lease: Duration,
}

impl IpPool {
    pub fn new(network: Ipv4Addr, netmask: Ipv4Addr) -> Self {
        let network_u32 = u32::from(network);
        let netmask_u32 = u32::from(netmask);
        
        // Calculate usable range
        let network_start = network_u32 & netmask_u32;
        let broadcast = network_start | !netmask_u32;
        
        // Gateway is first usable IP
        let gateway = Ipv4Addr::from(network_start + 1);
        
        // Client IPs start at .2 and end at broadcast-1
        let start_ip = network_start + 2;
        let end_ip = broadcast - 1;
        
        Self {
            network,
            netmask,
            gateway,
            assignments: Arc::new(RwLock::new(HashMap::new())),
            used_ips: Arc::new(RwLock::new(HashSet::new())),
            start_ip,
            end_ip,
            next_ip: Arc::new(RwLock::new(start_ip)),
            dns_servers: vec![
                Ipv4Addr::new(1, 1, 1, 1),
                Ipv4Addr::new(8, 8, 8, 8),
            ],
            default_lease: Duration::from_secs(86400), // 24 hours
        }
    }

    // Assign IP to client
    pub async fn assign_ip(&self, client_id: &str) -> Result<IpAssignment, Box<dyn std::error::Error + Send + Sync>> {
        let mut assignments = self.assignments.write().await;
        
        // Check if client already has an assignment
        if let Some(assignment) = assignments.get(client_id) {
            // Renew lease
            let mut renewed = assignment.clone();
            renewed.last_activity = Instant::now();
            assignments.insert(client_id.to_string(), renewed.clone());
            
            println!("♻️ Renewed IP {} for client {}", renewed.ip_address, client_id);
            return Ok(renewed);
        }
        
        // Find available IP
        let ip = self.find_available_ip().await?;
        
        // Create assignment
        let assignment = IpAssignment {
            client_id: client_id.to_string(),
            ip_address: ip,
            assigned_at: Instant::now(),
            last_activity: Instant::now(),
            lease_duration: self.default_lease,
        };
        
        // Store assignment
        assignments.insert(client_id.to_string(), assignment.clone());
        let mut used = self.used_ips.write().await;
        used.insert(ip);
        
        println!("📍 Assigned IP {} to client {}", ip, client_id);
        
        // Start lease expiry check
        self.start_lease_monitor();
        
        Ok(assignment)
    }

    // Find next available IP
    async fn find_available_ip(&self) -> Result<Ipv4Addr, Box<dyn std::error::Error + Send + Sync>> {
        let mut next_ip = self.next_ip.write().await;
        let used = self.used_ips.read().await;
        
        let start = *next_ip;
        let mut current = *next_ip;
        
        loop {
            let ip = Ipv4Addr::from(current);
            
            if !used.contains(&ip) && ip != self.gateway {
                *next_ip = current + 1;
                if *next_ip > self.end_ip {
                    *next_ip = self.start_ip;
                }
                return Ok(ip);
            }
            
            current += 1;
            if current > self.end_ip {
                current = self.start_ip;
            }
            
            // Checked entire range
            if current == start {
                return Err("IP pool exhausted".into());
            }
        }
    }

    // Release IP assignment
    pub async fn release_ip(&self, client_id: &str) -> Option<Ipv4Addr> {
        let mut assignments = self.assignments.write().await;
        
        if let Some(assignment) = assignments.remove(client_id) {
            let mut used = self.used_ips.write().await;
            used.remove(&assignment.ip_address);
            
            println!("🔓 Released IP {} from client {}", assignment.ip_address, client_id);
            Some(assignment.ip_address)
        } else {
            None
        }
    }

    // Get client's assigned IP
    pub async fn get_client_ip(&self, client_id: &str) -> Option<Ipv4Addr> {
        let assignments = self.assignments.read().await;
        assignments.get(client_id).map(|a| a.ip_address)
    }

    // Update client activity
    pub async fn update_activity(&self, client_id: &str) {
        let mut assignments = self.assignments.write().await;
        
        if let Some(assignment) = assignments.get_mut(client_id) {
            assignment.last_activity = Instant::now();
        }
    }

    // Get network configuration for client
    pub fn get_network_config(&self) -> NetworkConfig {
        NetworkConfig {
            ip_address: Ipv4Addr::new(0, 0, 0, 0), // Will be filled per-client
            netmask: self.netmask,
            gateway: self.gateway,
            dns_servers: self.dns_servers.clone(),
            mtu: 1420, // Conservative MTU for VPN
        }
    }

    // Start lease expiry monitor
    fn start_lease_monitor(&self) {
        let assignments = self.assignments.clone();
        let used_ips = self.used_ips.clone();
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(300)); // Check every 5 min
            
            loop {
                interval.tick().await;
                
                let mut assignments_guard = assignments.write().await;
                let mut used_guard = used_ips.write().await;
                let now = Instant::now();
                
                // Find expired leases
                let expired: Vec<String> = assignments_guard
                    .iter()
                    .filter(|(_, assignment)| {
                        now.duration_since(assignment.last_activity) > assignment.lease_duration
                    })
                    .map(|(id, _)| id.clone())
                    .collect();
                
                // Remove expired assignments
                for client_id in expired {
                    if let Some(assignment) = assignments_guard.remove(&client_id) {
                        used_guard.remove(&assignment.ip_address);
                        println!("⏰ Expired IP {} for inactive client {}", 
                            assignment.ip_address, client_id);
                    }
                }
                
                println!("📊 IP pool status: {}/{} assigned", 
                    assignments_guard.len(), 
                    (used_guard.len() as u32).saturating_sub(1) // Exclude gateway
                );
            }
        });
    }

    // Get pool statistics
    pub async fn get_stats(&self) -> PoolStats {
        let assignments = self.assignments.read().await;
        let used = self.used_ips.read().await;
        
        let total_ips = self.end_ip - self.start_ip + 1;
        let assigned = assignments.len() as u32;
        let available = total_ips - assigned;
        
        PoolStats {
            total_ips,
            assigned,
            available,
            network: self.network,
            netmask: self.netmask,
            active_clients: assignments.len(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct NetworkConfig {
    pub ip_address: Ipv4Addr,
    pub netmask: Ipv4Addr,
    pub gateway: Ipv4Addr,
    pub dns_servers: Vec<Ipv4Addr>,
    pub mtu: u16,
}

#[derive(Debug)]
pub struct PoolStats {
    pub total_ips: u32,
    pub assigned: u32,
    pub available: u32,
    pub network: Ipv4Addr,
    pub netmask: Ipv4Addr,
    pub active_clients: usize,
}

// Helper to calculate network size
pub fn calculate_network_size(netmask: Ipv4Addr) -> u32 {
    let mask_bits = u32::from(netmask).count_ones();
    1u32 << (32 - mask_bits)
}