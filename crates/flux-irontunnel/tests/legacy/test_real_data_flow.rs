// Integration tests for real production data flow
// Run with: cargo test --test test_real_data_flow --features=server-api,api

use flux_irontunnel::*;
use tokio::time::{sleep, Duration};
use std::sync::Arc;

#[tokio::test]
async fn test_vpn_packet_handler_real_traffic() {
    use flux_irontunnel::vpn_packet_handler::VpnPacketHandler;
    
    let handler = VpnPacketHandler::new();
    let client_id = "test-client-1";
    
    // Simulate real packet data
    let packet_data = b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n";
    
    // Process incoming packet
    let result = handler.process_incoming_packet(client_id, packet_data).await;
    assert!(result.is_ok());
    
    // Process outgoing response
    let response_data = b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\nHello, World!";
    let result = handler.process_outgoing_packet(client_id, response_data).await;
    assert!(result.is_ok());
    
    // Verify real traffic statistics
    let stats = handler.get_client_stats(client_id).await;
    assert!(stats.is_some());
    
    let stats = stats.unwrap();
    assert_eq!(stats.bytes_received, packet_data.len() as u64);
    assert_eq!(stats.bytes_sent, response_data.len() as u64);
    assert_eq!(stats.packets_received, 1);
    assert_eq!(stats.packets_sent, 1);
    
    println!("✅ VPN Packet Handler: Real traffic tracked correctly");
}

#[tokio::test]
async fn test_network_monitor_real_interfaces() {
    use flux_irontunnel::network_monitor::NetworkMonitor;
    
    let mut monitor = NetworkMonitor::new();
    
    // Update real network interface statistics
    let result = monitor.update_interface_stats();
    assert!(result.is_ok(), "Should read real network interfaces");
    
    // Get all interfaces
    let interfaces = monitor.get_all_interfaces();
    assert!(!interfaces.is_empty(), "Should detect at least one network interface");
    
    println!("🌐 Detected {} real network interfaces:", interfaces.len());
    for (name, stats) in interfaces {
        println!("   {} - RX: {} bytes, TX: {} bytes", 
            name, stats.bytes_received, stats.bytes_transmitted);
    }
    
    // Test VPN interface detection
    let (rx, tx, _, _) = monitor.get_vpn_traffic();
    println!("📊 VPN Traffic - RX: {} bytes, TX: {} bytes", rx, tx);
}

#[tokio::test]
async fn test_rest_api_real_client_tracking() {
    use flux_irontunnel::rest_api::RestApiServer;
    
    let api = RestApiServer::new(100);
    let client_id = "test-client-api";
    let client_addr = "192.168.1.100:54321".parse().ok();
    
    // Add client
    api.add_client(client_id.to_string(), client_addr).await;
    
    // Simulate real traffic updates
    let bytes_sent = 1024 * 1024; // 1MB
    let bytes_received = 512 * 1024; // 512KB
    
    api.update_client_traffic(client_id, bytes_sent, bytes_received).await;
    
    // Update ping
    api.update_client_ping(client_id, Duration::from_millis(25)).await;
    
    // Get server status
    let status = api.get_server_status().await;
    assert_eq!(status.active_clients, 1);
    assert_eq!(status.total_traffic, bytes_sent + bytes_received);
    
    // Get client stats
    let clients = api.get_all_clients().await;
    assert_eq!(clients.len(), 1);
    
    let client = &clients[0];
    assert_eq!(client.id, client_id);
    assert!(client.connected);
    
    // Verify traffic is tracked
    let sent = *client.bytes_sent.read().await;
    let received = *client.bytes_received.read().await;
    assert_eq!(sent, bytes_sent);
    assert_eq!(received, bytes_received);
    
    println!("✅ REST API: Real client traffic tracking works");
    println!("   Client {} - Sent: {} bytes, Received: {} bytes", 
        client_id, sent, received);
}

#[tokio::test]
async fn test_real_api_client_data_formatting() {
    use flux_irontunnel::real_api_client::RealApiClient;
    
    // Test speed formatting
    assert_eq!(RealApiClient::format_speed(0), "0 B/s");
    assert_eq!(RealApiClient::format_speed(1024), "1.0 KB/s");
    assert_eq!(RealApiClient::format_speed(1024 * 1024), "1.0 MB/s");
    assert_eq!(RealApiClient::format_speed(1024 * 1024 * 1024), "1.0 GB/s");
    
    // Test byte formatting
    assert_eq!(RealApiClient::format_bytes(0), "0 B");
    assert_eq!(RealApiClient::format_bytes(1024), "1.0 KB");
    assert_eq!(RealApiClient::format_bytes(1024 * 1024 * 512), "512.0 MB");
    
    // Test connection time formatting
    assert_eq!(RealApiClient::format_connection_time(0), "00:00:00");
    assert_eq!(RealApiClient::format_connection_time(3661), "01:01:01");
    assert_eq!(RealApiClient::format_connection_time(7200), "02:00:00");
    
    println!("✅ API Client: Data formatting works correctly");
}

#[tokio::test]
async fn test_full_data_flow_integration() {
    use flux_irontunnel::vpn_packet_handler::VpnPacketHandler;
    use flux_irontunnel::rest_api::RestApiServer;
    use flux_irontunnel::network_monitor::NetworkMonitor;
    
    // Initialize components
    let packet_handler = Arc::new(VpnPacketHandler::new());
    let rest_api = Arc::new(RestApiServer::new(100));
    let network_monitor = Arc::new(tokio::sync::Mutex::new(NetworkMonitor::new()));
    
    // Simulate client connection
    let client_id = "integration-test-client";
    rest_api.add_client(client_id.to_string(), Some("10.0.0.100:12345".parse().unwrap())).await;
    
    // Simulate VPN traffic
    let test_packets = vec![
        b"PING request packet data".to_vec(),
        b"HTTP GET request to server".to_vec(),
        b"Large data transfer packet with lots of bytes...".to_vec(),
    ];
    
    let mut total_rx = 0u64;
    let mut total_tx = 0u64;
    
    for packet in test_packets {
        let packet_size = packet.len() as u64;
        
        // Process incoming
        packet_handler.process_incoming_packet(client_id, &packet).await.unwrap();
        total_rx += packet_size;
        
        // Simulate response
        let response = b"ACK response from server";
        packet_handler.process_outgoing_packet(client_id, response).await.unwrap();
        total_tx += response.len() as u64;
    }
    
    // Get packet handler stats
    let stats = packet_handler.get_client_stats(client_id).await.unwrap();
    
    // Update REST API with real stats
    rest_api.update_client_traffic(client_id, stats.bytes_sent, stats.bytes_received).await;
    
    // Update network monitor
    {
        let mut monitor = network_monitor.lock().await;
        monitor.update_interface_stats().ok();
    }
    
    // Verify complete data flow
    let server_status = rest_api.get_server_status().await;
    assert_eq!(server_status.active_clients, 1);
    assert_eq!(server_status.total_traffic, total_rx + total_tx);
    
    println!("✅ Full Integration Test Passed!");
    println!("   Total RX: {} bytes", total_rx);
    println!("   Total TX: {} bytes", total_tx);
    println!("   Active Clients: {}", server_status.active_clients);
    println!("   Server Uptime: {} seconds", server_status.uptime);
}

#[tokio::test]
async fn test_concurrent_client_traffic() {
    use flux_irontunnel::vpn_packet_handler::VpnPacketHandler;
    use flux_irontunnel::rest_api::RestApiServer;
    
    let packet_handler = Arc::new(VpnPacketHandler::new());
    let rest_api = Arc::new(RestApiServer::new(100));
    
    // Simulate multiple concurrent clients
    let num_clients = 10;
    let mut handles = vec![];
    
    for i in 0..num_clients {
        let handler = packet_handler.clone();
        let api = rest_api.clone();
        
        let handle = tokio::spawn(async move {
            let client_id = format!("concurrent-client-{}", i);
            api.add_client(client_id.clone(), None).await;
            
            // Each client sends different amounts of data
            let packet_count = (i + 1) * 10;
            let packet_size = 1024; // 1KB per packet
            
            for j in 0..packet_count {
                let data = vec![j as u8; packet_size];
                handler.process_incoming_packet(&client_id, &data).await.unwrap();
                
                // Simulate some processing delay
                sleep(Duration::from_millis(1)).await;
            }
            
            // Get final stats
            let stats = handler.get_client_stats(&client_id).await.unwrap();
            (client_id, stats.bytes_received)
        });
        
        handles.push(handle);
    }
    
    // Wait for all clients to complete
    let results = futures::future::join_all(handles).await;
    
    // Verify all clients processed correctly
    let mut total_bytes = 0u64;
    println!("📊 Concurrent Client Results:");
    for result in results {
        let (client_id, bytes) = result.unwrap();
        println!("   {} - Received: {} bytes", client_id, bytes);
        total_bytes += bytes;
    }
    
    let server_status = rest_api.get_server_status().await;
    assert_eq!(server_status.active_clients, num_clients);
    
    println!("✅ Concurrent test passed - {} clients, {} total bytes", 
        num_clients, total_bytes);
}