pub mod protection;
pub mod resolver;
pub mod blocklist;

use crate::{Result, IronTunnelError};
use hickory_resolver::{TokioAsyncResolver, config::{ResolverConfig, ResolverOpts}};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};
use trust_dns_resolver::proto::rr::RecordType;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsConfig {
    pub servers: Vec<DnsServer>,
    pub leak_protection: bool,
    pub block_ads: bool,
    pub block_malware: bool,
    pub block_trackers: bool,
    pub custom_blocklist_urls: Vec<String>,
    pub fallback_servers: Vec<DnsServer>,
    pub cache_size: usize,
    pub cache_ttl: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsServer {
    pub name: String,
    pub address: IpAddr,
    pub port: u16,
    pub supports_doh: bool,
    pub supports_dot: bool,
    pub provider: DnsProvider,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DnsProvider {
    Cloudflare,
    Quad9,
    OpenDNS,
    Custom,
    CleanBrowsing,
    AdGuard,
}

impl Default for DnsConfig {
    fn default() -> Self {
        Self {
            servers: vec![
                DnsServer {
                    name: "Cloudflare Primary".to_string(),
                    address: IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)),
                    port: 53,
                    supports_doh: true,
                    supports_dot: true,
                    provider: DnsProvider::Cloudflare,
                },
                DnsServer {
                    name: "Cloudflare Secondary".to_string(),
                    address: IpAddr::V4(Ipv4Addr::new(1, 0, 0, 1)),
                    port: 53,
                    supports_doh: true,
                    supports_dot: true,
                    provider: DnsProvider::Cloudflare,
                },
            ],
            leak_protection: true,
            block_ads: true,
            block_malware: true,
            block_trackers: true,
            custom_blocklist_urls: vec![
                "https://raw.githubusercontent.com/StevenBlack/hosts/master/hosts".to_string(),
                "https://someonewhocares.org/hosts/zero/hosts".to_string(),
            ],
            fallback_servers: vec![
                DnsServer {
                    name: "Quad9".to_string(),
                    address: IpAddr::V4(Ipv4Addr::new(9, 9, 9, 9)),
                    port: 53,
                    supports_doh: true,
                    supports_dot: true,
                    provider: DnsProvider::Quad9,
                },
            ],
            cache_size: 10000,
            cache_ttl: 300,
        }
    }
}

#[derive(Debug, Clone)]
struct DnsEntry {
    ip: IpAddr,
    ttl: u64,
    timestamp: std::time::Instant,
}

#[derive(Debug, Clone)]
struct DnsQuery {
    domain: String,
    record_type: RecordType,
    client_ip: IpAddr,
    timestamp: std::time::Instant,
}

pub struct DnsManager {
    config: DnsConfig,
    resolver: TokioAsyncResolver,
    cache: Arc<RwLock<HashMap<String, DnsEntry>>>,
    blocklist: Arc<RwLock<HashSet<String>>>,
    query_log: Arc<RwLock<Vec<DnsQuery>>>,
    leak_detector: Arc<LeakDetector>,
    original_dns: Option<Vec<IpAddr>>,
}

impl DnsManager {
    pub async fn new(config: DnsConfig) -> Result<Self> {
        let resolver = Self::create_resolver(&config).await?;
        let cache = Arc::new(RwLock::new(HashMap::with_capacity(config.cache_size)));
        let blocklist = Arc::new(RwLock::new(HashSet::new()));
        let query_log = Arc::new(RwLock::new(Vec::new()));
        let leak_detector = Arc::new(LeakDetector::new());
        
        let mut manager = Self {
            config,
            resolver,
            cache,
            blocklist,
            query_log,
            leak_detector,
            original_dns: None,
        };
        
        manager.load_blocklists().await?;
        Ok(manager)
    }
    
    pub async fn start_dns_protection(&mut self) -> Result<()> {
        if self.config.leak_protection {
            self.backup_system_dns().await?;
            self.set_system_dns().await?;
            self.start_leak_detection().await?;
        }
        
        self.start_dns_server().await?;
        tracing::info!("DNS protection started");
        Ok(())
    }
    
    pub async fn stop_dns_protection(&mut self) -> Result<()> {
        if let Some(ref original_dns) = self.original_dns.take() {
            self.restore_system_dns(original_dns).await?;
        }
        
        tracing::info!("DNS protection stopped");
        Ok(())
    }
    
    pub async fn resolve_domain(&self, domain: &str, record_type: RecordType, client_ip: IpAddr) -> Result<Vec<IpAddr>> {
        // Log the query
        self.log_query(domain, record_type, client_ip).await;
        
        // Check blocklist
        if self.is_blocked(domain).await {
            tracing::debug!("Blocked domain: {}", domain);
            return Ok(vec![IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0))]);
        }
        
        // Check cache
        if let Some(cached) = self.get_cached_result(domain).await {
            return Ok(vec![cached]);
        }
        
        // Perform actual resolution
        let result = self.perform_resolution(domain, record_type).await?;
        
        // Cache the result
        if !result.is_empty() {
            self.cache_result(domain, result[0]).await;
        }
        
        Ok(result)
    }
    
    pub async fn add_to_blocklist(&self, domains: Vec<String>) -> Result<()> {
        let mut blocklist = self.blocklist.write().await;
        for domain in domains {
            blocklist.insert(domain.to_lowercase());
        }
        tracing::info!("Added {} domains to blocklist", blocklist.len());
        Ok(())
    }
    
    pub async fn remove_from_blocklist(&self, domains: Vec<String>) -> Result<()> {
        let mut blocklist = self.blocklist.write().await;
        for domain in domains {
            blocklist.remove(&domain.to_lowercase());
        }
        Ok(())
    }
    
    pub async fn get_dns_stats(&self) -> Result<DnsStats> {
        let cache = self.cache.read().await;
        let blocklist = self.blocklist.read().await;
        let query_log = self.query_log.read().await;
        
        let total_queries = query_log.len();
        let blocked_queries = query_log.iter()
            .filter(|q| blocklist.contains(&q.domain.to_lowercase()))
            .count();
        
        Ok(DnsStats {
            total_queries,
            blocked_queries,
            cache_hits: cache.len(),
            cache_size: cache.len(),
            blocklist_size: blocklist.len(),
        })
    }
    
    pub async fn check_dns_leaks(&self) -> Result<Vec<DnsLeak>> {
        self.leak_detector.check_leaks().await
    }
    
    async fn create_resolver(config: &DnsConfig) -> Result<TokioAsyncResolver> {
        let mut resolver_config = ResolverConfig::new();
        
        for server in &config.servers {
            let socket_addr = SocketAddr::new(server.address, server.port);
            resolver_config.add_name_server(socket_addr.into());
        }
        
        let resolver = TokioAsyncResolver::tokio(resolver_config, ResolverOpts::default())
            .map_err(|e| IronTunnelError::Network(format!("Failed to create DNS resolver: {}", e)))?;
        
        Ok(resolver)
    }
    
    async fn load_blocklists(&self) -> Result<()> {
        let mut all_domains = HashSet::new();
        
        // Load built-in blocklists
        if self.config.block_ads {
            all_domains.extend(self.load_ad_blocklist().await?);
        }
        
        if self.config.block_malware {
            all_domains.extend(self.load_malware_blocklist().await?);
        }
        
        if self.config.block_trackers {
            all_domains.extend(self.load_tracker_blocklist().await?);
        }
        
        // Load custom blocklists
        for url in &self.config.custom_blocklist_urls {
            if let Ok(domains) = self.load_remote_blocklist(url).await {
                all_domains.extend(domains);
            }
        }
        
        let mut blocklist = self.blocklist.write().await;
        *blocklist = all_domains;
        
        tracing::info!("Loaded {} domains into blocklist", blocklist.len());
        Ok(())
    }
    
    async fn load_ad_blocklist(&self) -> Result<HashSet<String>> {
        // Built-in ad domains
        Ok([
            "doubleclick.net",
            "googleadservices.com",
            "googlesyndication.com",
            "facebook.com/tr",
            "amazon-adsystem.com",
            "adsystem.amazon.com",
        ].iter().map(|s| s.to_string()).collect())
    }
    
    async fn load_malware_blocklist(&self) -> Result<HashSet<String>> {
        // Built-in malware domains
        Ok([
            "malware.example.com",
            "phishing.example.com",
            "badsite.example.com",
        ].iter().map(|s| s.to_string()).collect())
    }
    
    async fn load_tracker_blocklist(&self) -> Result<HashSet<String>> {
        // Built-in tracker domains
        Ok([
            "google-analytics.com",
            "googletagmanager.com",
            "hotjar.com",
            "mixpanel.com",
            "segment.com",
        ].iter().map(|s| s.to_string()).collect())
    }
    
    async fn load_remote_blocklist(&self, url: &str) -> Result<HashSet<String>> {
        // Mock implementation - in production would fetch from URL
        tracing::debug!("Loading blocklist from: {}", url);
        
        // Return mock data for now
        Ok([
            "ads.example.com",
            "tracker.example.com",
            "malware.example.com",
        ].iter().map(|s| s.to_string()).collect())
    }
    
    async fn backup_system_dns(&mut self) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            let resolv_conf = tokio::fs::read_to_string("/etc/resolv.conf").await?;
            let nameservers: Vec<IpAddr> = resolv_conf
                .lines()
                .filter(|line| line.starts_with("nameserver"))
                .filter_map(|line| {
                    line.split_whitespace()
                        .nth(1)
                        .and_then(|ip| ip.parse().ok())
                })
                .collect();
            
            self.original_dns = Some(nameservers);
        }
        
        Ok(())
    }
    
    async fn set_system_dns(&self) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            let mut resolv_conf = String::new();
            for server in &self.config.servers {
                resolv_conf.push_str(&format!("nameserver {}\n", server.address));
            }
            
            tokio::fs::write("/etc/resolv.conf", resolv_conf).await?;
        }
        
        Ok(())
    }
    
    async fn restore_system_dns(&self, original_dns: &[IpAddr]) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            let mut resolv_conf = String::new();
            for ip in original_dns {
                resolv_conf.push_str(&format!("nameserver {}\n", ip));
            }
            
            tokio::fs::write("/etc/resolv.conf", resolv_conf).await?;
        }
        
        Ok(())
    }
    
    async fn start_leak_detection(&self) -> Result<()> {
        let detector = self.leak_detector.clone();
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));
            
            loop {
                interval.tick().await;
                if let Err(e) = detector.perform_leak_test().await {
                    tracing::warn!("DNS leak detection failed: {}", e);
                }
            }
        });
        
        Ok(())
    }
    
    async fn start_dns_server(&self) -> Result<()> {
        // TODO: Implement local DNS server for intercepting queries
        Ok(())
    }
    
    async fn log_query(&self, domain: &str, record_type: RecordType, client_ip: IpAddr) {
        let mut query_log = self.query_log.write().await;
        query_log.push(DnsQuery {
            domain: domain.to_string(),
            record_type,
            client_ip,
            timestamp: std::time::Instant::now(),
        });
        
        // Keep only recent queries
        if query_log.len() > 10000 {
            query_log.drain(..1000);
        }
    }
    
    async fn is_blocked(&self, domain: &str) -> bool {
        let blocklist = self.blocklist.read().await;
        let domain_lower = domain.to_lowercase();
        
        // Check exact match
        if blocklist.contains(&domain_lower) {
            return true;
        }
        
        // Check subdomains
        let parts: Vec<&str> = domain_lower.split('.').collect();
        for i in 1..parts.len() {
            let parent_domain = parts[i..].join(".");
            if blocklist.contains(&parent_domain) {
                return true;
            }
        }
        
        false
    }
    
    async fn get_cached_result(&self, domain: &str) -> Option<IpAddr> {
        let cache = self.cache.read().await;
        if let Some(entry) = cache.get(domain) {
            let age = entry.timestamp.elapsed().as_secs();
            if age < entry.ttl {
                return Some(entry.ip);
            }
        }
        None
    }
    
    async fn cache_result(&self, domain: &str, ip: IpAddr) {
        let mut cache = self.cache.write().await;
        cache.insert(domain.to_string(), DnsEntry {
            ip,
            ttl: self.config.cache_ttl,
            timestamp: std::time::Instant::now(),
        });
        
        // Cleanup old entries if cache is full
        if cache.len() > self.config.cache_size {
            let mut entries: Vec<_> = cache.iter().collect();
            entries.sort_by_key(|(_, entry)| entry.timestamp);
            
            let to_remove = entries.len() - self.config.cache_size + 1000;
            for (domain, _) in entries.iter().take(to_remove) {
                cache.remove(*domain);
            }
        }
    }
    
    async fn perform_resolution(&self, domain: &str, _record_type: RecordType) -> Result<Vec<IpAddr>> {
        let response = self.resolver.lookup_ip(domain).await
            .map_err(|e| IronTunnelError::Network(format!("DNS resolution failed: {}", e)))?;
        
        Ok(response.iter().collect())
    }
}

pub struct LeakDetector {
    test_domains: Vec<String>,
}

impl LeakDetector {
    pub fn new() -> Self {
        Self {
            test_domains: vec![
                "whoami.akamai.net".to_string(),
                "resolver.dnscrypt.info".to_string(),
                "test.nextdns.io".to_string(),
            ],
        }
    }
    
    pub async fn check_leaks(&self) -> Result<Vec<DnsLeak>> {
        let mut leaks = Vec::new();
        
        for domain in &self.test_domains {
            if let Ok(leak) = self.test_domain_leak(domain).await {
                if leak.is_leak {
                    leaks.push(leak);
                }
            }
        }
        
        Ok(leaks)
    }
    
    async fn perform_leak_test(&self) -> Result<()> {
        let leaks = self.check_leaks().await?;
        
        if !leaks.is_empty() {
            tracing::warn!("DNS leaks detected: {} leaks", leaks.len());
            for leak in leaks {
                tracing::warn!("Leak: {} -> {}", leak.domain, leak.leaked_server);
            }
        }
        
        Ok(())
    }
    
    async fn test_domain_leak(&self, domain: &str) -> Result<DnsLeak> {
        // Mock leak detection - in production would perform actual DNS tests
        Ok(DnsLeak {
            domain: domain.to_string(),
            expected_server: "1.1.1.1".to_string(),
            leaked_server: "8.8.8.8".to_string(),
            is_leak: false, // Mock: no leak detected
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsStats {
    pub total_queries: usize,
    pub blocked_queries: usize,
    pub cache_hits: usize,
    pub cache_size: usize,
    pub blocklist_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsLeak {
    pub domain: String,
    pub expected_server: String,
    pub leaked_server: String,
    pub is_leak: bool,
}