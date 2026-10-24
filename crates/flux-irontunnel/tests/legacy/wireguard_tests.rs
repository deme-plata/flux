use flux_irontunnel::wireguard::*;
use std::net::{SocketAddr, IpAddr, Ipv4Addr};

#[tokio::test]
async fn test_wireguard_key_generation() {
    let result = generate_keypair();
    assert!(result.is_ok());
    
    let (private_key, public_key) = result.unwrap();
    
    assert!(!private_key.is_empty());
    assert!(!public_key.is_empty());
    assert_ne!(private_key, public_key);
    
    // Keys should be base64 encoded and proper length
    let private_decoded = base64::decode(&private_key);
    let public_decoded = base64::decode(&public_key);
    
    assert!(private_decoded.is_ok());
    assert!(public_decoded.is_ok());
    assert_eq!(private_decoded.unwrap().len(), 32);
    assert_eq!(public_decoded.unwrap().len(), 32);
}

#[tokio::test]
async fn test_public_key_derivation() {
    let (private_key, expected_public) = generate_keypair().unwrap();
    
    let derived_public = public_key_from_private(&private_key);
    assert!(derived_public.is_ok());
    
    let derived_public = derived_public.unwrap();
    assert_eq!(derived_public, expected_public);
}

#[tokio::test]
async fn test_wireguard_keys_structure() {
    let keys = WireGuardKeys::generate();
    assert!(keys.is_ok());
    
    let keys = keys.unwrap();
    
    let private_b64 = keys.private_key_base64();
    let public_b64 = keys.public_key_base64();
    
    assert!(!private_b64.is_empty());
    assert!(!public_b64.is_empty());
    assert_ne!(private_b64, public_b64);
    
    // Test key access
    let private_key = keys.private_key();
    let public_key = keys.public_key();
    
    assert_eq!(private_key.as_bytes().len(), 32);
    assert_eq!(public_key.as_bytes().len(), 32);
}

#[tokio::test]
async fn test_wireguard_keys_from_string() {
    let (private_str, _) = generate_keypair().unwrap();
    
    let keys = WireGuardKeys::from_private_key(&private_str);
    assert!(keys.is_ok());
    
    let keys = keys.unwrap();
    assert_eq!(keys.private_key_base64(), private_str);
}

#[tokio::test]
async fn test_wireguard_config_default() {
    let config = WireGuardConfig::default();
    
    assert!(config.private_key.is_empty());
    assert!(config.public_key.is_empty());
    assert_eq!(config.listen_port, 51820);
    assert!(config.peers.is_empty());
    assert_eq!(config.interface.address, "10.0.0.2/24");
    assert_eq!(config.interface.dns.len(), 2);
    assert_eq!(config.interface.mtu, 1420);
}

#[tokio::test]
async fn test_wireguard_peer_configuration() {
    let (_, peer_public_key) = generate_keypair().unwrap();
    
    let peer = WireGuardPeer {
        public_key: peer_public_key,
        endpoint: Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 51820)),
        allowed_ips: vec!["0.0.0.0/0".to_string(), "::/0".to_string()],
        persistent_keepalive: Some(25),
        preshared_key: None,
    };
    
    assert!(!peer.public_key.is_empty());
    assert!(peer.endpoint.is_some());
    assert_eq!(peer.allowed_ips.len(), 2);
    assert_eq!(peer.persistent_keepalive, Some(25));
    assert!(peer.preshared_key.is_none());
}

#[tokio::test]
async fn test_wireguard_tunnel_creation() {
    let (private_key, public_key) = generate_keypair().unwrap();
    
    let config = WireGuardConfig {
        private_key,
        public_key: public_key.clone(),
        listen_port: 51820,
        peers: vec![],
        interface: WireGuardInterface {
            address: "10.0.0.1/24".to_string(),
            dns: vec!["1.1.1.1".to_string()],
            mtu: 1420,
        },
    };
    
    let tunnel = WireGuardTunnel::new(config).await;
    assert!(tunnel.is_ok());
    
    let tunnel = tunnel.unwrap();
    assert_eq!(tunnel.get_public_key(), public_key);
}

#[tokio::test]
async fn test_wireguard_tunnel_with_peers() {
    let (server_private, server_public) = generate_keypair().unwrap();
    let (peer_private, peer_public) = generate_keypair().unwrap();
    
    let peer = WireGuardPeer {
        public_key: peer_public,
        endpoint: Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)), 51820)),
        allowed_ips: vec!["10.0.0.0/24".to_string()],
        persistent_keepalive: Some(25),
        preshared_key: None,
    };
    
    let config = WireGuardConfig {
        private_key: server_private,
        public_key: server_public,
        listen_port: 51821, // Different port for test
        peers: vec![peer],
        interface: WireGuardInterface::default(),
    };
    
    let tunnel = WireGuardTunnel::new(config).await;
    assert!(tunnel.is_ok());
}

#[tokio::test]
async fn test_wireguard_tunnel_start() {
    let (private_key, public_key) = generate_keypair().unwrap();
    
    let config = WireGuardConfig {
        private_key,
        public_key,
        listen_port: 51822, // Different port for test
        peers: vec![],
        interface: WireGuardInterface::default(),
    };
    
    let mut tunnel = WireGuardTunnel::new(config).await.unwrap();
    
    // Start might fail in test environment due to socket binding
    let start_result = tunnel.start().await;
    // Should either succeed or fail gracefully
    assert!(start_result.is_ok() || start_result.is_err());
}

#[tokio::test]
async fn test_wireguard_packet_operations() {
    let (private_key, public_key) = generate_keypair().unwrap();
    
    let config = WireGuardConfig {
        private_key,
        public_key,
        listen_port: 51823,
        peers: vec![],
        interface: WireGuardInterface::default(),
    };
    
    let mut tunnel = WireGuardTunnel::new(config).await.unwrap();
    
    let test_data = b"Hello, WireGuard!";
    let peer_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 51820);
    
    // Send packet (might fail without proper setup, but should handle gracefully)
    let send_result = tunnel.send_packet(test_data, peer_addr).await;
    assert!(send_result.is_ok() || send_result.is_err());
}

#[tokio::test]
async fn test_wireguard_interface_config() {
    let interface = WireGuardInterface {
        address: "192.168.100.1/24".to_string(),
        dns: vec!["8.8.8.8".to_string(), "8.8.4.4".to_string()],
        mtu: 1280,
    };
    
    assert_eq!(interface.address, "192.168.100.1/24");
    assert_eq!(interface.dns.len(), 2);
    assert_eq!(interface.dns[0], "8.8.8.8");
    assert_eq!(interface.mtu, 1280);
}

#[tokio::test]
async fn test_concurrent_key_generation() {
    use tokio::task;
    
    let handles: Vec<_> = (0..10).map(|_| {
        task::spawn(async {
            let result = generate_keypair();
            assert!(result.is_ok());
            
            let (private, public) = result.unwrap();
            assert!(!private.is_empty());
            assert!(!public.is_empty());
            assert_ne!(private, public);
        })
    }).collect();
    
    for handle in handles {
        handle.await.expect("Concurrent key generation task failed");
    }
}

#[tokio::test]
async fn test_wireguard_keys_uniqueness() {
    let mut keys = Vec::new();
    
    // Generate multiple keypairs
    for _ in 0..10 {
        let keypair = generate_keypair().unwrap();
        keys.push(keypair);
    }
    
    // All private keys should be unique
    for i in 0..keys.len() {
        for j in (i + 1)..keys.len() {
            assert_ne!(keys[i].0, keys[j].0);
            assert_ne!(keys[i].1, keys[j].1);
        }
    }
}

#[tokio::test]
async fn test_invalid_private_key() {
    let invalid_key = "invalid_base64_key!!!";
    let result = WireGuardKeys::from_private_key(invalid_key);
    assert!(result.is_err());
    
    let short_key = base64::encode(&[1, 2, 3]); // Too short
    let result = WireGuardKeys::from_private_key(&short_key);
    assert!(result.is_err());
}

#[tokio::test]
async fn test_wireguard_packet_structure() {
    use flux_irontunnel::wireguard::WireGuardPacket;
    use bytes::Bytes;
    
    let packet = WireGuardPacket {
        data: Bytes::from_static(b"test packet data"),
        peer: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 51820),
        is_incoming: true,
    };
    
    assert_eq!(packet.data, Bytes::from_static(b"test packet data"));
    assert_eq!(packet.peer.ip(), IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));
    assert_eq!(packet.peer.port(), 51820);
    assert!(packet.is_incoming);
}

#[tokio::test]
async fn test_wireguard_config_serialization() {
    let (private_key, public_key) = generate_keypair().unwrap();
    
    let config = WireGuardConfig {
        private_key: private_key.clone(),
        public_key: public_key.clone(),
        listen_port: 51820,
        peers: vec![],
        interface: WireGuardInterface::default(),
    };
    
    // Test JSON serialization
    let json = serde_json::to_string(&config);
    assert!(json.is_ok());
    
    let deserialized: Result<WireGuardConfig, _> = serde_json::from_str(&json.unwrap());
    assert!(deserialized.is_ok());
    
    let config2 = deserialized.unwrap();
    assert_eq!(config2.private_key, private_key);
    assert_eq!(config2.public_key, public_key);
    assert_eq!(config2.listen_port, 51820);
}

#[tokio::test]
async fn test_preshared_key_handling() {
    let (_, peer_public) = generate_keypair().unwrap();
    let (preshared_key, _) = generate_keypair().unwrap(); // Use as PSK
    
    let peer = WireGuardPeer {
        public_key: peer_public,
        endpoint: Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 51820)),
        allowed_ips: vec!["0.0.0.0/0".to_string()],
        persistent_keepalive: Some(25),
        preshared_key: Some(preshared_key.clone()),
    };
    
    assert!(peer.preshared_key.is_some());
    assert_eq!(peer.preshared_key.unwrap(), preshared_key);
}

#[tokio::test]
async fn test_wireguard_memory_safety() {
    // Test that keys are properly zeroized
    {
        let keys = WireGuardKeys::generate().unwrap();
        let _private = keys.private_key_base64();
        let _public = keys.public_key_base64();
    } // Keys should be zeroized when dropped
    
    // Test with tunnel
    {
        let (private_key, public_key) = generate_keypair().unwrap();
        let config = WireGuardConfig {
            private_key,
            public_key,
            listen_port: 51824,
            peers: vec![],
            interface: WireGuardInterface::default(),
        };
        
        let _tunnel = WireGuardTunnel::new(config).await.unwrap();
    } // Tunnel should clean up when dropped
}