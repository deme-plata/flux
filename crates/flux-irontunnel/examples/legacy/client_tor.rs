use flux_irontunnel::{
    IronTunnelConfig, IronTunnelClient, TorMode, Transport, AuthMethod, Result,
};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt::init();
    
    // Load identity from file
    let identity_data = std::fs::read("identity.zki")
        .expect("Failed to read identity file. Generate one first!");
    
    // Configure client with Tor integration
    let config = IronTunnelConfig {
        server: "3g2upl4pq6kufc4m.onion".to_string(), // Example onion address
        port: 1194,
        auth: AuthMethod::ZkProof { identity: identity_data },
        tor_mode: TorMode::OverTor, // Connect through Tor
        transport: Transport::Obfs4, // Use obfuscated transport
        enable_kill_switch: true,
        key_rotation_interval: 3600,
    };
    
    // Create and connect client
    let mut client = IronTunnelClient::new(config)?;
    
    println!("Connecting to VPN server through Tor...");
    println!("This may take longer than usual as Tor establishes circuits...");
    
    client.connect().await?;
    
    println!("Connected through Tor! Your traffic is now:");
    println!("  1. Encrypted by IronTunnel VPN");
    println!("  2. Routed through Tor network");
    println!("  3. Protected by ZK proof authentication");
    println!();
    println!("Press Ctrl+C to disconnect.");
    
    // Wait for shutdown signal
    tokio::signal::ctrl_c().await.expect("Failed to listen for ctrl+c");
    
    println!("Disconnecting...");
    client.disconnect().await?;
    
    println!("Disconnected.");
    Ok(())
}