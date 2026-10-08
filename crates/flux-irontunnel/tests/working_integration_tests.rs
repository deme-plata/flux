use flux_irontunnel::*;
use tokio::time::{timeout, Duration};
use std::sync::Arc;
use std::time::Instant;

#[tokio::test]
async fn test_full_client_lifecycle() {
    let config = IronTunnelConfig::default();
    let mut client = IronTunnelClient::new(config).await.unwrap();
    
    // Test connection
    assert!(!client.is_connected());
    client.connect().await.unwrap();
    assert!(client.is_connected());
    
    // Test performance metrics
    let metrics = client.get_performance_metrics().await;
    assert!(metrics.connection_time > 0);
    assert_eq!(metrics.throughput_mbps, 100.0);
    
    // Test kill switch
    client.enable_kill_switch().await.unwrap();
    client.disable_kill_switch().await.unwrap();
    
    // Test disconnection
    client.disconnect().await.unwrap();
    assert!(!client.is_connected());
}

#[tokio::test]
async fn test_crypto_engine_comprehensive() {
    let crypto = crypto::CryptoEngine::new().await.unwrap();
    
    // Test multiple encryption/decryption cycles
    for i in 0..10 {
        let data = format!("Test data {}", i).into_bytes();
        let encrypted = crypto.encrypt_data(&data).await.unwrap();
        let decrypted = crypto.decrypt_data(&encrypted).await.unwrap();
        assert_eq!(data, decrypted);
    }
    
    // Test session key rotation
    crypto.rotate_session_key().await.unwrap();
    crypto.rotate_session_key().await.unwrap();
    crypto.rotate_session_key().await.unwrap();
}

#[tokio::test]
async fn test_zk_engine_comprehensive() {
    let mut engine = zk::ZkEngine::new().unwrap();
    
    // Generate multiple identities
    let mut identities = Vec::new();
    for _ in 0..5 {
        let identity = engine.generate_identity().unwrap();
        identities.push(identity);
    }
    
    assert_eq!(engine.get_identity_count(), 5);
    
    // Test proof creation and verification for each identity
    for identity in &identities {
        let challenge = b"test_challenge";
        let proof = engine.create_proof(identity, challenge).unwrap();
        let verified = engine.verify_proof(&proof, &identity.commitment).unwrap();
        assert!(verified);
    }
}

#[tokio::test]
async fn test_wireguard_comprehensive() {
    let wg = wireguard::WireGuardTunnel::new().await.unwrap();
    
    // Test configuration
    let config = wg.get_config();
    assert_eq!(config.listen_port, 51820);
    assert!(!config.private_key.is_empty());
    assert!(!config.public_key.is_empty());
    
    // Test peer management
    for i in 0..3 {
        let peer = wireguard::WireGuardPeer {
            public_key: format!("peer_{}", i),
            endpoint: None,
            bytes_sent: 0,
            bytes_received: 0,
        };
        wg.add_peer(peer).await.unwrap();
    }
    
    assert_eq!(wg.get_peer_count().await, 3);
}

#[tokio::test]
async fn test_multihop_comprehensive() {
    let multihop = multihop::MultiHopManager::new().await.unwrap();
    
    // Test server availability
    let servers = multihop.get_available_servers();
    assert_eq!(servers.len(), 2);
    assert_eq!(servers[0].country, "United States");
    assert_eq!(servers[1].country, "Germany");
    
    // Test hop chain creation with different hop counts
    for hop_count in 1..=2 {
        let chain_id = multihop.create_hop_chain(hop_count).await.unwrap();
        assert!(!chain_id.is_empty());
    }
    
    assert_eq!(multihop.get_chain_count().await, 2);
}

#[tokio::test]
async fn test_dns_comprehensive() {
    let dns = dns::DnsManager::new().await.unwrap();
    
    // Test blocklist functionality
    assert!(dns.get_blocklist_size().await > 0);
    
    // Test blocked domain resolution
    let result = dns.resolve_domain("ads.example.com").await;
    assert!(result.is_err());
    
    // Test normal domain resolution and caching
    let domains = ["example.com", "test.com", "demo.org"];
    for domain in &domains {
        let ip = dns.resolve_domain(domain).await.unwrap();
        assert_eq!(ip, std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)));
    }
    
    assert_eq!(dns.get_cache_size().await, domains.len());
    
    // Test cache hit (should be fast)
    let start = Instant::now();
    dns.resolve_domain("example.com").await.unwrap();
    let duration = start.elapsed();
    assert!(duration.as_millis() < 10); // Should be very fast from cache
}

#[tokio::test]
async fn test_split_tunnel_comprehensive() {
    let st = split_tunnel::SplitTunnelManager::new().await.unwrap();
    
    // Test rule management
    let rules = vec![
        split_tunnel::SplitTunnelRule {
            id: "app_rule".to_string(),
            rule_type: split_tunnel::SplitTunnelRuleType::Application {
                name: "Firefox".to_string(),
            },
            action: split_tunnel::SplitTunnelAction::BypassVpn,
            enabled: true,
        },
        split_tunnel::SplitTunnelRule {
            id: "domain_rule".to_string(),
            rule_type: split_tunnel::SplitTunnelRuleType::Domain {
                pattern: "*.example.com".to_string(),
            },
            action: split_tunnel::SplitTunnelAction::ForceVpn,
            enabled: true,
        },
        split_tunnel::SplitTunnelRule {
            id: "ip_rule".to_string(),
            rule_type: split_tunnel::SplitTunnelRuleType::IpRange {
                cidr: "192.168.1.0/24".to_string(),
            },
            action: split_tunnel::SplitTunnelAction::BypassVpn,
            enabled: false,
        },
    ];
    
    for rule in rules {
        st.add_rule(rule).await.unwrap();
    }
    
    assert_eq!(st.get_rule_count().await, 3);
    
    // Test rule removal
    st.remove_rule("ip_rule").await.unwrap();
    assert_eq!(st.get_rule_count().await, 2);
    
    // Test bypass decision
    let should_bypass = st.should_bypass_vpn("example.com").await;
    assert!(!should_bypass); // Mock implementation returns false
}

#[tokio::test]
async fn test_concurrent_operations() {
    let config = IronTunnelConfig::default();
    
    // Create multiple clients concurrently
    let mut handles = Vec::new();
    for i in 0..5 {
        let config_clone = config.clone();
        let handle = tokio::spawn(async move {
            let mut client = IronTunnelClient::new(config_clone).await.unwrap();
            client.connect().await.unwrap();
            assert!(client.is_connected());
            
            // Simulate some work
            tokio::time::sleep(Duration::from_millis(100)).await;
            
            client.disconnect().await.unwrap();
            assert!(!client.is_connected());
            
            i
        });
        handles.push(handle);
    }
    
    // Wait for all clients to complete
    for handle in handles {
        let result = handle.await.unwrap();
        assert!(result < 5);
    }
}

#[tokio::test]
async fn test_performance_stress() {
    let crypto = crypto::CryptoEngine::new().await.unwrap();
    
    // Test performance under load
    let start = Instant::now();
    let mut handles = Vec::new();
    
    let crypto_arc = Arc::new(crypto);
    for i in 0..100 {
        let crypto_clone = crypto_arc.clone();
        let handle = tokio::spawn(async move {
            let data = format!("Performance test data {}", i).repeat(100);
            let encrypted = crypto_clone.encrypt_data(data.as_bytes()).await.unwrap();
            let decrypted = crypto_clone.decrypt_data(&encrypted).await.unwrap();
            assert_eq!(data.as_bytes(), decrypted.as_slice());
        });
        handles.push(handle);
    }
    
    // Wait for all operations to complete
    for handle in handles {
        handle.await.unwrap();
    }
    
    let duration = start.elapsed();
    println!("100 concurrent crypto operations completed in {:?}", duration);
    assert!(duration.as_secs() < 5); // Should complete within 5 seconds
}

#[tokio::test]
async fn test_error_handling() {
    // Test DNS error handling
    let dns = dns::DnsManager::new().await.unwrap();
    let result = dns.resolve_domain("malware.example.com").await;
    assert!(result.is_err());
    
    // Test ZK engine error scenarios
    let engine = zk::ZkEngine::new().unwrap();
    let fake_proof = zk::ZkProof {
        data: vec![],
        timestamp: 0,
    };
    let result = engine.verify_proof(&fake_proof, &[]);
    assert!(result.is_ok()); // Mock implementation always returns true
}

#[tokio::test]
async fn test_memory_usage() {
    // flux port: upstream compared SYSTEM-wide used memory (any other process could fail
    // it) against 100_000 while sysinfo reports bytes (so the real bound was 100 KB, not
    // the 100 MB the comment claimed). Measure this process's RSS, in bytes.
    use sysinfo::{get_current_pid, System};

    let pid = get_current_pid().unwrap();
    let mut system = System::new();
    system.refresh_process(pid);
    let initial_memory = system.process(pid).map(|p| p.memory()).unwrap_or(0);

    // Create and test multiple components
    let _crypto = crypto::CryptoEngine::new().await.unwrap();
    let _zk = zk::ZkEngine::new().unwrap();
    let _wg = wireguard::WireGuardTunnel::new().await.unwrap();
    let _multihop = multihop::MultiHopManager::new().await.unwrap();
    let _dns = dns::DnsManager::new().await.unwrap();
    let _st = split_tunnel::SplitTunnelManager::new().await.unwrap();

    system.refresh_process(pid);
    let final_memory = system.process(pid).map(|p| p.memory()).unwrap_or(0);
    let memory_increase = final_memory.saturating_sub(initial_memory);

    println!("Process RSS increase: {} bytes", memory_increase);
    // Memory usage should be reasonable (less than 100 MB for all components)
    assert!(memory_increase < 100 * 1024 * 1024);
}

#[tokio::test]
async fn test_timeout_handling() {
    let config = IronTunnelConfig::default();
    
    // Test that operations complete within reasonable time
    let result = timeout(Duration::from_secs(5), async {
        let mut client = IronTunnelClient::new(config).await.unwrap();
        client.connect().await.unwrap();
        client.disconnect().await.unwrap();
    }).await;
    
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_configuration_validation() {
    // Test different configuration combinations
    let configs = vec![
        IronTunnelConfig {
            enable_wireguard: true,
            enable_multihop: true,
            enable_dns_protection: true,
            enable_split_tunnel: true,
            ..IronTunnelConfig::default()
        },
        IronTunnelConfig {
            enable_wireguard: false,
            enable_multihop: false,
            enable_dns_protection: false,
            enable_split_tunnel: false,
            ..IronTunnelConfig::default()
        },
    ];
    
    for config in configs {
        let tunnel = IronTunnel::new(config).await.unwrap();
        assert_eq!(tunnel.get_connection_count().await, 0);
    }
}

#[tokio::test]
async fn test_feature_toggles() {
    // Test all feature combinations
    let feature_combinations = vec![
        (true, true, true, true),   // All features enabled
        (false, false, false, false), // All features disabled
        (true, false, true, false),   // Mixed configuration
        (false, true, false, true),   // Opposite mixed configuration
    ];
    
    for (wg, mh, dns, st) in feature_combinations {
        let config = IronTunnelConfig {
            enable_wireguard: wg,
            enable_multihop: mh,
            enable_dns_protection: dns,
            enable_split_tunnel: st,
            ..IronTunnelConfig::default()
        };
        
        let tunnel = IronTunnel::new(config.clone()).await.unwrap();
        let mut client = IronTunnelClient::new(config).await.unwrap();
        
        client.connect().await.unwrap();
        assert!(client.is_connected());
        
        client.disconnect().await.unwrap();
        assert!(!client.is_connected());
        
        assert_eq!(tunnel.get_connection_count().await, 0);
    }
}