//! flux-p2p-test — Direct P2P mesh test Epsilon ↔ Delta.
//! Uses raw libp2p Swarm events (not broken event channel).
//!   Epsilon: flux-p2p-test --peer /ip4/5.79.79.158/tcp/9003
//!   Delta:   flux-p2p-test --peer /ip4/89.149.241.126/tcp/9003

use flux_p2p::swarm::{FluxSwarmManager, FluxSwarmConfig, TransportMode};
use std::time::{Duration, Instant};
use libp2p::{Multiaddr, swarm::SwarmEvent};
use libp2p::futures::StreamExt;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let peer_addr: Multiaddr = args.iter()
        .position(|a| a == "--peer")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.parse().expect("invalid peer addr"))
        .unwrap_or_else(|| "/ip4/5.79.79.158/tcp/9003".parse().unwrap());

    let node_id = std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".into());

    println!("╔══════════════════════════════════════╗");
    println!("║  Flux P2P Mesh Test                  ║");
    println!("║  Node: {:<30}║", node_id);
    println!("║  Peer: {:<30}║", peer_addr);
    println!("╚══════════════════════════════════════╝");

    let config = FluxSwarmConfig {
        node_id: node_id.clone(),
        listen_addr: "/ip4/0.0.0.0/tcp/9003".parse().unwrap(),
        bootstrap_peers: vec![peer_addr.clone()],
        transport_mode: TransportMode::TcpOnly,
        ..Default::default()
    };

    let (mut manager, _event_rx) = match FluxSwarmManager::new(config) {
        Ok(t) => t,
        Err(e) => { eprintln!("Failed: {}", e); return; }
    };

    let local_id = manager.local_peer_id;
    println!("  Peer ID: {}", local_id);

    // Start listening
    if let Err(e) = manager.swarm.listen_on("/ip4/0.0.0.0/tcp/9003".parse().unwrap()) {
        eprintln!("Listen failed: {}", e);
        return;
    }

    // Dial the peer
    println!("  → Dialing {}...", peer_addr);
    match manager.swarm.dial(peer_addr.clone()) {
        Ok(_) => println!("  Dial sent ✓"),
        Err(e) => println!("  Dial error: {:?}", e),
    }

    println!("  Event loop running (30s)...\n");

    let start = Instant::now();
    let deadline = start + Duration::from_secs(30);
    let mut connected = 0u32;
    let mut had_peer = false;

    // Fast-reconnect schedule: aggressive exponential backoff, capped low.
    // First re-dial fires at ~0.4s (not 15s), so a dropped first SYN or a
    // cold DHT/identity recovers in well under a second instead of stalling
    // on the old `elapsed % 5 == 0` modulo gate. On any connection loss we
    // reset the backoff and re-dial immediately — this is what keeps the
    // link effectively continuous through NAT idle-reaping (the ~20s
    // Right(Closed) drop seen on Contabo boxes): reconnect lands sub-second.
    const REDIAL_MIN_MS: u64 = 400;
    const REDIAL_MAX_MS: u64 = 3000;
    let mut backoff_ms: u64 = REDIAL_MIN_MS;
    let mut next_redial = start + Duration::from_millis(REDIAL_MIN_MS);

    loop {
        let now = Instant::now();
        if now >= deadline { break; }

        // Re-dial whenever we have no live connection and the backoff elapsed.
        if connected == 0 && now >= next_redial {
            let dur = start.elapsed().as_secs_f64();
            println!("  ⏳ [{:.1}s] Re-dialing (backoff {}ms)...", dur, backoff_ms);
            let _ = manager.swarm.dial(peer_addr.clone());
            backoff_ms = (backoff_ms * 2).min(REDIAL_MAX_MS);
            next_redial = now + Duration::from_millis(backoff_ms);
        }

        // Short poll window keeps the re-dial check responsive (~0.5s granularity).
        match tokio::time::timeout(Duration::from_millis(500), manager.swarm.select_next_some()).await {
            Ok(SwarmEvent::ConnectionEstablished { peer_id, endpoint, num_established, .. }) => {
                let dir = match endpoint {
                    libp2p::core::ConnectedPoint::Dialer { .. } => "outbound",
                    libp2p::core::ConnectedPoint::Listener { .. } => "inbound",
                };
                connected += 1;
                had_peer = true;
                backoff_ms = REDIAL_MIN_MS; // reset for next time we drop
                let dur = start.elapsed().as_secs_f64();
                println!("  ✅ [{:.1}s] CONNECTED {} ({}) — {} total", dur, peer_id, dir, num_established);
            }
            Ok(SwarmEvent::ConnectionClosed { peer_id, num_established, cause, .. }) => {
                connected = connected.saturating_sub(1);
                let dur = start.elapsed().as_secs_f64();
                println!("  ❌ [{:.1}s] LOST {} — {} left ({:?})", dur, peer_id, num_established, cause);
                // Recover instantly: reset backoff and re-dial on the next loop tick.
                backoff_ms = REDIAL_MIN_MS;
                next_redial = Instant::now();
            }
            Ok(SwarmEvent::IncomingConnection { send_back_addr, .. }) => {
                println!("  ← Incoming from {}", send_back_addr);
            }
            Ok(SwarmEvent::Dialing { peer_id, .. }) => {
                if let Some(pid) = peer_id {
                    println!("  → Dialing {}...", pid);
                }
            }
            Ok(SwarmEvent::NewListenAddr { address, .. }) => {
                println!("  📡 Listening on {}", address);
            }
            Ok(SwarmEvent::Behaviour(event)) => {
                manager.handle_swarm_event(SwarmEvent::Behaviour(event));
            }
            Ok(_) => {} // ignore other events
            Err(_timeout) => {} // poll-window expiry; re-dial is handled at loop top
        }
    }

    let dur = start.elapsed();
    println!("\n╔══════════════════════════════════════╗");
    println!("║  TEST COMPLETE                       ║");
    println!("╠══════════════════════════════════════╣");
    println!("║  Node:     {:<25}║", node_id);
    println!("║  Peer ID:  {:<25}║", local_id);
    println!("║  Duration: {:.1}s                     ║", dur.as_secs_f64());
    println!("║  Connected:{}                         ║", if had_peer { " YES ✓" } else { " NO ✗" });
    println!("╚══════════════════════════════════════╝");
}
