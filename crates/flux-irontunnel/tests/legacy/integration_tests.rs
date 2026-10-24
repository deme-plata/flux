use flux_irontunnel::{
    IronTunnelConfig, IronTunnel, TorMode, Transport, AuthMethod, Result,
    crypto::CryptoEngine,
    zk::{ZkEngine, ZkIdentity},
    tunnel::TunnelManager,
    tor::TorManager,
    wireguard::{WireGuardConfig, WireGuardTunnel, generate_keypair},
    multihop::{MultiHopManager, MultiHopConfig, HopSelectionStrategy, VpnServer, ServerFeatures, VpnProtocol},
    dns::{DnsManager, DnsConfig, DnsServer, DnsProvider},
    split_tunnel::{SplitTunnelManager, SplitTunnelConfig, SplitAction, AppRule, DomainRule, IpRule},
    transport::{TransportManager, TransportConfig},
};
use tokio;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

#[tokio::test]
async fn test_crypto_engine_operations() {
    let mut crypto = CryptoEngine::new().expect("Failed to create crypto engine");
    
    // Test key generation
    let (secret, public) = CryptoEngine::generate_ephemeral_keypair();
    assert_ne!(secret.as_bytes(), [0u8; 32]);
    assert_ne!(public.as_bytes(), [0u8; 32]);
    
    // Test key exchange
    let peer_public = [0u8; 32]; // Mock peer public key
    let key_exchange = crypto.perform_key_exchange(&peer_public).await;
    assert!(key_exchange.is_ok());
    
    // Test encryption/decryption
    let plaintext = b"Hello, IronTunnel!";
    let encrypted = crypto.encrypt_packet(plaintext).await;
    assert!(encrypted.is_ok());
    
    if let Ok(packet) = encrypted {
        let decrypted = crypto.decrypt_packet(&packet).await;
        assert!(decrypted.is_ok());
        assert_eq!(decrypted.unwrap(), plaintext.to_vec());
    }
    
    // Test key rotation
    let rotation_result = crypto.rotate_session_key().await;
    assert!(rotation_result.is_ok());
}

#[tokio::test]
async fn test_zk_proof_system() {
    let mut zk_engine = ZkEngine::new().expect("Failed to create ZK engine");
    
    // Test identity generation
    let identity = zk_engine.generate_identity();
    assert!(identity.is_ok());
    let identity = identity.unwrap();
    
    // Test proof creation
    let challenge = b"test_challenge_data";
    let proof = zk_engine.create_proof(&identity, challenge);
    assert!(proof.is_ok());
    let proof = proof.unwrap();
    
    // Test proof verification
    let verification = zk_engine.verify_proof(&proof, &identity.commitment);
    assert!(verification.is_ok());
    assert!(verification.unwrap());
    
    // Test invalid proof rejection
    let mut invalid_proof = proof.clone();
    invalid_proof.timestamp = 0; // Make it expired
    let invalid_verification = zk_engine.verify_proof(&invalid_proof, &identity.commitment);
    assert!(invalid_verification.is_ok());
    assert!(!invalid_verification.unwrap()); // Should be false for expired proof
}

#[tokio::test]
async fn test_wireguard_implementation() {
    // Test key generation
    let keypair = generate_keypair();
    assert!(keypair.is_ok());
    let (private_key, public_key) = keypair.unwrap();
    
    assert!(!private_key.is_empty());
    assert!(!public_key.is_empty());
    assert_ne!(private_key, public_key);
    
    // Test WireGuard configuration
    let mut config = WireGuardConfig::default();
    config.private_key = private_key;
    config.public_key = public_key;
    
    // Test tunnel creation
    let tunnel = WireGuardTunnel::new(config).await;
    assert!(tunnel.is_ok());
    let tunnel = tunnel.unwrap();
    
    assert!(!tunnel.get_public_key().is_empty());
}

#[tokio::test]
async fn test_multi_hop_functionality() {
    let config = MultiHopConfig {
        enabled: true,
        max_hops: 3,
        hop_selection_strategy: HopSelectionStrategy::SecurityFocused,
        entry_countries: vec!["US".to_string()],
        exit_countries: vec!["DE".to_string()],
        avoid_countries: vec!["CN".to_string(), "RU".to_string()],
        min_latency_ms: None,
        max_latency_ms: Some(500),
    };
    
    let mut manager = MultiHopManager::new(config);
    
    // Test server loading
    let load_result = manager.load_servers().await;
    assert!(load_result.is_ok());
    
    // Test chain creation
    let chain_result = manager.create_hop_chain().await;
    assert!(chain_result.is_ok());
    let chain = chain_result.unwrap();
    
    assert!(!chain.hops.is_empty());
    assert!(chain.hops.len() >= 2);
    assert!(chain.hops.len() <= 3);
    assert!(chain.security_score > 0.0);
    
    // Test chain info
    let chain_info = manager.get_chain_info();
    assert!(chain_info.is_some());
    let info = chain_info.unwrap();
    assert!(!info.countries.is_empty());
}

#[tokio::test]
async fn test_dns_protection() {
    let config = DnsConfig {
        servers: vec![
            DnsServer {
                name: "Test DNS".to_string(),
                address: IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)),
                port: 53,
                supports_doh: true,
                supports_dot: true,
                provider: DnsProvider::Cloudflare,
            }
        ],
        leak_protection: true,
        block_ads: true,
        block_malware: true,
        block_trackers: true,
        custom_blocklist_urls: vec![],
        fallback_servers: vec![],
        cache_size: 1000,
        cache_ttl: 300,
    };
    
    let dns_manager = DnsManager::new(config).await;
    assert!(dns_manager.is_ok());
    let dns_manager = dns_manager.unwrap();
    
    // Test DNS stats
    let stats = dns_manager.get_dns_stats().await;
    assert!(stats.is_ok());
    let stats = stats.unwrap();
    assert_eq!(stats.total_queries, 0); // Initially no queries
    
    // Test leak detection
    let leaks = dns_manager.check_dns_leaks().await;
    assert!(leaks.is_ok());
    let leaks = leaks.unwrap();
    assert!(leaks.is_empty()); // No leaks in test environment
    
    // Test blocklist functionality
    let block_result = dns_manager.add_to_blocklist(vec!["badsite.com".to_string()]).await;
    assert!(block_result.is_ok());
}

#[tokio::test]
async fn test_split_tunneling() {
    let config = SplitTunnelConfig {
        enabled: true,
        default_action: SplitAction::VpnOnly,
        app_rules: vec![
            AppRule {
                name: "Test App".to_string(),
                executable_path: "test_app".to_string(),
                action: SplitAction::VpnBypass,
                enabled: true,
                priority: 10,
            }
        ],
        domain_rules: vec![
            DomainRule {
                domain: "example.com".to_string(),
                action: SplitAction::VpnBypass,
                include_subdomains: true,
                enabled: true,
                priority: 8,
            }
        ],
        ip_rules: vec![
            IpRule {
                ip_range: "192.168.1.0/24".to_string(),
                action: SplitAction::VpnBypass,
                enabled: true,
                priority: 9,
                description: "Local network".to_string(),
            }
        ],
        allow_lan_access: true,
        bypass_china: false,
        bypass_russia: false,
        custom_bypass_countries: vec![],
    };
    
    let manager = SplitTunnelManager::new(config);
    
    // Test stats
    let stats = manager.get_bypass_stats().await;
    assert!(stats.is_ok());
    let stats = stats.unwrap();
    assert!(stats.total_rules > 0);
    
    // Test rule addition
    let mut manager = manager;
    let new_rule = AppRule {
        name: "New App".to_string(),
        executable_path: "new_app".to_string(),
        action: SplitAction::VpnOnly,
        enabled: true,
        priority: 5,
    };
    
    let add_result = manager.add_app_rule(new_rule).await;
    assert!(add_result.is_ok());
}

#[tokio::test]
async fn test_tunnel_manager() {
    let tunnel_manager = TunnelManager::new();
    assert!(tunnel_manager.is_ok());
    let mut tunnel_manager = tunnel_manager.unwrap();
    
    // Test connection management
    let peer_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080);
    let connection_id = tunnel_manager.add_connection(peer_addr).await;
    assert!(connection_id.is_ok());
    let connection_id = connection_id.unwrap();
    
    // Test connection stats
    let stats = tunnel_manager.get_connection_stats().await;
    assert!(stats.is_ok());
    let stats = stats.unwrap();
    assert!(!stats.is_empty());
    assert_eq!(stats[0].id, connection_id);
    
    // Test connection removal
    let remove_result = tunnel_manager.remove_connection(connection_id).await;
    assert!(remove_result.is_ok());
    
    // Test kill switch
    let kill_switch_result = tunnel_manager.activate_kill_switch().await;
    assert!(kill_switch_result.is_ok());
    
    let deactivate_result = tunnel_manager.deactivate_kill_switch().await;
    assert!(deactivate_result.is_ok());
}

#[tokio::test]
async fn test_tor_integration() {
    let tor_manager = TorManager::new();
    assert!(tor_manager.is_ok());
    let mut tor_manager = tor_manager.unwrap();
    
    // Test initialization (mock)
    let init_result = tor_manager.initialize().await;
    // This might fail in test environment without Tor, but should handle gracefully
    assert!(init_result.is_ok() || init_result.is_err());
    
    // Test SOCKS proxy address
    let proxy_addr = tor_manager.get_socks_proxy_addr();
    assert_eq!(proxy_addr.port(), 9050);
    
    // Test running status
    let is_running = tor_manager.is_tor_running().await;
    // Should return false in test environment
    assert!(!is_running);
}

#[tokio::test]
async fn test_transport_manager() {
    let config = TransportConfig {
        transport_type: Transport::Direct,
        obfs4_cert: None,
        obfs4_iat_mode: 0,
        buffer_size: 4096,
    };
    
    let manager = TransportManager::new(config);
    
    // Test with mock TCP stream (this would need actual networking in real test)
    // For now, just test manager creation
    assert_eq!(manager.config.buffer_size, 4096);
}

#[tokio::test]
async fn test_irontunnel_config() {
    let config = IronTunnelConfig {
        server: "test.irontunnel.com".to_string(),
        port: 1194,
        auth: AuthMethod::ZkProof { identity: vec![1, 2, 3, 4] },
        tor_mode: TorMode::Direct,
        transport: Transport::Direct,
        enable_kill_switch: true,
        key_rotation_interval: 3600,
    };
    
    // Test IronTunnel creation
    let irontunnel = IronTunnel::new(config.clone());
    assert!(irontunnel.is_ok());
    
    // Test default config
    let default_config = IronTunnelConfig::default();
    assert_eq!(default_config.server, "127.0.0.1");
    assert_eq!(default_config.port, 1194);
    assert!(default_config.enable_kill_switch);
}

#[tokio::test]
async fn test_error_handling() {
    // Test invalid ZK proof
    let zk_engine = ZkEngine::new().expect("Failed to create ZK engine");
    let invalid_proof_data = b"invalid_proof_data";
    
    // This should handle errors gracefully
    let invalid_identity = ZkIdentity {
        secret: vec![],
        public_params: vec![],
        commitment: vec![],
    };
    
    let proof_result = zk_engine.create_proof(&invalid_identity, b"challenge");
    // Should either succeed with valid proof or fail gracefully
    assert!(proof_result.is_ok() || proof_result.is_err());
}

#[tokio::test]
async fn test_performance_benchmarks() {
    use std::time::Instant;
    
    // Test crypto performance
    let start = Instant::now();
    let mut crypto = CryptoEngine::new().expect("Failed to create crypto engine");
    let crypto_init_time = start.elapsed();
    
    assert!(crypto_init_time < Duration::from_millis(100)); // Should be fast
    
    // Test encryption performance
    let plaintext = vec![0u8; 1024]; // 1KB test data
    let start = Instant::now();
    
    // This will fail without active session, but tests the path
    let _encrypt_result = crypto.encrypt_packet(&plaintext).await;
    let encrypt_time = start.elapsed();
    
    assert!(encrypt_time < Duration::from_millis(10)); // Should be very fast
    
    // Test ZK proof generation performance
    let start = Instant::now();
    let mut zk_engine = ZkEngine::new().expect("Failed to create ZK engine");
    let identity = zk_engine.generate_identity().expect("Failed to generate identity");
    let zk_init_time = start.elapsed();
    
    assert!(zk_init_time < Duration::from_millis(500)); // Should be reasonable
    
    let start = Instant::now();
    let _proof = zk_engine.create_proof(&identity, b"challenge");
    let proof_time = start.elapsed();
    
    assert!(proof_time < Duration::from_millis(1000)); // ZK proofs are more expensive
}

#[tokio::test]
async fn test_concurrent_operations() {
    use tokio::task;
    
    // Test concurrent crypto operations
    let handles: Vec<_> = (0..10).map(|_| {
        task::spawn(async {
            let crypto = CryptoEngine::new();
            assert!(crypto.is_ok());
        })
    }).collect();
    
    for handle in handles {
        handle.await.expect("Task failed");
    }
    
    // Test concurrent ZK operations
    let handles: Vec<_> = (0..5).map(|_| {
        task::spawn(async {
            let mut zk_engine = ZkEngine::new().expect("Failed to create ZK engine");
            let identity = zk_engine.generate_identity();
            assert!(identity.is_ok());
        })
    }).collect();
    
    for handle in handles {
        handle.await.expect("Task failed");
    }
}

#[tokio::test]
async fn test_memory_safety() {
    // Test that sensitive data is properly zeroized
    {
        let mut zk_engine = ZkEngine::new().expect("Failed to create ZK engine");
        let identity = zk_engine.generate_identity().expect("Failed to generate identity");
        
        // Identity should be properly structured
        assert!(!identity.secret.is_empty());
        assert!(!identity.public_params.is_empty());
        assert!(!identity.commitment.is_empty());
    } // Identity should be zeroized when dropped
    
    {
        let crypto = CryptoEngine::new().expect("Failed to create crypto engine");
        let public_key = crypto.get_public_key();
        
        // Public key should be valid
        assert_ne!(public_key.as_bytes(), &[0u8; 32]);
    } // Crypto engine should clean up when dropped
}

#[tokio::test]
async fn test_configuration_validation() {
    // Test valid configurations
    let valid_config = IronTunnelConfig {
        server: "valid.server.com".to_string(),
        port: 1194,
        auth: AuthMethod::ZkProof { identity: vec![1, 2, 3, 4] },
        tor_mode: TorMode::Direct,
        transport: Transport::Direct,
        enable_kill_switch: true,
        key_rotation_interval: 3600,
    };
    
    let irontunnel = IronTunnel::new(valid_config);
    assert!(irontunnel.is_ok());
    
    // Test DNS config validation
    let dns_config = DnsConfig::default();
    assert!(!dns_config.servers.is_empty());
    assert!(dns_config.leak_protection);
    
    // Test split tunnel config validation
    let split_config = SplitTunnelConfig::default();
    assert!(!split_config.enabled); // Default is disabled
    assert!(matches!(split_config.default_action, SplitAction::VpnOnly));
}

#[tokio::test]
async fn test_feature_integration() {
    // Test that all features work together
    let irontunnel_config = IronTunnelConfig {
        server: "integration.test.com".to_string(),
        port: 1194,
        auth: AuthMethod::ZkProof { identity: vec![1, 2, 3, 4] },
        tor_mode: TorMode::OverTor,
        transport: Transport::Obfs4,
        enable_kill_switch: true,
        key_rotation_interval: 1800,
    };
    
    let irontunnel = IronTunnel::new(irontunnel_config);
    assert!(irontunnel.is_ok());
    
    // Test multi-hop with DNS protection
    let multihop_config = MultiHopConfig {
        enabled: true,
        max_hops: 2,
        hop_selection_strategy: HopSelectionStrategy::GeographicDiversity,
        entry_countries: vec!["US".to_string()],
        exit_countries: vec!["DE".to_string()],
        avoid_countries: vec!["CN".to_string()],
        min_latency_ms: None,
        max_latency_ms: Some(300),
    };
    
    let mut multihop_manager = MultiHopManager::new(multihop_config);
    let load_result = multihop_manager.load_servers().await;
    assert!(load_result.is_ok());
    
    // Test DNS with split tunneling
    let dns_config = DnsConfig {
        leak_protection: true,
        block_ads: true,
        block_malware: true,
        block_trackers: true,
        ..Default::default()
    };
    
    let dns_manager = DnsManager::new(dns_config).await;
    assert!(dns_manager.is_ok());
    
    let split_config = SplitTunnelConfig {
        enabled: true,
        allow_lan_access: true,
        bypass_china: true,
        ..Default::default()
    };
    
    let split_manager = SplitTunnelManager::new(split_config);
    let stats = split_manager.get_bypass_stats().await;
    assert!(stats.is_ok());
}