//! IronTunnel v1 client: a SOCKS5 proxy on loopback, tunnelled to a relay.
//!
//!   irontunnel-socks --relay HOST:1195 --key client.key --relay-key <hex>
//!   curl --socks5-hostname 127.0.0.1:1080 https://example.org
//!
//! `--probe` checks the relay (handshake + encrypted ping) and exits.

use clap::Parser;
use flux_irontunnel::proxy::{probe, start_proxy, ProxyConfig};
use flux_irontunnel::secure::{PublicKey, StaticKeypair};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "irontunnel-socks", about = "IronTunnel v1 local SOCKS5 proxy")]
struct Args {
    #[arg(long, default_value = "127.0.0.1:1080")]
    listen: SocketAddr,
    /// Relay address host:port
    #[arg(long, env = "IRONTUNNEL_RELAY")]
    relay: String,
    /// Client private key file (irontunnel-keygen)
    #[arg(long)]
    key: PathBuf,
    /// The relay's public key (hex), pinned
    #[arg(long, env = "IRONTUNNEL_RELAY_KEY")]
    relay_key: String,
    /// Only check that the relay answers, then exit
    #[arg(long)]
    probe: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().init();
    let args = Args::parse();
    let mut cfg = ProxyConfig::new(args.relay, StaticKeypair::load(&args.key)?, PublicKey::from_hex(&args.relay_key)?);
    cfg.listen = args.listen;
    let rtt = probe(&cfg).await?;
    println!("relay {} up: authenticated handshake + encrypted ping in {} ms", cfg.relay, rtt.as_millis());
    if args.probe {
        return Ok(());
    }
    let proxy = start_proxy(cfg).await?;
    println!("SOCKS5 on {}  (url {})  — UDP is not tunnelled", proxy.local_addr(), proxy.socks5_url());
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
        let s = proxy.stats();
        println!(
            "connections {} (active {}, failed {})  up {} B  down {} B",
            s.connections_total, s.connections_active, s.failures, s.bytes_up, s.bytes_down
        );
    }
}
