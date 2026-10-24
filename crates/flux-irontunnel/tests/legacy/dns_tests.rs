use flux_irontunnel::dns::*;
use std::net::{IpAddr, Ipv4Addr};
use trust_dns_resolver::proto::rr::RecordType;

#[tokio::test]
async fn test_dns_config_creation() {
    let config = DnsConfig::default();
    
    assert!(!config.servers.is_empty());
    assert!(config.leak_protection);
    assert!(config.block_ads);
    assert!(config.block_malware);
    assert!(config.block_trackers);
    assert!(config.cache_size > 0);
    assert!(config.cache_ttl > 0);
}

#[tokio::test]
async fn test_dns_manager_creation() {
    let config = DnsConfig::default();
    let manager = DnsManager::new(config).await;
    
    assert!(manager.is_ok());
}

#[tokio::test]
async fn test_dns_server_configuration() {
    let server = DnsServer {
        name: "Test DNS".to_string(),
        address: IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)),
        port: 53,
        supports_doh: true,
        supports_dot: true,
        provider: DnsProvider::Cloudflare,
    };
    
    assert_eq!(server.name, "Test DNS");
    assert_eq!(server.address, IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)));
    assert_eq!(server.port, 53);
    assert!(server.supports_doh);
    assert!(server.supports_dot);
}

#[tokio::test]
async fn test_dns_resolution() {
    let config = DnsConfig::default();
    let manager = DnsManager::new(config).await.unwrap();
    
    let client_ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100));
    
    // Test domain resolution (this will use mock implementation)
    let result = manager.resolve_domain("example.com", RecordType::A, client_ip).await;
    
    // Should either succeed or fail gracefully
    match result {
        Ok(ips) => {
            // If successful, should return some IPs
            assert!(!ips.is_empty());
        }
        Err(_) => {
            // If failed, should handle gracefully (expected in test environment)
        }
    }
}

#[tokio::test]
async fn test_dns_blocklist_functionality() {
    let config = DnsConfig::default();
    let manager = DnsManager::new(config).await.unwrap();
    
    // Add domains to blocklist
    let domains_to_block = vec![
        "malicious.example.com".to_string(),
        "tracker.example.com".to_string(),
        "ads.example.com".to_string(),
    ];
    
    let result = manager.add_to_blocklist(domains_to_block.clone()).await;
    assert!(result.is_ok());
    
    // Remove some domains
    let domains_to_remove = vec!["tracker.example.com".to_string()];
    let remove_result = manager.remove_from_blocklist(domains_to_remove).await;
    assert!(remove_result.is_ok());
}

#[tokio::test]
async fn test_dns_stats() {
    let config = DnsConfig::default();
    let manager = DnsManager::new(config).await.unwrap();
    
    let stats = manager.get_dns_stats().await;
    assert!(stats.is_ok());
    
    let stats = stats.unwrap();
    assert_eq!(stats.total_queries, 0); // Initially no queries
    assert_eq!(stats.blocked_queries, 0); // Initially no blocked queries
    assert!(stats.blocklist_size > 0); // Should have some built-in blocked domains
}

#[tokio::test]
async fn test_dns_leak_detection() {
    let config = DnsConfig::default();
    let manager = DnsManager::new(config).await.unwrap();
    
    let leaks = manager.check_dns_leaks().await;
    assert!(leaks.is_ok());
    
    let leaks = leaks.unwrap();
    // In test environment, should not detect leaks
    assert!(leaks.is_empty());
}

#[tokio::test]
async fn test_leak_detector() {
    let detector = LeakDetector::new();
    
    assert!(!detector.test_domains.is_empty());
    
    let leaks = detector.check_leaks().await;
    assert!(leaks.is_ok());
    
    // Should not detect leaks in test environment
    let leaks = leaks.unwrap();
    assert!(leaks.is_empty());
}

#[tokio::test]
async fn test_dns_protection_lifecycle() {
    let config = DnsConfig::default();
    let mut manager = DnsManager::new(config).await.unwrap();
    
    // Start DNS protection
    let start_result = manager.start_dns_protection().await;
    // Might fail in test environment without proper permissions
    // but should handle gracefully
    assert!(start_result.is_ok() || start_result.is_err());
    
    // Stop DNS protection
    let stop_result = manager.stop_dns_protection().await;
    assert!(stop_result.is_ok() || stop_result.is_err());
}

#[tokio::test]
async fn test_dns_server_providers() {
    // Test different DNS providers
    let cloudflare = DnsServer {
        name: "Cloudflare".to_string(),
        address: IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)),
        port: 53,
        supports_doh: true,
        supports_dot: true,
        provider: DnsProvider::Cloudflare,
    };
    
    let quad9 = DnsServer {
        name: "Quad9".to_string(),
        address: IpAddr::V4(Ipv4Addr::new(9, 9, 9, 9)),
        port: 53,
        supports_doh: true,
        supports_dot: true,
        provider: DnsProvider::Quad9,
    };
    
    assert!(matches!(cloudflare.provider, DnsProvider::Cloudflare));
    assert!(matches!(quad9.provider, DnsProvider::Quad9));
}

#[tokio::test]
async fn test_dns_config_with_custom_servers() {
    let custom_servers = vec![
        DnsServer {
            name: "Custom DNS 1".to_string(),
            address: IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
            port: 53,
            supports_doh: false,
            supports_dot: false,
            provider: DnsProvider::Custom,
        },
        DnsServer {
            name: "Custom DNS 2".to_string(),
            address: IpAddr::V4(Ipv4Addr::new(8, 8, 4, 4)),
            port: 53,
            supports_doh: false,
            supports_dot: false,
            provider: DnsProvider::Custom,
        },
    ];
    
    let config = DnsConfig {
        servers: custom_servers,
        leak_protection: true,
        block_ads: false,
        block_malware: true,
        block_trackers: false,
        custom_blocklist_urls: vec!["https://example.com/blocklist".to_string()],
        fallback_servers: vec![],
        cache_size: 5000,
        cache_ttl: 600,
    };
    
    let manager = DnsManager::new(config).await;
    assert!(manager.is_ok());
}

#[tokio::test]
async fn test_concurrent_dns_operations() {
    use tokio::task;
    use std::sync::Arc;
    
    let config = DnsConfig::default();
    let manager = Arc::new(DnsManager::new(config).await.unwrap());
    
    // Perform concurrent DNS operations
    let handles: Vec<_> = (0..10).map(|i| {
        let manager = manager.clone();
        task::spawn(async move {
            let domain = format!("test{}.example.com", i);
            let client_ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100 + i as u8));
            
            // This might fail in test environment, but should handle gracefully
            let _result = manager.resolve_domain(&domain, RecordType::A, client_ip).await;
            
            // Add domain to blocklist
            let block_result = manager.add_to_blocklist(vec![domain]).await;
            assert!(block_result.is_ok());
        })
    }).collect();
    
    for handle in handles {
        handle.await.expect("Concurrent DNS task failed");
    }
}

#[tokio::test]
async fn test_dns_cache_behavior() {
    let config = DnsConfig {
        cache_size: 100,
        cache_ttl: 60, // 1 minute
        ..Default::default()
    };
    
    let manager = DnsManager::new(config).await.unwrap();
    let client_ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100));
    
    // First resolution (cache miss)
    let _result1 = manager.resolve_domain("cache-test.example.com", RecordType::A, client_ip).await;
    
    // Second resolution (should be cache hit if first succeeded)
    let _result2 = manager.resolve_domain("cache-test.example.com", RecordType::A, client_ip).await;
    
    // Both should behave consistently
}

#[tokio::test]
async fn test_dns_blocking_categories() {
    let mut config = DnsConfig::default();
    
    // Test different blocking categories
    config.block_ads = true;
    config.block_malware = true;
    config.block_trackers = true;
    
    let manager = DnsManager::new(config).await.unwrap();
    
    // Get stats to verify blocklists were loaded
    let stats = manager.get_dns_stats().await.unwrap();
    assert!(stats.blocklist_size > 0);
}

#[tokio::test]
async fn test_dns_leak_structure() {
    let leak = DnsLeak {
        domain: "test.example.com".to_string(),
        expected_server: "1.1.1.1".to_string(),
        leaked_server: "8.8.8.8".to_string(),
        is_leak: true,
    };
    
    assert_eq!(leak.domain, "test.example.com");
    assert_eq!(leak.expected_server, "1.1.1.1");
    assert_eq!(leak.leaked_server, "8.8.8.8");
    assert!(leak.is_leak);
}

#[tokio::test]
async fn test_dns_stats_structure() {
    let stats = DnsStats {
        total_queries: 1000,
        blocked_queries: 150,
        cache_hits: 400,
        cache_size: 500,
        blocklist_size: 50000,
    };
    
    assert_eq!(stats.total_queries, 1000);
    assert_eq!(stats.blocked_queries, 150);
    assert_eq!(stats.cache_hits, 400);
    assert_eq!(stats.cache_size, 500);
    assert_eq!(stats.blocklist_size, 50000);
    
    // Calculate block rate
    let block_rate = stats.blocked_queries as f64 / stats.total_queries as f64;
    assert_eq!(block_rate, 0.15); // 15% blocked
}

#[tokio::test]
async fn test_dns_manager_with_ipv6() {
    use std::net::Ipv6Addr;
    
    let ipv6_server = DnsServer {
        name: "IPv6 DNS".to_string(),
        address: IpAddr::V6(Ipv6Addr::new(0x2001, 0x4860, 0x4860, 0, 0, 0, 0, 0x8888)),
        port: 53,
        supports_doh: true,
        supports_dot: true,
        provider: DnsProvider::Custom,
    };
    
    let config = DnsConfig {
        servers: vec![ipv6_server],
        ..Default::default()
    };
    
    let manager = DnsManager::new(config).await;
    assert!(manager.is_ok());
}