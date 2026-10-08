//! IronTunnel v1 relay: accepts Noise-IK tunnels from authorized client keys and
//! connects out to public internet addresses on their behalf.
//!
//! Binds 127.0.0.1 by default; exposing it is a deliberate `--listen 0.0.0.0:PORT`.
//! Unlike the v0.0.2 server it touches no iptables and needs no root.

use clap::Parser;
use flux_irontunnel::relay::{parse_authorized, start_relay, RelayConfig};
use flux_irontunnel::secure::StaticKeypair;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "irontunnel-relay", about = "IronTunnel v1 relay (Noise IK + egress guard)")]
struct Args {
    #[arg(long, default_value = "127.0.0.1:1195")]
    listen: SocketAddr,
    /// Relay private key file (irontunnel-keygen)
    #[arg(long)]
    key: PathBuf,
    /// File of authorized client public keys, one hex key per line
    #[arg(long)]
    authorized: PathBuf,
    /// Also allow loopback/private targets (testing only — exposes the relay host)
    #[arg(long)]
    allow_private: bool,
    /// Extra ports to refuse (25 is always refused)
    #[arg(long = "deny-port")]
    deny_ports: Vec<u16>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().init();
    let args = Args::parse();
    let key = StaticKeypair::load(&args.key)?;
    let authorized = parse_authorized(&std::fs::read_to_string(&args.authorized)?)?;
    if authorized.is_empty() {
        return Err("authorized-clients file is empty: nobody could connect".into());
    }
    println!("relay public key {}", key.public());
    let mut cfg = RelayConfig::new(args.listen, key, authorized);
    cfg.egress.allow_private = args.allow_private;
    cfg.egress.deny_ports.extend(args.deny_ports);
    let n_auth = cfg.authorized.len();
    let relay = start_relay(cfg).await?;
    println!("listening {}  authorized clients {}  allow_private {}", relay.local_addr, n_auth, args.allow_private);
    let st = relay.stats.clone();
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
        println!(
            "handshakes ok/refused {}/{}  connects ok/denied/failed {}/{}/{}  bytes to/from targets {}/{}",
            st.handshakes_ok.load(Ordering::Relaxed),
            st.handshakes_refused.load(Ordering::Relaxed),
            st.connects_ok.load(Ordering::Relaxed),
            st.connects_denied.load(Ordering::Relaxed),
            st.connects_failed.load(Ordering::Relaxed),
            st.bytes_to_targets.load(Ordering::Relaxed),
            st.bytes_from_targets.load(Ordering::Relaxed),
        );
    }
}
