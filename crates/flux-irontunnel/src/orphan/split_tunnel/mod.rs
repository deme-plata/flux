pub mod rules;
pub mod manager;
pub mod policy;

use crate::{Result, IronTunnelError};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::collections::{HashMap, HashSet};
use std::process::Pid;
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};
use ipnet::{Ipv4Net, Ipv6Net};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SplitTunnelConfig {
    pub enabled: bool,
    pub default_action: SplitAction,
    pub app_rules: Vec<AppRule>,
    pub domain_rules: Vec<DomainRule>,
    pub ip_rules: Vec<IpRule>,
    pub allow_lan_access: bool,
    pub bypass_china: bool,
    pub bypass_russia: bool,
    pub custom_bypass_countries: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SplitAction {
    VpnBypass,    // Traffic goes direct (bypasses VPN)
    VpnOnly,      // Traffic goes through VPN
    Block,        // Traffic is blocked
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppRule {
    pub name: String,
    pub executable_path: String,
    pub action: SplitAction,
    pub enabled: bool,
    pub priority: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainRule {
    pub domain: String,
    pub action: SplitAction,
    pub include_subdomains: bool,
    pub enabled: bool,
    pub priority: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpRule {
    pub ip_range: String,  // CIDR notation
    pub action: SplitAction,
    pub enabled: bool,
    pub priority: u8,
    pub description: String,
}

impl Default for SplitTunnelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            default_action: SplitAction::VpnOnly,
            app_rules: vec![
                AppRule {
                    name: "Torrent Client".to_string(),
                    executable_path: "qbittorrent".to_string(),
                    action: SplitAction::VpnOnly,
                    enabled: true,
                    priority: 10,
                },
                AppRule {
                    name: "Gaming Platform".to_string(),
                    executable_path: "steam".to_string(),
                    action: SplitAction::VpnBypass,
                    enabled: false,
                    priority: 5,
                },
            ],
            domain_rules: vec![
                DomainRule {
                    domain: "netflix.com".to_string(),
                    action: SplitAction::VpnBypass,
                    include_subdomains: true,
                    enabled: false,
                    priority: 8,
                },
                DomainRule {
                    domain: "banking.example.com".to_string(),
                    action: SplitAction::VpnBypass,
                    include_subdomains: true,
                    enabled: false,
                    priority: 9,
                },
            ],
            ip_rules: vec![
                IpRule {
                    ip_range: "192.168.0.0/16".to_string(),
                    action: SplitAction::VpnBypass,
                    enabled: true,
                    priority: 10,
                    description: "Local Network".to_string(),
                },
                IpRule {
                    ip_range: "10.0.0.0/8".to_string(),
                    action: SplitAction::VpnBypass,
                    enabled: true,
                    priority: 10,
                    description: "Private Network".to_string(),
                },
            ],
            allow_lan_access: true,
            bypass_china: false,
            bypass_russia: false,
            custom_bypass_countries: vec![],
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub executable_path: String,
    pub parent_pid: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct ConnectionInfo {
    pub local_addr: IpAddr,
    pub local_port: u16,
    pub remote_addr: IpAddr,
    pub remote_port: u16,
    pub protocol: String,
    pub process: Option<ProcessInfo>,
    pub domain: Option<String>,
}

pub struct SplitTunnelManager {
    config: SplitTunnelConfig,
    active_processes: Arc<RwLock<HashMap<u32, ProcessInfo>>>,
    domain_cache: Arc<RwLock<HashMap<IpAddr, String>>>,
    routing_table: Arc<RwLock<HashMap<String, SplitAction>>>,
    country_ip_ranges: Arc<RwLock<HashMap<String, Vec<String>>>>,
}

impl SplitTunnelManager {
    pub fn new(config: SplitTunnelConfig) -> Self {
        Self {
            config,
            active_processes: Arc::new(RwLock::new(HashMap::new())),
            domain_cache: Arc::new(RwLock::new(HashMap::new())),
            routing_table: Arc::new(RwLock::new(HashMap::new())),
            country_ip_ranges: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    
    pub async fn start(&mut self) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }
        
        self.load_country_ip_ranges().await?;
        self.setup_routing_rules().await?;
        self.start_process_monitoring().await?;
        self.setup_packet_interception().await?;
        
        tracing::info!("Split tunneling started");
        Ok(())
    }
    
    pub async fn stop(&mut self) -> Result<()> {
        self.cleanup_routing_rules().await?;
        tracing::info!("Split tunneling stopped");
        Ok(())
    }
    
    pub async fn should_bypass_vpn(&self, connection: &ConnectionInfo) -> Result<SplitAction> {
        if !self.config.enabled {
            return Ok(SplitAction::VpnOnly);
        }
        
        // Check app rules first (highest priority)
        if let Some(process) = &connection.process {
            if let Some(action) = self.check_app_rules(process).await {
                return Ok(action);
            }
        }
        
        // Check domain rules
        if let Some(domain) = &connection.domain {
            if let Some(action) = self.check_domain_rules(domain).await {
                return Ok(action);
            }
        }
        
        // Check IP rules
        if let Some(action) = self.check_ip_rules(connection.remote_addr).await {
            return Ok(action);
        }
        
        // Check country bypass rules
        if let Some(action) = self.check_country_rules(connection.remote_addr).await {
            return Ok(action);
        }
        
        // Default action
        Ok(self.config.default_action.clone())
    }
    
    pub async fn add_app_rule(&mut self, rule: AppRule) -> Result<()> {
        self.config.app_rules.push(rule);
        self.update_routing_rules().await?;
        Ok(())
    }
    
    pub async fn add_domain_rule(&mut self, rule: DomainRule) -> Result<()> {
        self.config.domain_rules.push(rule);
        self.update_routing_rules().await?;
        Ok(())
    }
    
    pub async fn add_ip_rule(&mut self, rule: IpRule) -> Result<()> {
        self.config.ip_rules.push(rule);
        self.update_routing_rules().await?;
        Ok(())
    }
    
    pub async fn remove_app_rule(&mut self, executable_path: &str) -> Result<()> {
        self.config.app_rules.retain(|rule| rule.executable_path != executable_path);
        self.update_routing_rules().await?;
        Ok(())
    }
    
    pub async fn get_bypass_stats(&self) -> Result<SplitTunnelStats> {
        let routing_table = self.routing_table.read().await;
        
        let total_rules = routing_table.len();
        let vpn_bypass_count = routing_table.values()
            .filter(|action| matches!(action, SplitAction::VpnBypass))
            .count();
        let vpn_only_count = routing_table.values()
            .filter(|action| matches!(action, SplitAction::VpnOnly))
            .count();
        let blocked_count = routing_table.values()
            .filter(|action| matches!(action, SplitAction::Block))
            .count();
        
        Ok(SplitTunnelStats {
            total_rules,
            vpn_bypass_count,
            vpn_only_count,
            blocked_count,
            active_processes: self.active_processes.read().await.len(),
        })
    }
    
    async fn check_app_rules(&self, process: &ProcessInfo) -> Option<SplitAction> {
        let mut matching_rules: Vec<_> = self.config.app_rules.iter()
            .filter(|rule| rule.enabled)
            .filter(|rule| {
                process.executable_path.contains(&rule.executable_path) ||
                process.name.contains(&rule.name)
            })
            .collect();
        
        // Sort by priority (higher priority first)
        matching_rules.sort_by(|a, b| b.priority.cmp(&a.priority));
        
        matching_rules.first().map(|rule| rule.action.clone())
    }
    
    async fn check_domain_rules(&self, domain: &str) -> Option<SplitAction> {
        let domain_lower = domain.to_lowercase();
        
        let mut matching_rules: Vec<_> = self.config.domain_rules.iter()
            .filter(|rule| rule.enabled)
            .filter(|rule| {
                if rule.include_subdomains {
                    domain_lower.ends_with(&rule.domain.to_lowercase()) ||
                    domain_lower == rule.domain.to_lowercase()
                } else {
                    domain_lower == rule.domain.to_lowercase()
                }
            })
            .collect();
        
        matching_rules.sort_by(|a, b| b.priority.cmp(&a.priority));
        matching_rules.first().map(|rule| rule.action.clone())
    }
    
    async fn check_ip_rules(&self, ip: IpAddr) -> Option<SplitAction> {
        let mut matching_rules: Vec<_> = self.config.ip_rules.iter()
            .filter(|rule| rule.enabled)
            .filter(|rule| self.ip_in_range(ip, &rule.ip_range))
            .collect();
        
        matching_rules.sort_by(|a, b| b.priority.cmp(&a.priority));
        matching_rules.first().map(|rule| rule.action.clone())
    }
    
    async fn check_country_rules(&self, ip: IpAddr) -> Option<SplitAction> {
        let country_ranges = self.country_ip_ranges.read().await;
        
        // Check bypass_china
        if self.config.bypass_china {
            if let Some(china_ranges) = country_ranges.get("CN") {
                for range in china_ranges {
                    if self.ip_in_range(ip, range) {
                        return Some(SplitAction::VpnBypass);
                    }
                }
            }
        }
        
        // Check bypass_russia
        if self.config.bypass_russia {
            if let Some(russia_ranges) = country_ranges.get("RU") {
                for range in russia_ranges {
                    if self.ip_in_range(ip, range) {
                        return Some(SplitAction::VpnBypass);
                    }
                }
            }
        }
        
        // Check custom bypass countries
        for country_code in &self.config.custom_bypass_countries {
            if let Some(ranges) = country_ranges.get(country_code) {
                for range in ranges {
                    if self.ip_in_range(ip, range) {
                        return Some(SplitAction::VpnBypass);
                    }
                }
            }
        }
        
        None
    }
    
    fn ip_in_range(&self, ip: IpAddr, range_str: &str) -> bool {
        match ip {
            IpAddr::V4(ipv4) => {
                if let Ok(network) = range_str.parse::<Ipv4Net>() {
                    network.contains(&ipv4)
                } else {
                    false
                }
            }
            IpAddr::V6(ipv6) => {
                if let Ok(network) = range_str.parse::<Ipv6Net>() {
                    network.contains(&ipv6)
                } else {
                    false
                }
            }
        }
    }
    
    async fn load_country_ip_ranges(&self) -> Result<()> {
        // Mock implementation - in production would load from GeoIP database
        let mut ranges = HashMap::new();
        
        ranges.insert("CN".to_string(), vec![
            "1.0.1.0/24".to_string(),
            "1.0.2.0/23".to_string(),
        ]);
        
        ranges.insert("RU".to_string(), vec![
            "5.8.0.0/19".to_string(),
            "5.34.0.0/16".to_string(),
        ]);
        
        ranges.insert("US".to_string(), vec![
            "3.0.0.0/8".to_string(),
            "4.0.0.0/8".to_string(),
        ]);
        
        let mut country_ranges = self.country_ip_ranges.write().await;
        *country_ranges = ranges;
        
        Ok(())
    }
    
    async fn setup_routing_rules(&self) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            self.setup_linux_routing().await?;
        }
        
        #[cfg(target_os = "windows")]
        {
            self.setup_windows_routing().await?;
        }
        
        Ok(())
    }
    
    #[cfg(target_os = "linux")]
    async fn setup_linux_routing(&self) -> Result<()> {
        use tokio::process::Command;
        
        // Create custom routing tables
        let output = Command::new("ip")
            .args(&["rule", "add", "table", "vpn_bypass", "priority", "100"])
            .output()
            .await?;
        
        if !output.status.success() {
            tracing::warn!("Failed to create VPN bypass routing table");
        }
        
        // Add routes for LAN access if enabled
        if self.config.allow_lan_access {
            for ip_rule in &self.config.ip_rules {
                if matches!(ip_rule.action, SplitAction::VpnBypass) && ip_rule.enabled {
                    let _ = Command::new("ip")
                        .args(&["route", "add", &ip_rule.ip_range, "table", "vpn_bypass"])
                        .output()
                        .await;
                }
            }
        }
        
        Ok(())
    }
    
    #[cfg(target_os = "windows")]
    async fn setup_windows_routing(&self) -> Result<()> {
        use tokio::process::Command;
        
        // Windows-specific routing rules
        for ip_rule in &self.config.ip_rules {
            if matches!(ip_rule.action, SplitAction::VpnBypass) && ip_rule.enabled {
                let _ = Command::new("route")
                    .args(&["add", &ip_rule.ip_range, "0.0.0.0", "metric", "1"])
                    .output()
                    .await;
            }
        }
        
        Ok(())
    }
    
    async fn cleanup_routing_rules(&self) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            use tokio::process::Command;
            
            let _ = Command::new("ip")
                .args(&["rule", "del", "table", "vpn_bypass"])
                .output()
                .await;
        }
        
        Ok(())
    }
    
    async fn update_routing_rules(&self) -> Result<()> {
        self.cleanup_routing_rules().await?;
        self.setup_routing_rules().await?;
        Ok(())
    }
    
    async fn start_process_monitoring(&self) -> Result<()> {
        let processes = self.active_processes.clone();
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));
            
            loop {
                interval.tick().await;
                if let Err(e) = Self::update_process_list(processes.clone()).await {
                    tracing::warn!("Failed to update process list: {}", e);
                }
            }
        });
        
        Ok(())
    }
    
    async fn update_process_list(processes: Arc<RwLock<HashMap<u32, ProcessInfo>>>) -> Result<()> {
        // Mock implementation - in production would use sysinfo or similar
        let mut process_map = processes.write().await;
        process_map.clear();
        
        // Add some mock processes
        process_map.insert(1234, ProcessInfo {
            pid: 1234,
            name: "firefox".to_string(),
            executable_path: "/usr/bin/firefox".to_string(),
            parent_pid: Some(1),
        });
        
        Ok(())
    }
    
    async fn setup_packet_interception(&self) -> Result<()> {
        // TODO: Implement packet interception for split tunneling decisions
        // This would involve netfilter on Linux, WinDivert on Windows, etc.
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SplitTunnelStats {
    pub total_rules: usize,
    pub vpn_bypass_count: usize,
    pub vpn_only_count: usize,
    pub blocked_count: usize,
    pub active_processes: usize,
}