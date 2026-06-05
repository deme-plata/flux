//! flux-bench-p2p — Real P2P throughput/latency benchmark over libp2p gossipsub.
//!
//!   Listener: flux-bench-p2p --listen --count 1000
//!   Sender:   flux-bench-p2p --send --count 1000 --size 65536 --peer /ip4/PEER_IP/tcp/9003

use flux_p2p::swarm::{FluxSwarmManager, FluxSwarmConfig, TransportMode, FluxBehaviourEvent};
use std::time::{Duration, Instant};
use libp2p::{Multiaddr, swarm::SwarmEvent, gossipsub};
use libp2p::futures::StreamExt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const BENCH_TOPIC: &str = "/flux/bench/1.0.0";

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let is_listener = args.iter().any(|a| a == "--listen");
    let is_sender = args.iter().any(|a| a == "--send");

    let msg_count: u64 = args.iter().position(|a| a == "--count")
        .and_then(|i| args.get(i + 1)).and_then(|s| s.parse().ok()).unwrap_or(100);
    let msg_size: usize = args.iter().position(|a| a == "--size")
        .and_then(|i| args.get(i + 1)).and_then(|s| s.parse().ok()).unwrap_or(65536);

    let peer_addr: Option<Multiaddr> = args.iter().position(|a| a == "--peer")
        .and_then(|i| args.get(i + 1)).map(|s| s.parse().expect("invalid peer addr"));

    let node_id = std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".into());
    let mode = if is_listener { "LISTENER" } else { "SENDER" };

    println!("╔══════════════════════════════════════════╗");
    println!("║  Flux P2P Benchmark — {:<19}║", mode);
    println!("║  Node: {:<34}║", node_id);
    println!("║  {} msgs × {} bytes                       ║", msg_count, msg_size);
    println!("╚══════════════════════════════════════════╝");

    let total_bytes = msg_count * msg_size as u64;

    let config = FluxSwarmConfig {
        node_id: node_id.clone(),
        listen_addr: "/ip4/0.0.0.0/tcp/9003".parse().unwrap(),
        bootstrap_peers: peer_addr.clone().into_iter().collect(),
        transport_mode: TransportMode::TcpOnly,
        ..Default::default()
    };

    let (mut manager, _) = match FluxSwarmManager::new(config) {
        Ok(t) => t,
        Err(e) => { eprintln!("Swarm failed: {}", e); return; }
    };

    println!("  Peer ID: {}", manager.local_peer_id);
    let _ = manager.swarm.listen_on("/ip4/0.0.0.0/tcp/9003".parse().unwrap());

    let topic = gossipsub::IdentTopic::new(BENCH_TOPIC);
    let _ = manager.swarm.behaviour_mut().gossipsub.subscribe(&topic);
    println!("  Subscribed to {}", BENCH_TOPIC);

    if let Some(ref addr) = peer_addr {
        println!("  → Dialing {}...", addr);
        let _ = manager.swarm.dial(addr.clone());
    }

    let rcv_count = Arc::new(AtomicU64::new(0));
    let rcv_bytes = Arc::new(AtomicU64::new(0));
    let first_ts = Arc::new(Mutex::new(None::<Instant>));
    let last_ts = Arc::new(Mutex::new(None::<Instant>));

    // === Wait for connection ===
    println!("  Waiting for connection...");
    let t0 = Instant::now();
    let deadline = t0 + Duration::from_secs(60);
    let mut connected = false;

    'conn: loop {
        if Instant::now() > deadline { break; }
        match tokio::time::timeout(Duration::from_secs(5), manager.swarm.select_next_some()).await {
            Ok(SwarmEvent::ConnectionEstablished { peer_id, .. }) => {
                println!("  ✅ [{:.1}s] Connected to {}", t0.elapsed().as_secs_f64(), peer_id);
                connected = true;
                break 'conn;
            }
            Ok(other) => { manager.handle_swarm_event(other); }
            Err(_) => {
                if let Some(ref addr) = peer_addr {
                    let _ = manager.swarm.dial(addr.clone());
                }
            }
        }
    }

    if !connected { println!("  ❌ No connection in 60s"); return; }

    // === SENDER MODE ===
    if is_sender {
        println!("  Sending {} × {} byte messages...", msg_count, msg_size);
        let payload = vec![0xABu8; msg_size];
        let send_t0 = Instant::now();

        for i in 0..msg_count {
            let _ = manager.swarm.behaviour_mut().gossipsub.publish(topic.clone(), payload.clone());
            if i > 0 && i % 100 == 0 {
                let mb = (i * msg_size as u64) as f64 / 1_048_576.0;
                let mbps = mb / send_t0.elapsed().as_secs_f64() * 8.0;
                println!("  ● {} msgs — {:.1} MB — {:.1} Mbps", i, mb, mbps);
            }
        }

        let send_elapsed = send_t0.elapsed();
        let send_mbps = (total_bytes as f64 * 8.0 / 1_000_000.0) / send_elapsed.as_secs_f64();
        let total_mb = total_bytes as f64 / 1_048_576.0;
        let dur_s = send_elapsed.as_secs_f64();
        println!("\n╔══════════════════════════════════════╗");
        println!("║  SENDER RESULTS                       ║");
        println!("╠══════════════════════════════════════╣");
        println!("║  Messages:  {:25}║", msg_count);
        println!("║  Total:     {:.1} MB               ║", total_mb);
        println!("║  Duration:  {:.2}s                  ║", dur_s);
        println!("║  Throughput:{:.1} Mbps             ║", send_mbps);
        println!("╚══════════════════════════════════════╝");

        println!("  Waiting 15s for receiver...");
        tokio::time::sleep(Duration::from_secs(15)).await;
        return;
    }

    // === LISTENER MODE ===
    println!("  Listening for messages...");
    let listen_t0 = Instant::now();

    loop {
        if listen_t0.elapsed() > Duration::from_secs(90) { println!("  ⏰ Timeout"); break; }

        match tokio::time::timeout(Duration::from_secs(3), manager.swarm.select_next_some()).await {
            Ok(event) => {
                // Inspect before consuming
                if let SwarmEvent::Behaviour(FluxBehaviourEvent::Gossipsub(gossipsub::Event::Message { message, .. })) = &event {
                    let n = rcv_count.fetch_add(1, Ordering::Relaxed) + 1;
                    rcv_bytes.fetch_add(message.data.len() as u64, Ordering::Relaxed);
                    if n == 1 { *first_ts.lock().unwrap() = Some(Instant::now()); }
                    *last_ts.lock().unwrap() = Some(Instant::now());
                    if n % 100 == 0 { println!("  ● [{:.1}s] {} msgs", listen_t0.elapsed().as_secs_f64(), n); }
                    if n >= msg_count { println!("  ✅ All {} msgs received!", n); break; }
                }
                manager.handle_swarm_event(event);
            }
            Err(_) => {}
        }
    }

    let total = rcv_count.load(Ordering::Relaxed);
    let bytes = rcv_bytes.load(Ordering::Relaxed);
    let f = first_ts.lock().unwrap().take();
    let l = last_ts.lock().unwrap().take();

    if let (Some(first), Some(last)) = (f, l) {
        let dur = last.duration_since(first);
        let mbps = if dur.as_secs_f64() > 0.0 {
            (bytes as f64 * 8.0 / 1_000_000.0) / dur.as_secs_f64()
        } else { 0.0 };

        let total_mb = bytes as f64 / 1_048_576.0;
        let msg_per_sec = total as f64 / dur.as_secs_f64();
        println!("\n╔══════════════════════════════════════╗");
        println!("║  RECEIVER RESULTS                     ║");
        println!("╠══════════════════════════════════════╣");
        println!("║  Messages:  {:25}║", total);
        println!("║  Total:     {:.1} MB               ║", total_mb);
        println!("║  Duration:  {:.3}s                 ║", dur.as_secs_f64());
        println!("║  Throughput:{:.1} Mbps             ║", mbps);
        println!("║  Msg/sec:   {:.0}                  ║", msg_per_sec);
        println!("╚══════════════════════════════════════╝");
    } else {
        println!("  ❌ No messages received");
    }
}
