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
    
    // Configure client
    let config = IronTunnelConfig {
        server: "vpn.example.com".to_string(),
        port: 1194,
        auth: AuthMethod::ZkProof { identity: identity_data },
        tor_mode: TorMode::Direct,
        transport: Transport::Direct,
        enable_kill_switch: true,
        key_rotation_interval: 3600,
    };
    
    // Create and connect client
    let mut client = IronTunnelClient::new(config)?;
    
    println!("Connecting to VPN server...");
    client.connect().await?;
    
    println!("Connected! Press Ctrl+C to disconnect.");
    
    // Wait for shutdown signal
    tokio::signal::ctrl_c().await.expect("Failed to listen for ctrl+c");
    
    println!("Disconnecting...");
    client.disconnect().await?;
    
    println!("Disconnected.");
    Ok(())
}