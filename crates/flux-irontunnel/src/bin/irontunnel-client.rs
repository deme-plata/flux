//! Command-line client: connect, run the v0.0.2 handshake, print what the server
//! assigned. Upstream's README promised an `irontunnel-client` binary that never
//! existed; this is it, built on `flux_irontunnel::handshake`.
//!
//! It stops after TunnelReady because no packet path exists yet (see PORT.md).

use clap::Parser;
use flux_irontunnel::handshake::client_handshake;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(name = "irontunnel-client", about = "IronTunnel v0.0.2 handshake client (flux port)")]
struct Args {
    /// Server address, host:port
    #[arg(long, env = "IRONTUNNEL_SERVER", default_value = "127.0.0.1:1194")]
    server: String,
    /// Client id sent in CLIENT_AUTH (no ':' allowed)
    #[arg(long, default_value_t = format!("cli-{}", std::process::id()))]
    id: String,
    /// Per-step timeout in seconds
    #[arg(long, default_value_t = 10)]
    timeout: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    if args.id.contains(':') {
        return Err("--id must not contain ':' (the server splits CLIENT_AUTH on it)".into());
    }
    let t0 = Instant::now();
    let mut stream = tokio::net::TcpStream::connect(&args.server).await?;
    stream.set_nodelay(true)?;
    let out = client_handshake(&mut stream, &args.id, Duration::from_secs(args.timeout)).await?;
    println!("server        {}", args.server);
    println!("client_id     {}", out.client_id);
    println!("challenge     {}", String::from_utf8_lossy(&out.challenge));
    println!("server_ip     {}", out.server_ip);
    println!("assigned_ip   {}", out.assigned_ip);
    println!("dns_servers   {}", out.dns_servers.join(", "));
    println!("handshake_ms  {}", t0.elapsed().as_millis());
    println!("note          authenticated by a constant mock proof; link is NOT encrypted; no packets carried");
    Ok(())
}
