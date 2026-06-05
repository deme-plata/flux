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

use std::sync::Arc;
use parking_lot::RwLock;
use tokio::sync::mpsc;
use libp2p::futures::StreamExt;

use swarm::{FluxSwarmManager, FluxSwarmConfig, TransportMode, PeerInfo};

/// Re-export of the event types pushed to the application by the swarm loop.
/// Consume via [`NetworkManager::drain_events`].
pub use swarm::SwarmAppEvent;

/// Default bootstrap peers — always configured for out-of-the-box P2P.
/// Delta (5.79.79.158) and Epsilon (89.149.241.126) are the core mesh.
pub const DEFAULT_BOOTSTRAP_PEERS: &[(&str, &str)] = &[
    ("delta",  "/ip4/5.79.79.158/tcp/9003"),
    ("epsilon","/ip4/89.149.241.126/tcp/9003"),
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
        NetworkManager {
            inner: Arc::new(RwLock::new(NetworkInner {
                started: false,
                peer_count: 0,
                dagknight_round: 0,
                sap_scores: sap::ScoreTable::new(),
                x_algo_scores: x_algo::CrossScoreTable::new(),
                peers: Vec::new(),
            })),
            config,
            cmd_tx: None,
            app_events: Arc::new(RwLock::new(Vec::new())),
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

    /// Get a summary of the network state (for MCP tools / stats).
    pub fn summary(&self) -> NetworkSummary {
        let inner = self.inner.read();
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
        }
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
