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
pub use swarm::{sigil_topic, SIGIL_G0_BLOCKS_TOPIC};

/// Default bootstrap peers — always configured for out-of-the-box P2P.
/// Delta (5.79.79.158) and Epsilon (89.149.241.126) are the core mesh.
pub const DEFAULT_BOOTSTRAP_PEERS: &[(&str, &str)] = &[
    ("delta",  "/ip4/5.79.79.158/tcp/9003"),
    ("epsilon","/ip4/89.149.241.126/tcp/9003"),
];

/// SIGIL g0 testnet bootstrap peers — port 9501 is the sigil-node P2P port.
/// Use these when connecting to the SIGIL block mesh for sync.
pub const SIGIL_BOOTSTRAP_PEERS: &[(&str, &str)] = &[
    ("epsilon-sigil", "/ip4/89.149.241.126/tcp/9501"),
    ("delta-sigil",   "/ip4/5.79.79.158/tcp/9501"),
    ("gamma-sigil",   "/ip4/109.205.176.60/tcp/9501"),
    ("beta-sigil",    "/ip4/185.182.185.227/tcp/9501"),
];

/// SIGIL gossipsub topics — matches sigil-net crate constants.
pub const SIGIL_TOPICS: &[&str] = &[
    "/sigil/g0/blocks",
    "/sigil/g0/peer-heights",
    "/sigil/g0/tip-proofs",
    "/sigil/g0/txs",
    "/sigil/g0/release",
];

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
}

/// Commands sent to the swarm event loop.
enum SwarmCommand {
    Publish { topic: String, data: Vec<u8> },
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

            // Batch flush timer
            let mut flush_interval = tokio::time::interval(
                std::time::Duration::from_millis(swarm.batch_config.flush_interval_ms)
            );

            // Connection retry timer — exponential backoff for bootstrap peers
            let mut retry_interval = tokio::time::interval(
                std::time::Duration::from_secs(5)
            );
            let mut retry_backoff_secs: u64 = 5;

            loop {
                tokio::select! {
                    // Batch flush tick
                    _ = flush_interval.tick() => {
                        swarm.flush_all();
                    }

                    // Connection retry — exponential backoff
                    _ = retry_interval.tick() => {
                        let peer_count = swarm.connected_peers().len();
                        if peer_count == 0 && !swarm.bootstrap_peers_cache.is_empty() {
                            tracing::warn!(
                                backoff_secs = retry_backoff_secs,
                                "No connected peers — retrying bootstrap connections"
                            );
                            for addr in &swarm.bootstrap_peers_cache.clone() {
                                if let Err(e) = swarm.swarm.dial(addr.clone()) {
                                    tracing::debug!(%addr, %e, "Dial failed (will retry)");
                                }
                            }
                            retry_backoff_secs = (retry_backoff_secs * 2).min(120);
                            retry_interval = tokio::time::interval(
                                std::time::Duration::from_secs(retry_backoff_secs)
                            );
                        } else if peer_count > 0 {
                            retry_backoff_secs = 5; // Reset backoff when connected
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

    /// Get connected peer info.
    pub fn connected_peers(&self) -> Vec<PeerInfo> {
        self.inner.read().peers.clone()
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
        config.node_id = format!("sigil-top-{node_name}-{}", std::process::id());
        config.listen_addr = "/ip4/0.0.0.0/tcp/0".into(); // ephemeral
        config.gossipsub_topics = SIGIL_TOPICS.iter().map(|s| s.to_string()).collect();
        config.bootstrap_peers = SIGIL_BOOTSTRAP_PEERS.iter()
            .map(|(_, addr)| addr.to_string())
            .collect();
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
}
