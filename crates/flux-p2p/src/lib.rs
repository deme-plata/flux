// flux-p2p — libp2p-rust with DAGKnight native support
//
// Architecture:
//   NetworkManager → tokio task (FluxSwarmManager) → libp2p Swarm
//   DAGKnight      → Round-based DAG consensus, implicit voting, VDF leader
//   SAP            → Score-Adjusted Priority — vertex & peer scoring
//   X-Algo         → Cross-algorithm multi-dimensional scoring
//   Entanglement   → QtFT linking-number + Bloom routing
//
// Integrates with Quillon Graph's existing libp2p mesh.
// Topics: /qnk/mainnet-genesis/blocks, /flux/1/compile-*, /dagknight/1/*

pub mod dagknight;
pub mod sap;
pub mod x_algo;
pub mod entanglement;
pub mod swarm;
pub mod cortex_optimizer;
/// Content-addressed block backfill protocol (flux-sync over the gossip mesh).
/// The reusable version of what sigil-top wired inline — any flux-p2p consumer
/// gets verify-don't-trust history backfill, not just live gossip.
pub mod backfill;

use std::sync::Arc;
use parking_lot::RwLock;
use tokio::sync::mpsc;
use libp2p::futures::StreamExt;

use swarm::{FluxSwarmManager, FluxSwarmConfig, TransportMode, PeerInfo};

/// Re-export of the event types pushed to the application by the swarm loop.
/// Consume via [`NetworkManager::drain_events`].
pub use swarm::SwarmAppEvent;

/// Re-export SIGIL block topic helpers for sigil-top integration.
pub use swarm::{sigil_topic, SIGIL_G0_BLOCKS_TOPIC, COMBO_AFFINITY_TOPIC};

/// Default bootstrap peers — always configured for out-of-the-box P2P.
/// Delta (5.79.79.158) and Epsilon (89.149.241.126) are the core mesh.
pub const DEFAULT_BOOTSTRAP_PEERS: &[(&str, &str)] = &[
    ("delta",  "/ip4/5.79.79.158/tcp/9003"),
    ("epsilon","/ip4/89.149.241.126/tcp/9003"),
];

/// SIGIL g0 testnet bootstrap peers — port 9501 is the sigil-node P2P port.
/// Use these when connecting to the SIGIL block mesh for sync.
///
/// ⚠️ Each multiaddr MUST carry the node's `/p2p/<PeerId>` suffix. A bare
/// `/ip4/.../tcp/9501` opens a transport connection but libp2p/Kademlia can't
/// register the peer without its PeerId, so `peer_count` stays 0, the
/// peer-gated backfill never starts, and sync sits at ~0 blk/s (root-caused
/// 2026-06-09). Epsilon's id pulled live from `journalctl -u sigil-node`.
pub const SIGIL_BOOTSTRAP_PEERS: &[(&str, &str)] = &[
    // All four ids confirmed LIVE from the [p2p-sync] connection log (libp2p verifies
    // the PeerId during the Noise handshake, so a connected id is authoritative — more
    // reliable than `journalctl`, which can hold a stale id from an old node restart).
    ("epsilon-sigil", "/ip4/89.149.241.126/tcp/9501/p2p/12D3KooWQ1E42MDH2BVcC1qp5bo6oydPptA6VBbgXcAt3gaTMSof"),
    ("delta-sigil",   "/ip4/5.79.79.158/tcp/9501/p2p/12D3KooWPTkN7eVEfjWBkojcecTEsGr1udU2VgjxxbDSiQpDja9b"),
    ("gamma-sigil",   "/ip4/109.205.176.60/tcp/9501/p2p/12D3KooWGr61P7jks3NjPMQkRa8Qmc2TNVyWYXm24BxZLtK675gd"),
    ("beta-sigil",    "/ip4/185.182.185.227/tcp/9501/p2p/12D3KooWSJxrXTttxp6WVPTWxAZJG1JQ46zSRF1C6gY6LLtyVMuA"),
];

/// Firewall-friendly relay bootstrap for networks where tcp/9501 is reachable
/// from the fleet but blocked or slow from an operator desktop. This is a
/// socat-fronted Epsilon path that speaks to the same SIGIL node identity.
pub const SIGIL_RELAY853_BOOTSTRAP_PEERS: &[(&str, &str)] = &[
    ("epsilon-relay853", "/ip4/89.149.241.126/tcp/853/p2p/12D3KooWQ1E42MDH2BVcC1qp5bo6oydPptA6VBbgXcAt3gaTMSof"),
];

/// v0.57 (sync): well-known path where a sigil-node PRODUCER publishes its libp2p peer-id, so a
/// CO-LOCATED sigil-top monitor can auto-dial it over loopback. `SIGIL_PEERID_FILE` overrides;
/// the default is a FIXED path (NOT `temp_dir()`, which honors `TMPDIR` and would differ between
/// the producer's systemd env and an operator shell that sets `TMPDIR`) so both sides agree.
pub fn sigil_peerid_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("SIGIL_PEERID_FILE") {
        if !p.trim().is_empty() {
            return std::path::PathBuf::from(p);
        }
    }
    #[cfg(windows)]
    {
        std::env::temp_dir().join("sigil-node-g0.peerid")
    }
    #[cfg(not(windows))]
    {
        std::path::PathBuf::from("/tmp/sigil-node-g0.peerid")
    }
}

/// Producer side: publish our peer-id to [`sigil_peerid_path`] so a co-located monitor can dial
/// us first. Best-effort; the caller logs any error and never treats it as fatal.
pub fn publish_sigil_peerid(peer_id: &str) -> std::io::Result<()> {
    std::fs::write(sigil_peerid_path(), peer_id.trim())
}

/// Monitor side: if a sigil-node producer is running on THIS box — it published its peer-id AND
/// `127.0.0.1:9501` is accepting — return its loopback multiaddr so [`NetworkManager::for_sigil`]
/// dials it FIRST. Loopback specifically: the public-IP bootstrap entry (`89.149.241.126:9501`)
/// is unreachable same-box on Epsilon (hairpin/firewall on `:9501`), which is WHY a monitor next
/// to a tip-complete producer still crawled the remote fleet. `None` when absent/stale (standalone
/// monitor, or Windows) → normal fleet bootstrap, costing only one short connect probe.
fn detect_local_producer() -> Option<String> {
    let pid = std::fs::read_to_string(sigil_peerid_path()).ok()?;
    let pid = pid.trim();
    if !pid.starts_with("12D3Koo") || pid.len() < 46 {
        return None; // not a plausible libp2p PeerId — ignore a garbage/partial file
    }
    let addr: std::net::SocketAddr = "127.0.0.1:9501".parse().ok()?;
    // Confirm the producer is LIVE before adding it, so a stale file never becomes a dead
    // bootstrap entry. One-time 300ms probe at monitor startup.
    std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(300)).ok()?;
    Some(format!("/ip4/127.0.0.1/tcp/9501/p2p/{pid}"))
}

/// SIGIL gossipsub topics — matches sigil-net crate constants.
pub const SIGIL_TOPICS: &[&str] = &[
    "/sigil/g0/blocks",
    "/sigil/g0/peer-heights",
    "/sigil/g0/tip-proofs",
    "/sigil/g0/txs",
    "/sigil/g0/release",
];

/// Extract the `/ip4/<ip>/tcp/<port>` (or ip6) endpoint prefix from a multiaddr string,
/// dropping any `/p2p/<id>` suffix. Used for ENDPOINT-based bootstrap dedup that's robust
/// to peer-id rotation: a node that doesn't persist its libp2p identity changes its id on
/// every restart, so id-based matching breaks — but the ip:port endpoint is stable.
fn ip_tcp_endpoint(ma: &str) -> Option<String> {
    let p: Vec<&str> = ma.split('/').collect();
    // ["", "ip4"|"ip6", "<ip>", "tcp", "<port>", ...]
    if p.len() >= 5 && (p[1] == "ip4" || p[1] == "ip6") && p[3] == "tcp" {
        Some(format!("/{}/{}/tcp/{}", p[1], p[2], p[4]))
    } else {
        None
    }
}

/// Split comma/semicolon/whitespace-separated bootstrap multiaddrs.
fn split_bootstrap_env(value: &str) -> Vec<String> {
    value
        .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn bootstrap_env(name: &str) -> Option<Vec<String>> {
    let value = std::env::var(name).ok()?;
    let peers = split_bootstrap_env(&value);
    if peers.is_empty() {
        tracing::warn!(%name, "bootstrap env var was set but empty after parsing; ignoring");
        None
    } else {
        Some(peers)
    }
}

fn dedupe_bootstrap_peers(peers: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for peer in peers {
        if seen.insert(peer.clone()) {
            out.push(peer);
        }
    }
    out
}

fn sigil_bootstrap_peers() -> Vec<String> {
    // Full override: when set, this is the entire bootstrap set. This is the
    // escape hatch for private fleets, Windows desktops behind restrictive
    // firewalls, or emergency peer-id rotation before a release can be cut.
    if let Some(peers) = bootstrap_env("SIGIL_BOOTSTRAP_PEERS") {
        tracing::info!(count = peers.len(), "using SIGIL_BOOTSTRAP_PEERS full override");
        return dedupe_bootstrap_peers(peers);
    }

    let mut bootstrap = Vec::new();

    // A co-located producer is the fastest and most reliable peer; only add it
    // when the full override above is absent, so SIGIL_BOOTSTRAP_PEERS remains
    // exact.
    if let Some(local) = detect_local_producer() {
        tracing::info!(target: "sigil::identity", addr = %local, "co-located producer detected — dialing it FIRST");
        bootstrap.push(local);
    }

    // Back-compat: SIGIL_BOOTSTRAP is additive. Prefer SIGIL_BOOTSTRAP_PEERS for
    // exact replacement semantics.
    if let Some(extra) = bootstrap_env("SIGIL_BOOTSTRAP") {
        tracing::info!(count = extra.len(), "using legacy SIGIL_BOOTSTRAP additive peers");
        bootstrap.extend(extra);
    }

    bootstrap.extend(SIGIL_BOOTSTRAP_PEERS.iter().map(|(_, addr)| addr.to_string()));
    bootstrap.extend(SIGIL_RELAY853_BOOTSTRAP_PEERS.iter().map(|(_, addr)| addr.to_string()));
    dedupe_bootstrap_peers(bootstrap)
}

/// Global P2P network manager — thread-safe, cloneable.
/// Wraps a tokio task running the real libp2p FluxSwarmManager.
pub struct NetworkManager {
    inner: Arc<RwLock<NetworkInner>>,
    config: NetworkConfig,
    /// Channel to send commands to the swarm event loop.
    cmd_tx: Option<mpsc::UnboundedSender<SwarmCommand>>,
    /// Application-level events surfaced by the swarm loop —
    /// gossipsub messages, peer connect/disconnect, listen-address updates.
    /// Drain via [`NetworkManager::drain_events`].
    app_events: Arc<RwLock<Vec<SwarmAppEvent>>>,
    /// Notifier signalled when new events are pushed to app_events.
    /// Subscribers use this to wake instead of polling.
    event_notify: Arc<tokio::sync::Notify>,
    /// Set of currently-connected libp2p PeerIds, shared with the swarm task.
    /// Populated on start(); empty until then. Read by `connected_peer_ids()`.
    connected: Arc<RwLock<std::collections::HashSet<libp2p::PeerId>>>,
}

/// Commands sent to the swarm event loop.
enum SwarmCommand {
    Publish { topic: String, data: Vec<u8> },
    /// Point-to-point backfill: send a request to `peer`, fulfil `resp` with the
    /// response bytes (or an error string).
    SendRequest {
        peer: libp2p::PeerId,
        payload: Vec<u8>,
        resp: tokio::sync::oneshot::Sender<Result<Vec<u8>, String>>,
    },
    /// Point-to-point backfill: answer an inbound request by its opaque u64 id.
    Respond { request_id: u64, payload: Vec<u8> },
    Shutdown,
}

struct NetworkInner {
    started: bool,
    peer_count: u32,
    dagknight_round: u64,
    sap_scores: sap::ScoreTable,
    x_algo_scores: x_algo::CrossScoreTable,
    /// Cached peer info updated by the swarm event loop.
    peers: Vec<PeerInfo>,
    /// v0.8: Mesh health metrics for SIGIL fleet dashboard
    mesh_health: MeshHealth,
    /// v0.8: Cortex-driven autonomous P2P optimizer
    cortex_opt: cortex_optimizer::CortexP2POptimizer,
}

/// Configuration for the Flux P2P network node.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct NetworkConfig {
    pub node_id: String,
    pub listen_addr: String,
    pub bootstrap_peers: Vec<String>,
    pub dagknight_enabled: bool,
    pub sap_enabled: bool,
    pub x_algo_enabled: bool,
    pub entanglement_enabled: bool,
    pub gossipsub_topics: Vec<String>,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        NetworkConfig {
            node_id: "flux-node-0".into(),
            listen_addr: "/ip4/0.0.0.0/tcp/9003".into(),
            bootstrap_peers: DEFAULT_BOOTSTRAP_PEERS.iter()
                .map(|(_, addr)| addr.to_string())
                .collect(),
            dagknight_enabled: true,
            sap_enabled: true,
            x_algo_enabled: true,
            entanglement_enabled: true,
            gossipsub_topics: vec![
                "/qnk/mainnet-genesis/blocks".into(),
                "/flux/1/compile-request".into(),
                "/flux/1/compile-result".into(),
                "/flux/1/cache-invalidate".into(),
                "/dagknight/1/vertices".into(),
                COMBO_AFFINITY_TOPIC.into(),
            ],
        }
    }
}

impl NetworkManager {
    /// Create a new NetworkManager with the given configuration.
    pub fn new(config: NetworkConfig) -> Self {
        let node_id = config.node_id.clone();
        NetworkManager {
            inner: Arc::new(RwLock::new(NetworkInner {
                started: false,
                peer_count: 0,
                dagknight_round: 0,
                sap_scores: sap::ScoreTable::new(),
                x_algo_scores: x_algo::CrossScoreTable::new(),
                peers: Vec::new(),
                mesh_health: MeshHealth::new(),
                cortex_opt: cortex_optimizer::CortexP2POptimizer::new(&node_id),
            })),
            config,
            cmd_tx: None,
            app_events: Arc::new(RwLock::new(Vec::new())),
            event_notify: Arc::new(tokio::sync::Notify::new()),
            connected: Arc::new(RwLock::new(std::collections::HashSet::new())),
        }
    }

    /// Start the P2P network — launches the real libp2p FluxSwarmManager.
    ///
    /// Spawns a tokio task that runs the swarm event loop.
    /// The NetworkManager handle can be cloned and shared across threads.
    pub async fn start(&mut self) -> Result<(), String> {
        {
            let inner = self.inner.read();
            if inner.started {
                return Err("NetworkManager already started".into());
            }
        }

        tracing::info!(
            node_id = %self.config.node_id,
            listen = %self.config.listen_addr,
            bootstrap_count = self.config.bootstrap_peers.len(),
            "Flux P2P starting — real libp2p swarm"
        );

        // Build swarm config
        let listen_addr: libp2p::Multiaddr = self.config.listen_addr
            .parse()
            .map_err(|e| format!("Invalid listen addr '{}': {}", self.config.listen_addr, e))?;

        let bootstrap_addrs: Result<Vec<libp2p::Multiaddr>, _> = self.config.bootstrap_peers
            .iter()
            .map(|s| s.parse())
            .collect();

        let bootstrap_addrs = bootstrap_addrs
            .map_err(|e: libp2p::multiaddr::Error| format!("Invalid bootstrap addr: {}", e))?;

        let swarm_config = FluxSwarmConfig {
            node_id: self.config.node_id.clone(),
            listen_addr,
            bootstrap_peers: bootstrap_addrs,
            topics: self.config.gossipsub_topics.clone(),
            transport_mode: TransportMode::TcpOnly,
            batch_config: swarm::BatchConfig {
                max_batch_size: 64,
                flush_interval_ms: 5,
                enabled: true,
            },
            entanglement_config: entanglement::EntanglementConfig {
                prefer_knot_routing: true,
                ..Default::default()
            },
        };

        // Create the real libp2p swarm and capture its app-event buffer so
        // callers of `NetworkManager::drain_events` can read messages other
        // peers publish on the gossipsub topics we subscribed to.
        let (swarm_manager, event_rx) =
            FluxSwarmManager::new(swarm_config)?;
        self.app_events = event_rx;
        // Share the swarm's connected-peer set so connected_peer_ids() can read it.
        self.connected = swarm_manager.connected.clone();

        // Channel for application commands
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<SwarmCommand>();
        self.cmd_tx = Some(cmd_tx);

        // Shared state for the event loop to update
        let inner = Arc::clone(&self.inner);
        let node_id = self.config.node_id.clone();
        let event_notify = Arc::clone(&self.event_notify);

        // Spawn the swarm event loop on tokio
        tokio::spawn(async move {
            tracing::info!(%node_id, "Flux P2P event loop started");

            // Listen + bootstrap happens inside run()
            // But run() blocks forever. We need to interleave application commands.
            // Strategy: spawn run() on a separate task, poll events + commands here.
            let mut swarm = swarm_manager;

            // Start listening
            if let Err(e) = swarm.swarm.listen_on(swarm.listen_addr.clone()) {
                tracing::error!(%e, "Failed to listen");
                return;
            }

            // Bootstrap Kademlia
            let _ = swarm.swarm.behaviour_mut().kademlia.bootstrap();

            // Mark started — BOTH the outer NetworkManager's shared `inner`
            // state AND the FluxSwarmManager's own `started` flag. The
            // latter is the gate that `swarm.publish()` checks (swarm.rs:423);
            // forgetting it makes every publish() silently return
            // "Swarm not started", which was the actual root cause of the
            // rocky-83 / P3-demo drain_events bug. The cmd_rx loop below
            // calls `swarm.publish(...)` with `let _ = ...` (line 267
            // before this fix), so the error was getting swallowed.
            // tests/two_node_gossipsub.rs verifies the fix.
            {
                let mut inner = inner.write();
                inner.started = true;
            }
            swarm.started = true;

            // v0.35 (sync-earlier #2): EAGER one-shot dial of every bootstrap
            // endpoint at startup. kademlia.bootstrap() above only schedules a
            // DHT lookup; without this, the first PeerConnected waits either on
            // that lookup OR on the 5s mesh-maintenance timer's first tick — so
            // time-to-first-peer carried an avoidable 1-5s gap even when the
            // bootstrap ip:ports are up and reachable right now. Dial them
            // immediately using the SAME bare-endpoint path the retry arm uses
            // (strip /p2p/ → id-rotation-immune; Identify learns the real id
            // post-handshake). No-op once connected; the retry timer still
            // self-heals drops. This is the single cheapest first-peer win.
            for addr in &swarm.bootstrap_peers_cache.clone() {
                let target = ip_tcp_endpoint(&addr.to_string())
                    .unwrap_or_else(|| addr.to_string());
                match target.parse::<libp2p::Multiaddr>() {
                    Ok(ma) => if let Err(e) = swarm.swarm.dial(ma) {
                        tracing::debug!(%target, %e, "eager bootstrap dial failed (retry timer will catch it)");
                    },
                    Err(e) => tracing::debug!(%target, %e, "bad bootstrap endpoint (eager dial)"),
                }
            }

            // Batch flush timer
            let mut flush_interval = tokio::time::interval(
                std::time::Duration::from_millis(swarm.batch_config.flush_interval_ms)
            );

            // v0.28: bootstrap MESH-MAINTENANCE timer — steady 5s; re-dials MISSING peers
            // (not only when the pool is empty). See the retry arm below.
            let mut retry_interval = tokio::time::interval(
                std::time::Duration::from_secs(1)
            );
            let mut bootstrap_retry: std::collections::HashMap<String, (u8, std::time::Instant)> =
                std::collections::HashMap::new();

            loop {
                tokio::select! {
                    // Batch flush tick
                    _ = flush_interval.tick() => {
                        swarm.flush_all();
                    }

                    // Connection retry — per-endpoint backoff.
                    _ = retry_interval.tick() => {
                        // v0.28 MESH-MAINTENANCE FIX (connections issue): re-dial any bootstrap
                        // peer that ISN'T currently connected — every tick, not only when
                        // peer_count==0. Before this, a dropped bootstrap peer (e.g. 4→3) was
                        // never re-connected while others remained, so the pool silently thinned
                        // toward a single point of failure (the "connections issue"). Connected
                        // peers are skipped via their /p2p/<id>, so it's a no-op at full mesh and
                        // a steady self-heal otherwise. (No backoff: 4 cheap dials/5s is fine, and
                        // a thin pool must recover promptly, not exponentially slowly.)
                        if !swarm.bootstrap_peers_cache.is_empty() {
                            // v0.30 (connections fixed for good): dedup by ENDPOINT (ip4+tcp), NOT
                            // peer-id — bootstrap sigil-nodes that don't persist their libp2p identity
                            // ROTATE their id every restart, so a hardcoded /p2p/<id> goes stale and
                            // the dial is rejected on mismatch (the node capped at 3/4 despite all
                            // bootstraps being up). Build the set of connected /ip4/_/tcp/_ endpoints;
                            // for any bootstrap endpoint NOT connected, dial the BARE address (strip
                            // /p2p/) — we reach WHOEVER is listening there regardless of their current
                            // id, and Identify learns the real id post-handshake (Kademlia adopts it).
                            // Immune to id rotation; a no-op at full mesh.
                            let connected_eps: std::collections::HashSet<String> =
                                swarm.connected_peers().into_iter()
                                    .filter_map(|pi| ip_tcp_endpoint(&pi.multiaddr)).collect();
                            let pool_empty = connected_eps.is_empty();
                            let now = std::time::Instant::now();
                            for addr in &swarm.bootstrap_peers_cache.clone() {
                                let ep = ip_tcp_endpoint(&addr.to_string());
                                if let Some(ref e) = ep {
                                    if connected_eps.contains(e) {
                                        bootstrap_retry.remove(e);
                                        continue; // already connected here
                                    }
                                }
                                let target = ep.clone().unwrap_or_else(|| addr.to_string());
                                let retry = bootstrap_retry.entry(target.clone()).or_insert((0, now));
                                if now < retry.1 {
                                    continue;
                                }
                                // v0.31 FAST FIRST PEER: with ZERO connected peers the node is
                                // useless — sync gated, mining blind — so recover at 1/2/4s
                                // instead of 3/6/12s (measured: eager dial + mesh registration
                                // lands a reachable peer in 1-4s; a missed eager dial shouldn't
                                // triple that). With a non-empty pool the gentler 3/6/12s holds
                                // (a missing 4th peer is not urgent; don't hammer dead hosts).
                                let delay_secs = match (pool_empty, retry.0) {
                                    (true, 0) => 1,
                                    (true, 1) => 2,
                                    (true, _) => 4,
                                    (false, 0) => 3,
                                    (false, 1) => 6,
                                    (false, _) => 12,
                                };
                                retry.0 = retry.0.saturating_add(1);
                                retry.1 = now + std::time::Duration::from_secs(delay_secs);
                                match target.parse::<libp2p::Multiaddr>() {
                                    Ok(ma) => if let Err(e) = swarm.swarm.dial(ma) {
                                        if pool_empty {
                                            tracing::warn!(%target, %e, next_retry_secs = delay_secs, "bootstrap dial failed while peer pool is empty");
                                        } else {
                                            tracing::debug!(%target, %e, next_retry_secs = delay_secs, "bootstrap bare re-dial failed");
                                        }
                                    },
                                    Err(e) => tracing::debug!(%target, %e, "bad bootstrap endpoint"),
                                }
                            }
                            if pool_empty {
                                tracing::warn!(bootstrap_count = swarm.bootstrap_peers_cache.len(), "no connected peers — retrying bootstrap set with 3/6/12s backoff");
                            }
                        }
                    }

                    // Swarm event
                    event = swarm.swarm.select_next_some() => {
                        swarm.handle_swarm_event(event);

                        // Wake subscribers — new events may have been pushed
                        event_notify.notify_one();

                        // Update shared peer state
                        let peers: Vec<PeerInfo> = swarm.connected_peers()
                            .into_iter().cloned().collect();
                        let count = peers.len() as u32;
                        {
                            let mut inner = inner.write();
                            inner.peer_count = count;
                            inner.peers = peers;
                        }
                    }

                    // Application command
                    cmd = cmd_rx.recv() => {
                        match cmd {
                            Some(SwarmCommand::Publish { topic, data }) => {
                                let _ = swarm.publish(&topic, data);
                            }
                            Some(SwarmCommand::SendRequest { peer, payload, resp }) => {
                                swarm.send_request(peer, payload, resp);
                            }
                            Some(SwarmCommand::Respond { request_id, payload }) => {
                                swarm.respond(request_id, payload);
                            }
                            Some(SwarmCommand::Shutdown) => {
                                tracing::info!("Flux P2P event loop shutting down");
                                break;
                            }
                            None => break,
                        }
                    }
                }
            }
        });

        tracing::info!(
            node_id = %self.config.node_id,
            "Flux P2P launched — real libp2p swarm with Noise/Yamux/gossipsub/Kademlia"
        );

        Ok(())
    }

    /// Stop the P2P network.
    pub async fn stop(&self) -> Result<(), String> {
        if let Some(ref tx) = self.cmd_tx {
            let _ = tx.send(SwarmCommand::Shutdown);
        }
        let mut inner = self.inner.write();
        inner.started = false;
        tracing::info!("Flux P2P stopped");
        Ok(())
    }

    /// Publish a message to a gossipsub topic.
    /// Drain pending application events accumulated by the swarm loop since
    /// the last call. Returns an empty Vec if nothing arrived.
    ///
    /// The shared buffer is cleared after this call — events are never
    /// surfaced twice. Call this from a periodic tick in your application
    /// loop (sigil-node calls it once per heartbeat in `run_start`).
    pub fn drain_events(&self) -> Vec<SwarmAppEvent> {
        let mut guard = self.app_events.write();
        std::mem::take(&mut *guard)
    }

    /// Subscribe to a specific gossipsub topic. Returns a non-blocking receiver
    /// that yields `(topic, Vec<u8>)` tuples for each message on that topic.
    /// Event-driven — no polling delay. Call `try_recv()` in your event loop.
    ///
    /// Uses a broadcast fan-out: the swarm's event buffer is shared, and the
    /// subscription task wakes on a notifier instead of sleeping. Multiple
    /// subscribers can coexist; each gets its own copy of matching messages.
    pub fn subscribe(&self, topic_filter: &str) -> tokio::sync::mpsc::UnboundedReceiver<(String, Vec<u8>)> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let filter = topic_filter.to_string();
        let events = self.app_events.clone();
        let notify = self.event_notify.clone();
        tokio::spawn(async move {
            loop {
                notify.notified().await;
                let mut guard = events.write();
                let matching: Vec<(String, Vec<u8>)> = guard.iter().filter_map(|ev| {
                    match ev {
                        SwarmAppEvent::GossipsubMessage { topic, data, .. } if *topic == filter => {
                            Some((topic.clone(), data.clone()))
                        }
                        _ => None,
                    }
                }).collect();
                guard.retain(|ev| {
                    !matches!(ev, SwarmAppEvent::GossipsubMessage { topic, .. } if *topic == filter)
                });
                drop(guard);
                for msg in matching {
                    if tx.send(msg).is_err() {
                        return;
                    }
                }
            }
        });
        rx
    }

    /// Synchronous wrapper: block the calling thread until a message arrives
    /// on the given topic. For use in non-async contexts (e.g. sigil-top's
    /// TUI event loop). Returns `None` if the subscription was dropped.
    pub fn subscribe_blocking(&self, topic_filter: &str) -> std::sync::mpsc::Receiver<(String, Vec<u8>)> {
        let (tx, rx) = std::sync::mpsc::channel();
        let filter = topic_filter.to_string();
        let events = self.app_events.clone();
        let notify = self.event_notify.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_time().build().unwrap();
            rt.block_on(async move {
                loop {
                    notify.notified().await;
                    let mut guard = events.write();
                    let matching: Vec<(String, Vec<u8>)> = guard.iter().filter_map(|ev| {
                        match ev {
                            SwarmAppEvent::GossipsubMessage { topic, data, .. } if *topic == filter => {
                                Some((topic.clone(), data.clone()))
                            }
                            _ => None,
                        }
                    }).collect();
                    guard.retain(|ev| {
                        !matches!(ev, SwarmAppEvent::GossipsubMessage { topic, .. } if *topic == filter)
                    });
                    drop(guard);
                    for msg in matching {
                        if tx.send(msg).is_err() {
                            return;
                        }
                    }
                }
            });
        });
        rx
    }

    pub fn publish(&self, topic: &str, data: Vec<u8>) -> Result<(), String> {
        match &self.cmd_tx {
            Some(tx) => {
                tx.send(SwarmCommand::Publish {
                    topic: topic.to_string(),
                    data,
                }).map_err(|e| format!("Publish channel closed: {}", e))
            }
            None => Err("NetworkManager not started".into()),
        }
    }

    /// Publish combo profile (from a recent supersonic combo run) for affinity.
    /// Nodes call this after `flux_combo_supersonic` to let the mesh know their speed.
    pub fn publish_combo_profile(&self, crate_name: &str, predicted_ms: f64, confidence: f64) -> Result<(), String> {
        self.publish(COMBO_AFFINITY_TOPIC, format!(r#"{{"crate":"{}","predicted_ms":{},"confidence":{}}}"#, crate_name, predicted_ms, confidence).into_bytes())
    }

    /// v0.7.0: Publish a compile request to the fleet. Build nodes subscribed
    /// to /flux/1/compile-request pick it up, compile the package, and publish
    /// results to /flux/1/compile-result with the same request_id.
    /// Returns the request_id so the caller can correlate results.
    pub fn request_compile(&self, package: &str, workspace: &str, release: bool) -> Result<String, String> {
        let request_id = format!("compile-{}-{}",
            package,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        );
        let payload = serde_json::json!({
            "request_id": request_id,
            "package": package,
            "workspace": workspace,
            "release": release,
            "requester": self.config.node_id,
            "ts_unix": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        });
        let data = serde_json::to_vec(&payload)
            .map_err(|e| format!("json encode: {e}"))?;
        self.publish("/flux/1/compile-request", data)?;
        Ok(request_id)
    }

    /// v0.7.0: Publish a compile result back to the fleet. Build nodes call
    /// this after completing a compile requested via /flux/1/compile-request.
    pub fn publish_compile_result(
        &self,
        request_id: &str,
        package: &str,
        success: bool,
        elapsed_ms: u64,
        binary_path: Option<&str>,
        blake3_hex: Option<&str>,
    ) -> Result<(), String> {
        let payload = serde_json::json!({
            "request_id": request_id,
            "package": package,
            "success": success,
            "elapsed_ms": elapsed_ms,
            "binary_path": binary_path,
            "blake3_hex": blake3_hex,
            "builder": self.config.node_id,
            "ts_unix": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        });
        let data = serde_json::to_vec(&payload)
            .map_err(|e| format!("json encode: {e}"))?;
        self.publish("/flux/1/compile-result", data)
    }

    /// v0.7.0: Subscribe to compile results matching a specific request_id.
    /// Returns a receiver that yields (request_id, package, success, elapsed_ms).
    pub fn subscribe_compile_results(
        &self,
        request_id_filter: &str,
    ) -> std::sync::mpsc::Receiver<(String, String, bool, u64)> {
        let (tx, rx) = std::sync::mpsc::channel();
        let filter = request_id_filter.to_string();
        let events = self.app_events.clone();
        let notify = self.event_notify.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_time().build().unwrap();
            rt.block_on(async move {
                loop {
                    notify.notified().await;
                    let mut guard = events.write();
                    let matching: Vec<(String, String, bool, u64)> = guard.iter().filter_map(|ev| {
                        match ev {
                            SwarmAppEvent::GossipsubMessage { topic, data, .. }
                                if topic == "/flux/1/compile-result" =>
                            {
                                if let Ok(payload) = serde_json::from_slice::<serde_json::Value>(data) {
                                    let rid = payload.get("request_id")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("");
                                    if rid == filter {
                                        let pkg = payload.get("package")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("unknown");
                                        let ok = payload.get("success")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(false);
                                        let ms = payload.get("elapsed_ms")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0);
                                        return Some((rid.to_string(), pkg.to_string(), ok, ms));
                                    }
                                }
                                None
                            }
                            _ => None,
                        }
                    }).collect();
                    guard.retain(|ev| {
                        !matches!(ev, SwarmAppEvent::GossipsubMessage { topic, .. } if topic == "/flux/1/compile-result")
                    });
                    drop(guard);
                    for msg in matching {
                        if tx.send(msg).is_err() {
                            return;
                        }
                    }
                }
            });
        });
        rx
    }

    /// v0.7.0: Push a block sync progress update to the application event
    /// channel. Sigil-top calls this from its block sync loop so the TUI
    /// can render a live sync gauge without polling.
    pub fn push_sync_progress(&self, height: u64, hash_hex: &str, peer_best_height: u64, total_synced: u64) {
        let peer_count = self.inner.read().peers.len();
        self.app_events.write().push(SwarmAppEvent::SyncProgress {
            height,
            hash_hex: hash_hex.to_string(),
            peer_best_height,
            total_synced,
            peer_count,
        });
    }

    /// Check if the network is running.
    pub fn is_running(&self) -> bool {
        self.inner.read().started
    }

    /// Get current peer count.
    pub fn peer_count(&self) -> u32 {
        self.inner.read().peer_count
    }

    /// Get the current DAGKnight round.
    pub fn dagknight_round(&self) -> u64 {
        self.inner.read().dagknight_round
    }

    /// Get the libp2p PeerIds we're currently connected to. The application
    /// picks one of these to send a backfill request to via `send_request`.
    pub fn connected_peers(&self) -> Vec<libp2p::PeerId> {
        self.connected.read().iter().copied().collect()
    }

    /// Get full connected-peer info (multiaddr, protocols, agent version).
    /// Updated by the swarm event loop from Identify + ConnectionEstablished.
    pub fn connected_peer_infos(&self) -> Vec<PeerInfo> {
        self.inner.read().peers.clone()
    }

    // ── Point-to-point backfill (request-response over /sigil/backfill/1) ──

    /// Send a backfill request to `peer` and await the response bytes.
    /// Returns Err on timeout, dial failure, disconnect, or if the network
    /// is not started. Payload is opaque (the application encodes block types).
    pub async fn send_request(&self, peer: libp2p::PeerId, payload: Vec<u8>) -> Result<Vec<u8>, String> {
        let tx = match &self.cmd_tx {
            Some(tx) => tx.clone(),
            None => return Err("NetworkManager not started".into()),
        };
        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
        tx.send(SwarmCommand::SendRequest { peer, payload, resp: resp_tx })
            .map_err(|e| format!("Request channel closed: {}", e))?;
        match resp_rx.await {
            Ok(result) => result,
            Err(_) => Err("Request dropped before a response arrived".into()),
        }
    }

    /// Answer an inbound backfill request surfaced via
    /// `SwarmAppEvent::InboundRequest`. Pass the same opaque `request_id`.
    /// Fire-and-forget; if the network is not started or the id is unknown,
    /// the request simply goes unanswered (the peer sees a timeout).
    pub fn respond(&self, request_id: u64, payload: Vec<u8>) {
        if let Some(ref tx) = self.cmd_tx {
            let _ = tx.send(SwarmCommand::Respond { request_id, payload });
        }
    }

    /// Get SAP peer score for a given peer ID.
    pub fn sap_score(&self, peer_id: &str) -> Option<f64> {
        let inner = self.inner.read();
        inner.sap_scores.get(&sap::PeerId::from(peer_id))
    }

    /// Get X-Algo composite score for a given peer ID.
    pub fn x_algo_score(&self, peer_id: &str) -> Option<x_algo::CrossScore> {
        let inner = self.inner.read();
        inner.x_algo_scores.get(&x_algo::PeerId::from(peer_id))
    }

    /// Update DAGKnight round (called when new blocks are committed).
    pub fn advance_round(&self, round: u64) {
        self.inner.write().dagknight_round = round;
    }

    /// Create a NetworkManager pre-configured for the Flux compile mesh.
    /// Uses port 9003, compile-request/compile-result topics, and the
    /// Delta+Epsilon bootstrap. For distributed compilation across the fleet.
    pub fn for_fleet_compile(node_name: &str) -> Self {
        let mut config = NetworkConfig::default();
        config.node_id = format!("flux-compile-{node_name}-{}", std::process::id());
        config.listen_addr = "/ip4/0.0.0.0/tcp/0".into(); // ephemeral
        config.gossipsub_topics = vec![
            "/flux/1/compile-request".into(),
            "/flux/1/compile-result".into(),
            "/flux/1/cache-invalidate".into(),
            "/flux/1/rev-snapshot".into(),
        ];
        config.bootstrap_peers = DEFAULT_BOOTSTRAP_PEERS.iter()
            .map(|(_, addr)| addr.to_string())
            .collect();
        config.dagknight_enabled = false;
        config.sap_enabled = false;
        config.x_algo_enabled = false;
        config.entanglement_enabled = false;
        Self::new(config)
    }

    /// Create a NetworkManager pre-configured for the SIGIL block mesh.
    /// Uses port 9501, SIGIL topics, and the 4-node testnet bootstrap.
    pub fn for_sigil(node_name: &str) -> Self {
        let mut config = NetworkConfig::default();
        // v0.31: STABLE per-install identity. node_id used to embed `{pid}`, and the libp2p
        // keypair is derived from node_id (`keypair_from_seed`) — so the peer-id CHANGED on
        // EVERY restart. Peers cache the dead id, the fresh id looks like a brand-new node each
        // launch, and the mesh kept capping at 3/4 peers (rotation never lets the pool fill).
        // Persist a random per-install token once and reuse it → peer-id stable across restarts
        // AND unique per install. (`SIGIL_TOP_IDENTITY` env pins it explicitly.)
        let (id_token, minted, id_path) = sigil_install_id();
        config.node_id = format!("sigil-top-{node_name}-{id_token}");
        let peer_id = crate::swarm::peer_id_string(&config.node_id);
        // ── DETAILED IDENTITY DEBUG (v0.31) ──────────────────────────────────────────────────
        tracing::info!(target: "sigil::identity", node_id = %config.node_id, peer_id = %peer_id,
            source = if minted { "minted-first-launch" } else { "persisted" },
            path = %id_path.display(), "SIGIL identity resolved (stable across restarts)");
        // v0.32.5: NO eprintln here — the sigil-top TUI owns the terminal, so raw stderr writes
        // corrupt the ratatui display ("ui sucks in terminal linux"). The identity is logged via
        // `tracing` above — RUST_LOG=sigil::identity=info to see it headless — never the raw screen.
        config.listen_addr = "/ip4/0.0.0.0/tcp/0".into(); // ephemeral
        config.gossipsub_topics = SIGIL_TOPICS.iter().map(|s| s.to_string()).collect();
        // SIGIL_BOOTSTRAP_PEERS is a FULL env override; legacy SIGIL_BOOTSTRAP
        // is additive. Built-ins include the 4 fleet nodes plus the Epsilon
        // relay853 fallback for restrictive networks.
        config.bootstrap_peers = sigil_bootstrap_peers();
        // ── DETAILED BOOTSTRAP DEBUG (v0.31): every bootstrap addr + whether Kademlia can seed ──
        // from it. A bare /ip4/IP/tcp/PORT (no /p2p/<PeerId>) registers under OUR id → DHT never
        // seeds → "0 peers". v0.32.5: logged via `tracing` ONLY (no eprintln — it corrupts the TUI).
        for (i, b) in config.bootstrap_peers.iter().enumerate() {
            let has_pid = b.contains("/p2p/");
            tracing::debug!(target: "sigil::identity", idx = i, addr = %b, has_peer_id = has_pid, "bootstrap peer");
        }
        // Disable DAGKnight/SAP/X-Algo for light clients — only need gossipsub
        config.dagknight_enabled = false;
        config.sap_enabled = false;
        config.x_algo_enabled = false;
        config.entanglement_enabled = false;
        Self::new(config)
    }

    /// Get a summary of the network state (for MCP tools / stats).
    pub fn summary(&self) -> NetworkSummary {
        let inner = self.inner.read();
        let mut health = inner.mesh_health.clone();
        health.connected_peers = inner.peer_count;
        health.update_quality();
        NetworkSummary {
            node_id: self.config.node_id.clone(),
            started: inner.started,
            peer_count: inner.peer_count,
            dagknight_round: inner.dagknight_round,
            sap_entries: inner.sap_scores.len(),
            x_algo_entries: inner.x_algo_scores.len(),
            listen_addr: self.config.listen_addr.clone(),
            topics: self.config.gossipsub_topics.clone(),
            bootstrap_peers: self.config.bootstrap_peers.clone(),
            mesh_health: Some(health),
        }
    }

    /// v0.8: Get live mesh health metrics for SIGIL fleet dashboard.
    pub fn mesh_health(&self) -> MeshHealth {
        let inner = self.inner.read();
        let mut h = inner.mesh_health.clone();
        h.connected_peers = inner.peer_count;
        h.update_quality();
        h
    }

    /// v0.8: Record a block propagation latency sample.
    pub fn record_block_latency(&self, latency_ms: f64, peer_height: Option<u64>) {
        let mut inner = self.inner.write();
        inner.mesh_health.record_latency(latency_ms);
        inner.mesh_health.messages_processed += 1;
        if let Some(h) = peer_height {
            inner.mesh_health.peer_heights.insert("last".into(), h);
        }
    }

    // ── v0.8: Cortex-driven autonomous P2P optimization ──

    /// Feed observed P2P metrics into the Cortex optimizer.
    /// Call periodically (every few seconds) with current SAP scores and mesh state.
    pub fn observe_cortex(&self, metrics: &cortex_optimizer::P2PMetrics) {
        self.inner.write().cortex_opt.observe(metrics);
    }

    /// Run the Cortex optimization loop against current P2P state.
    /// Returns recommended SAP/X-Algo weight adjustments, batch config,
    /// predicted mesh health, and preferred compile nodes.
    ///
    /// The caller should apply the returned weights to SAP/X-Algo scorers
    /// and update the batch configuration for optimal throughput.
    pub fn optimize_cortex(
        &self,
        preset: flux_optimize::OptimizationPreset,
    ) -> cortex_optimizer::CortexP2PResult {
        // Collect current metrics
        let inner = self.inner.read();
        let peer_count = inner.peer_count;
        let mesh = inner.mesh_health.clone();
        drop(inner);

        // Build current metrics snapshot
        let mut sap_map: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        sap_map.insert("self".to_string(), 0.8);

        let metrics = cortex_optimizer::collect_metrics(
            peer_count,
            &sap_map,
            &mesh,
            &swarm::BatchConfig::default(),
            100.0,
            std::collections::HashMap::new(),  // combo_profiles fed from live combos
        );

        // Run the optimizer
        let mut inner = self.inner.write();
        inner.cortex_opt.observe(&metrics);
        inner.cortex_opt.optimize(&metrics, preset)
    }

    /// Get a summary of Cortex optimization activity for this node.
    pub fn cortex_summary(&self) -> cortex_optimizer::CortexP2PSummary {
        self.inner.read().cortex_opt.summary()
    }

    /// Apply optimized weights from a CortexP2PResult to the live SAP scorer.
    /// Call this after optimize_cortex() to enact the recommendations.
    pub fn apply_cortex_weights(&self, result: &cortex_optimizer::CortexP2PResult) {
        let mut inner = self.inner.write();
        // Apply SAP weights by rebuilding the score table with new weights
        let w = &result.sap_weights;
        inner.sap_scores = sap::ScoreTable::with_weights(sap::SAPWeights {
            contribution_weight: w.contribution,
            latency_weight: w.latency,
            stake_weight: w.stake,
            accuracy_weight: w.accuracy,
            uptime_weight: w.uptime,
        });
        // Update mesh health predictions
        inner.mesh_health.quality = if result.predicted_peer_count >= 8 {
            "healthy".into()
        } else if result.predicted_peer_count >= 2 {
            "warming".into()
        } else {
            "empty".into()
        };
    }
}

/// Lightweight summary for MCP responses.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct NetworkSummary {
    pub node_id: String,
    pub started: bool,
    pub peer_count: u32,
    pub dagknight_round: u64,
    pub sap_entries: usize,
    pub x_algo_entries: usize,
    pub listen_addr: String,
    pub topics: Vec<String>,
    pub bootstrap_peers: Vec<String>,
    /// v0.8: Mesh health metrics for SIGIL fleet dashboard
    pub mesh_health: Option<MeshHealth>,
}

/// v0.8: Aggregated mesh quality metrics — feeds sigil-top fleet panel.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct MeshHealth {
    /// How many peers we're connected to
    pub connected_peers: u32,
    /// Mesh quality: healthy / warming / empty
    pub quality: String,
    /// Estimated gossip message drop rate (0.0–1.0)
    pub estimated_drop_rate: f64,
    /// Average block propagation latency in milliseconds
    pub avg_block_latency_ms: f64,
    /// Blocks received via gossip in the last window
    pub blocks_received: u64,
    /// Total gossip messages processed
    pub messages_processed: u64,
    /// Fan-out derived from sqrt(peers)
    pub fan_out: u32,
    /// v0.8: Known peer heights — keyed by peer_id
    pub peer_heights: std::collections::HashMap<String, u64>,
    /// v0.8: Block propagation latency samples (last N blocks)
    pub recent_latencies_ms: Vec<f64>,
}

impl MeshHealth {
    pub fn new() -> Self {
        Self { quality: "warming".into(), ..Default::default() }
    }
    /// Update quality string based on peer count
    pub fn update_quality(&mut self) {
        self.fan_out = (self.connected_peers as f64).sqrt().round() as u32;
        self.quality = if self.connected_peers >= 8 { "healthy".into() }
            else if self.connected_peers >= 2 { "warming".into() }
            else { "empty".into() };
    }
    /// Record a block propagation latency sample
    pub fn record_latency(&mut self, latency_ms: f64) {
        self.recent_latencies_ms.push(latency_ms);
        if self.recent_latencies_ms.len() > 20 {
            self.recent_latencies_ms.remove(0);
        }
        self.avg_block_latency_ms = self.recent_latencies_ms.iter().sum::<f64>()
            / self.recent_latencies_ms.len() as f64;
        self.blocks_received += 1;
    }
}

/// v0.31: load (or first-launch create-and-persist) a STABLE per-install identity token so the
/// derived libp2p peer-id stops rotating on every restart (the "caps at 3/4 peers" mesh-churn
/// root cause). Resolution order: `SIGIL_TOP_IDENTITY` env override → `$SIGIL_TOP_HOME/identity`
/// → `~/.sigil-top/identity` → temp dir. Returns (token, minted_this_launch, path).
fn sigil_install_id() -> (String, bool, std::path::PathBuf) {
    use std::path::PathBuf;
    let dir: PathBuf = std::env::var("SIGIL_TOP_HOME").ok().map(PathBuf::from)
        .or_else(|| std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".sigil-top")))
        .unwrap_or_else(std::env::temp_dir);
    let path = dir.join("identity");
    // explicit env override wins (operator pins an identity), but still report the path.
    if let Ok(v) = std::env::var("SIGIL_TOP_IDENTITY") {
        let v = v.trim().to_string();
        if !v.is_empty() { return (v, false, path); }
    }
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let t = existing.trim().to_string();
        if !t.is_empty() { return (t, false, path); }
    }
    // First launch: mint a random 16-hex token from OS entropy (read_exact a FIXED 32 bytes —
    // NEVER fs::read the infinite /dev/urandom), with pid+nanos as a portable fallback. Persist.
    let mut rnd = [0u8; 32];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read; let _ = f.read_exact(&mut rnd);
    }
    let mut h = blake3::Hasher::new();
    h.update(&rnd);
    h.update(&std::process::id().to_le_bytes());
    if let Ok(d) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        h.update(&d.as_nanos().to_le_bytes());
    }
    let token = h.finalize().to_hex()[..16].to_string();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(&path, &token);
    (token, true, path)
}

// ── P2P multiaddr parsing helpers ──

/// Parse a multiaddr string like "/ip4/89.149.241.126/tcp/9003" into components.
pub fn parse_multiaddr(addr: &str) -> Option<(String, u16)> {
    let parts: Vec<&str> = addr.split('/').filter(|s| !s.is_empty()).collect();
    let mut ip = String::new();
    let mut port = 0u16;

    let mut i = 0;
    while i < parts.len() {
        match parts[i] {
            "ip4" | "ip6" | "dns4" | "dns6" => {
                if i + 1 < parts.len() {
                    ip = parts[i + 1].to_string();
                    i += 2;
                } else { break; }
            }
            "tcp" | "udp" => {
                if i + 1 < parts.len() {
                    port = parts[i + 1].parse().unwrap_or(0);
                    i += 2;
                } else { break; }
            }
            _ => { i += 1; }
        }
    }

    if ip.is_empty() { None } else { Some((ip, port)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_manager_create() {
        let config = NetworkConfig {
            node_id: "test-node".into(),
            ..Default::default()
        };
        let nm = NetworkManager::new(config);
        assert!(!nm.is_running());
        assert_eq!(nm.peer_count(), 0);
    }

    #[test]
    fn test_parse_multiaddr() {
        let (ip, port) = parse_multiaddr("/ip4/89.149.241.126/tcp/9003").unwrap();
        assert_eq!(ip, "89.149.241.126");
        assert_eq!(port, 9003);
    }

    #[test]
    fn test_summary() {
        let config = NetworkConfig {
            node_id: "epsilon".into(),
            ..Default::default()
        };
        let nm = NetworkManager::new(config);
        let summary = nm.summary();
        assert_eq!(summary.node_id, "epsilon");
        assert!(!summary.started);
    }

    #[test]
    fn test_default_bootstrap_has_peers() {
        let config = NetworkConfig::default();
        assert!(!config.bootstrap_peers.is_empty(), "Default config must have bootstrap peers");
        // Delta should be in the list
        let has_delta = config.bootstrap_peers.iter()
            .any(|a| a.contains("5.79.79.158"));
        assert!(has_delta, "Default bootstrap must include Delta (5.79.79.158)");
    }

    #[test]
    fn test_split_bootstrap_env_accepts_common_separators() {
        let peers = split_bootstrap_env(" /ip4/1.1.1.1/tcp/1/p2p/a, /ip4/2.2.2.2/tcp/2/p2p/b\n/ip4/3.3.3.3/tcp/3/p2p/c ");
        assert_eq!(peers.len(), 3);
        assert_eq!(peers[0], "/ip4/1.1.1.1/tcp/1/p2p/a");
        assert_eq!(peers[2], "/ip4/3.3.3.3/tcp/3/p2p/c");
    }

    #[test]
    fn test_dedupe_bootstrap_peers_keeps_first_occurrence() {
        let peers = dedupe_bootstrap_peers(vec![
            "/ip4/1.1.1.1/tcp/1/p2p/a".into(),
            "/ip4/2.2.2.2/tcp/2/p2p/b".into(),
            "/ip4/1.1.1.1/tcp/1/p2p/a".into(),
        ]);
        assert_eq!(peers, vec![
            "/ip4/1.1.1.1/tcp/1/p2p/a".to_string(),
            "/ip4/2.2.2.2/tcp/2/p2p/b".to_string(),
        ]);
    }

    #[test]
    fn test_sigil_relay853_fallback_is_configured() {
        assert!(SIGIL_RELAY853_BOOTSTRAP_PEERS
            .iter()
            .any(|(_, addr)| addr.contains("/tcp/853/") && addr.contains("/p2p/")));
    }
}
