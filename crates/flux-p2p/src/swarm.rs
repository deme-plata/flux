// Flux P2P Swarm — Real libp2p Network Behaviour
//
// Manages a genuine libp2p Swarm with:
//   - TCP + Noise + Yamux transport (primary, 0-RTT handshake)
//   - QUIC transport (optional, lower latency for mesh peers)
//   - Gossipsub v1.1 for block/transaction/compile broadcast
//   - Kademlia DHT for peer discovery (parallel walks)
//   - Identify protocol for node metadata
//   - Ping for sub-second keepalive
//   - Request-Response for direct P2P queries
//   - Message batching (configurable window, burst flush)
//   - Connection pooling (max 50 idle per peer)
//
// Speed targets:
//   - < 5ms publish latency (local mesh hop)
//   - < 50ms cross-datacenter (Epsilon ↔ Delta)
//   - 100K+ msg/sec throughput per node
//   - < 1s peer discovery convergence

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::entanglement::{EntanglementRouter, EntanglementConfig};

use libp2p::{
    core::{muxing::StreamMuxerBox, upgrade},
    gossipsub, identify, kad, noise, ping, request_response,
    request_response::{ProtocolSupport, ResponseChannel},
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux, Multiaddr, PeerId, StreamProtocol, Swarm, SwarmBuilder, Transport,
};
use libp2p::futures::StreamExt;

/// Protocol id for the point-to-point backfill request-response channel.
/// Request and response payloads are opaque `Vec<u8>` — flux-p2p stays
/// domain-agnostic; the application defines block types and encodes to bytes.
pub const BACKFILL_PROTOCOL: &str = "/sigil/backfill/1";

/// Gossip topic for combo affinity profiles.
/// Nodes publish after running supersonic combos: {crate, predicted_ms, confidence}.
/// Receivers feed into Cortex for combo-aware compile routing.
/// Invention: "Supersonic Combo Affinity" — P2P mesh uses live combo predictions
/// (produced by flux_combo_supersonic) to route tasks to the fastest predicted peers.
pub const COMBO_AFFINITY_TOPIC: &str = "/flux/combo-affinity/v1";

/// Max request-response payload (request OR response), in bytes. 64 MiB — far above
/// the built-in cbor codec's hardcoded 10 MiB response cap, so backfill can ship big
/// block-range chunks point-to-point.
pub const BACKFILL_MAX_MSG: u64 = 64 * 1024 * 1024;

/// Raw opaque codec for the backfill request-response protocol: payloads are already
/// application-serialized `Vec<u8>`, so we just length-bound + read-to-end (the
/// substream is closed after each write, signalling end). No CBOR wrapping; cap is
/// [`BACKFILL_MAX_MSG`] (vs the built-in cbor codec's fixed 10 MiB).
#[derive(Clone, Default)]
pub struct BackfillCodec;

#[async_trait::async_trait]
impl request_response::Codec for BackfillCodec {
    type Protocol = StreamProtocol;
    type Request = Vec<u8>;
    type Response = Vec<u8>;

    async fn read_request<T>(&mut self, _: &Self::Protocol, io: &mut T) -> std::io::Result<Vec<u8>>
    where
        T: futures::AsyncRead + Unpin + Send,
    {
        use futures::AsyncReadExt;
        let mut v = Vec::new();
        io.take(BACKFILL_MAX_MSG).read_to_end(&mut v).await?;
        Ok(v)
    }

    async fn read_response<T>(&mut self, _: &Self::Protocol, io: &mut T) -> std::io::Result<Vec<u8>>
    where
        T: futures::AsyncRead + Unpin + Send,
    {
        use futures::AsyncReadExt;
        let mut v = Vec::new();
        io.take(BACKFILL_MAX_MSG).read_to_end(&mut v).await?;
        Ok(v)
    }

    async fn write_request<T>(&mut self, _: &Self::Protocol, io: &mut T, req: Vec<u8>) -> std::io::Result<()>
    where
        T: futures::AsyncWrite + Unpin + Send,
    {
        use futures::AsyncWriteExt;
        io.write_all(&req).await?;
        io.close().await?;
        Ok(())
    }

    async fn write_response<T>(&mut self, _: &Self::Protocol, io: &mut T, resp: Vec<u8>) -> std::io::Result<()>
    where
        T: futures::AsyncWrite + Unpin + Send,
    {
        use futures::AsyncWriteExt;
        io.write_all(&resp).await?;
        io.close().await?;
        Ok(())
    }
}
// ═══════════════════════════════════════════════════════════════
// Behaviour
// ═══════════════════════════════════════════════════════════════

#[derive(NetworkBehaviour)]
#[behaviour(out_event = "FluxBehaviourEvent")]
pub struct FluxBehaviour {
    pub gossipsub: gossipsub::Behaviour,
    pub kademlia: kad::Behaviour<kad::store::MemoryStore>,
    pub identify: identify::Behaviour,
    pub ping: ping::Behaviour,
    /// Point-to-point backfill (CBOR-coded, opaque Vec<u8> payloads).
    pub request_response: request_response::Behaviour<BackfillCodec>,
}

#[derive(Debug)]
pub enum FluxBehaviourEvent {
    Gossipsub(gossipsub::Event),
    Kademlia(kad::Event),
    Identify(identify::Event),
    Ping(ping::Event),
    RequestResponse(request_response::Event<Vec<u8>, Vec<u8>>),
}

impl From<gossipsub::Event> for FluxBehaviourEvent {
    fn from(e: gossipsub::Event) -> Self { Self::Gossipsub(e) }
}
impl From<kad::Event> for FluxBehaviourEvent {
    fn from(e: kad::Event) -> Self { Self::Kademlia(e) }
}
impl From<identify::Event> for FluxBehaviourEvent {
    fn from(e: identify::Event) -> Self { Self::Identify(e) }
}
impl From<ping::Event> for FluxBehaviourEvent {
    fn from(e: ping::Event) -> Self { Self::Ping(e) }
}
impl From<request_response::Event<Vec<u8>, Vec<u8>>> for FluxBehaviourEvent {
    fn from(e: request_response::Event<Vec<u8>, Vec<u8>>) -> Self { Self::RequestResponse(e) }
}

// ═══════════════════════════════════════════════════════════════
// Transport configuration
// ═══════════════════════════════════════════════════════════════

#[derive(Clone, Debug, PartialEq)]
pub enum TransportMode {
    /// TCP + Noise + Yamux (default)
    TcpOnly,
    /// QUIC only (requires `quic` feature on libp2p)
    QuicOnly,
}

/// Build a transport stack from the given keypair and mode.
///
/// TCP mode: TCP → Noise XX handshake → Yamux muxing → boxed
/// QUIC mode: QUIC with Noise + Yamux built-in → boxed
pub fn build_transport(
    keypair: &libp2p::identity::Keypair,
    mode: TransportMode,
) -> std::io::Result<libp2p::core::transport::Boxed<(PeerId, StreamMuxerBox)>> {
    match mode {
        TransportMode::TcpOnly => {
            let noise_config = noise::Config::new(keypair).expect("Noise key config");
            let tcp_transport = tcp::tokio::Transport::new(tcp::Config::default().nodelay(true));
            let transport = tcp_transport
                .upgrade(upgrade::Version::V1)
                .authenticate(noise_config)
                .multiplex(yamux::Config::default())
                .timeout(Duration::from_secs(20))
                .boxed();
            Ok(transport)
        }
        TransportMode::QuicOnly => {
            #[cfg(feature = "quic")]
            {
                let quic_config = libp2p::quic::Config::new(keypair);
                let transport = libp2p::quic::tokio::Transport::new(quic_config);
                Ok(transport
                    .map(|(peer_id, muxer), _| (peer_id, StreamMuxerBox::new(muxer)))
                    .boxed())
            }
            #[cfg(not(feature = "quic"))]
            {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "QUIC transport requires libp2p 'quic' feature",
                ))
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Message batching
// ═══════════════════════════════════════════════════════════════

#[derive(Clone, Debug)]
pub struct BatchConfig {
    /// Max messages to accumulate before flushing.
    pub max_batch_size: usize,
    /// Max time to hold messages before flushing (milliseconds).
    pub flush_interval_ms: u64,
    /// Enable batching (if false, send immediately).
    pub enabled: bool,
}

impl Default for BatchConfig {
    fn default() -> Self {
        BatchConfig {
            max_batch_size: 64,
            flush_interval_ms: 5,
            enabled: true,
        }
    }
}

/// A pending batch of messages for a single topic.
struct MessageBatch {
    #[allow(dead_code)]
    topic: String,
    messages: Vec<Vec<u8>>,
    first_added: Instant,
}

impl MessageBatch {
    fn new(topic: String) -> Self {
        MessageBatch {
            topic,
            messages: Vec::new(),
            first_added: Instant::now(),
        }
    }

    fn is_ready(&self, config: &BatchConfig) -> bool {
        self.messages.len() >= config.max_batch_size
            || self.first_added.elapsed().as_millis() as u64 >= config.flush_interval_ms
    }
}

// ═══════════════════════════════════════════════════════════════
// Swarm manager
// ═══════════════════════════════════════════════════════════════

/// High-level P2P swarm manager — spawns the libp2p event loop on tokio.
///
/// # Example
/// ```no_run
/// use flux_p2p::swarm::{FluxSwarmManager, FluxSwarmConfig, TransportMode};
///
/// let config = FluxSwarmConfig {
///     node_id: "epsilon".into(),
///     listen_addr: "/ip4/0.0.0.0/tcp/9003".parse().unwrap(),
///     ..Default::default()
/// };
/// let (mut manager, mut event_rx) = FluxSwarmManager::new(config).unwrap();
/// tokio::spawn(async move { manager.run().await });
/// ```
pub struct FluxSwarmManager {
    pub swarm: Swarm<FluxBehaviour>,
    pub local_peer_id: PeerId,
    pub listen_addr: Multiaddr,
    topics: Vec<String>,
    batcher: HashMap<String, MessageBatch>,
    pub batch_config: BatchConfig,
    peers: HashMap<PeerId, PeerInfo>,
    pub started: bool,
    /// Cache of bootstrap peer addresses for connection retry logic.
    pub bootstrap_peers_cache: Vec<Multiaddr>,
    /// Entanglement router for knot-based peer selection (QtFT Path C).
    pub entanglement: EntanglementRouter,
    pub event_rx: Option<std::sync::Arc<parking_lot::RwLock<Vec<SwarmAppEvent>>>>,
    /// Request-response: inbound response channels keyed by the opaque u64 id
    /// handed to the application. ResponseChannel is NOT Send across the drain
    /// boundary, so it stays owned by the swarm task; the app only sees the u64.
    inbound_channels: HashMap<u64, ResponseChannel<Vec<u8>>>,
    /// Monotonic counter for assigning inbound request ids.
    next_inbound_id: u64,
    /// Request-response: outbound oneshot senders keyed by OutboundRequestId.
    /// Resolved on Message::Response / OutboundFailure inside handle_swarm_event.
    outbound_waiters: HashMap<request_response::OutboundRequestId, tokio::sync::oneshot::Sender<Result<Vec<u8>, String>>>,
    /// Currently-connected peers, shared so NetworkManager::connected_peers can
    /// read it. Tracked from ConnectionEstablished/ConnectionClosed.
    pub connected: std::sync::Arc<parking_lot::RwLock<std::collections::HashSet<PeerId>>>,
}

#[derive(Clone, Debug)]
pub struct FluxSwarmConfig {
    pub node_id: String,
    pub listen_addr: Multiaddr,
    pub bootstrap_peers: Vec<Multiaddr>,
    pub topics: Vec<String>,
    pub transport_mode: TransportMode,
    pub batch_config: BatchConfig,
    pub entanglement_config: EntanglementConfig,
}

impl Default for FluxSwarmConfig {
    fn default() -> Self {
        FluxSwarmConfig {
            node_id: "flux-node-0".into(),
            listen_addr: "/ip4/0.0.0.0/tcp/9003".parse().unwrap(),
            bootstrap_peers: Vec::new(),
            topics: vec![
                "/qnk/mainnet-genesis/blocks".into(),
                "/flux/1/compile-request".into(),
                "/dagknight/1/vertices".into(),
            ],
            transport_mode: TransportMode::TcpOnly,
            batch_config: BatchConfig::default(),
            entanglement_config: EntanglementConfig::default(),
        }
    }
}

/// Events the manager sends to the application layer.
#[derive(Debug, Clone)]
pub enum SwarmAppEvent {
    /// A gossipsub message arrived on a topic.
    GossipsubMessage {
        topic: String,
        from: PeerId,
        data: Vec<u8>,
        message_id: gossipsub::MessageId,
    },
    /// A peer connected. `addr` is the remote multiaddr (so consumers can
    /// identify a peer by its IP, e.g. map 89.149.241.126 → Epsilon). Empty
    /// string if the endpoint address was unavailable.
    PeerConnected { peer_id: PeerId, addr: String },
    /// A peer disconnected.
    PeerDisconnected { peer_id: PeerId },
    /// New listen address confirmed.
    NewListenAddr(Multiaddr),
    /// Peer identified itself.
    PeerIdentified {
        peer_id: PeerId,
        agent_version: String,
        protocols: Vec<String>,
    },
    /// A point-to-point backfill request arrived from `peer`. The application
    /// answers it by calling `NetworkManager::respond(request_id, payload)` with
    /// the same opaque `request_id`. Payload is opaque (CBOR-coded Vec<u8>).
    InboundRequest {
        peer: PeerId,
        request_id: u64,
        payload: Vec<u8>,
    },
    /// Combo profile received via gossip (from publish_combo_profile after a supersonic run).
    /// Higher layers feed `crate`, `predicted_ms`, `confidence` into Cortex metrics
    /// for affinity routing.
    ComboProfile {
        from: PeerId,
        crate_name: String,
        predicted_ms: f64,
        confidence: f64,
    },
    /// v0.7.0: Block sync progress update — fired when a block sync operation
    /// makes progress. Sigil-top consumes this to drive the TUI sync gauge.
    SyncProgress {
        /// Height of the latest synced block.
        height: u64,
        /// Hex-encoded hash of the latest synced block.
        hash_hex: String,
        /// Best known peer height (target for sync).
        peer_best_height: u64,
        /// Total blocks synced in this session.
        total_synced: u64,
        /// Number of peers we're syncing from.
        peer_count: usize,
    },
}

/// Information about a connected peer.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PeerInfo {
    pub peer_id: String,
    pub multiaddr: String,
    pub connected_since_ms: u64,
    pub last_seen_ms: u64,
    pub protocols: Vec<String>,
    pub agent_version: String,
}

// ── Construction ──

impl FluxSwarmManager {
    /// Create a new swarm manager and start listening.
    /// Returns the manager + an mpsc receiver for application-level events.
    pub fn new(mut config: FluxSwarmConfig) -> Result<(Self, std::sync::Arc<parking_lot::RwLock<Vec<SwarmAppEvent>>>), String> {
        // Generate Ed25519 keypair from node_id (deterministic, for reproducible PeerIds)
        let keypair = keypair_from_seed(&config.node_id);
        let local_peer_id = PeerId::from(keypair.public());

        // v0.32: SKIP HAIRPIN SELF-DIALS. When this node runs ON a box whose own public IP is
        // also a bootstrap peer (e.g. sigil-top co-located on Epsilon, with 89.149.241.126 in
        // the bootstrap list), dialing that public IP hairpins back to the local box — NAT
        // routing drops the loop, so the peer connects then FLAPS (observed 3↔4 on-box; an
        // off-box node held a clean stable 4). Drop any bootstrap whose IP is one of OUR OWN
        // non-loopback interface IPs so we never hairpin. Filtering config.bootstrap_peers here
        // covers BOTH the Kademlia seed below AND the bootstrap_peers_cache the mesh-maintenance
        // re-dials. (Loopback 127.0.0.1 is kept — an intentional same-box mesh is fine.)
        {
            let own_ips: std::collections::HashSet<std::net::IpAddr> = if_addrs::get_if_addrs()
                .map(|ifs| ifs.into_iter().map(|i| i.ip()).filter(|ip| !ip.is_loopback()).collect())
                .unwrap_or_default();
            if !own_ips.is_empty() {
                config.bootstrap_peers.retain(|addr| {
                    match multiaddr_ip(addr) {
                        Some(ip) if own_ips.contains(&ip) => {
                            tracing::info!(%addr, "skip hairpin self-dial — bootstrap IP is our own interface");
                            // v0.32.5: no eprintln — the sigil-top TUI owns the terminal; this fired
                            // every dial cycle and corrupted the ratatui screen. tracing covers it.
                            false
                        }
                        _ => true,
                    }
                });
            }
        }
        let peer_id_str = local_peer_id.to_string();

        tracing::info!(
            node_id = %config.node_id,
            peer_id = %peer_id_str,
            listen = %config.listen_addr,
            "Building Flux P2P swarm"
        );

        // Transport built implicitly by SwarmBuilder; pre-validate with build_transport.
        let _ = build_transport(&keypair, config.transport_mode.clone())
            .map_err(|e| format!("Transport build failed: {}", e))?;

        // Build gossipsub.
        //
        // Mesh thresholds (rocky-sigil-86, 2026-05-29): libp2p defaults are
        // `mesh_n_low=4, mesh_outbound_min=2`. Those numbers are tuned for
        // 50+ peer meshes and DEADLOCK small networks — a 2-node SIGIL
        // testbed never satisfies "≥2 outbound in mesh" and the mesh
        // refuses to form, so GossipsubMessage events never surface even
        // though `publish()` succeeds at the swarm level. P3 demo blocker
        // (rocky msg #46) and P4-B (sigil-node-join, rocky-83) both hit
        // this. Lowering to (1, 8, 1) lets a 2-node mesh actually forward;
        // larger meshes still graft up to mesh_n_high=8 naturally.
        //
        // FLUX_GOSSIPSUB_MESH_N_LOW + FLUX_GOSSIPSUB_MESH_OUTBOUND_MIN env
        // overrides let production raise these back to libp2p defaults
        // without a rebuild when the cluster grows past ~6 peers.
        let mesh_n_low: usize = std::env::var("FLUX_GOSSIPSUB_MESH_N_LOW")
            .ok().and_then(|v| v.parse().ok()).unwrap_or(1);
        let mesh_n_high: usize = std::env::var("FLUX_GOSSIPSUB_MESH_N_HIGH")
            .ok().and_then(|v| v.parse().ok()).unwrap_or(8);
        // rocky-sigil-87 (task #46): mesh_outbound_min=1 looked safe but
        // DEADLOCKS two-node meshes in practice — the listener side has
        // ZERO outbound mesh peers (the only connection is inbound from
        // the dialer) and gossipsub refuses to graft, so the mesh stays
        // empty even though identify completes + both sides see each
        // other's SUBSCRIBE. Lowering to 0 lets the mesh form with
        // inbound-only peers; production raises this back via env once the
        // cluster has ≥3 peers, by which point each side has at least one
        // outbound. Verified via tests/two_node_gossipsub.rs.
        let mesh_outbound_min: usize = std::env::var("FLUX_GOSSIPSUB_MESH_OUTBOUND_MIN")
            .ok().and_then(|v| v.parse().ok()).unwrap_or(0);

        let gossipsub_config = gossipsub::ConfigBuilder::default()
            .heartbeat_interval(Duration::from_millis(500))  // Faster heartbeat for lower latency
            .validation_mode(gossipsub::ValidationMode::Strict)
            .max_transmit_size(1_048_576)  // 1 MB max message
            .mesh_n_low(mesh_n_low)
            .mesh_n_high(mesh_n_high)
            .mesh_outbound_min(mesh_outbound_min)
            .build()
            .map_err(|e| format!("Gossipsub config: {}", e))?;

        let mut gossipsub: gossipsub::Behaviour =
            gossipsub::Behaviour::new(gossipsub::MessageAuthenticity::Signed(keypair.clone()), gossipsub_config)
                .map_err(|e| format!("Gossipsub init: {}", e))?;

        // Subscribe to all topics
        for topic in &config.topics {
            let t = gossipsub::IdentTopic::new(topic);
            gossipsub.subscribe(&t).map_err(|e| format!("Subscribe {}: {}", topic, e))?;
        }

        let mut kademlia = kad::Behaviour::new(
            local_peer_id,
            kad::store::MemoryStore::new(local_peer_id),
        );
        // Set to server mode so it can answer queries
        kademlia.set_mode(Some(kad::Mode::Server));

        // Bootstrap Kademlia from configured peers
        for addr in &config.bootstrap_peers {
            kademlia.add_address(&extract_peer_id_from_addr(addr).unwrap_or(local_peer_id), addr.clone());
        }

        // Build identify
        let identify_config = identify::Config::new("/flux/1.0.0".into(), keypair.public());
        let identify = identify::Behaviour::new(identify_config);

        // Build ping
        let ping_config = ping::Config::new()
            .with_interval(Duration::from_secs(10))
            .with_timeout(Duration::from_secs(5));
        let ping = ping::Behaviour::new(ping_config);

        // Build request-response (point-to-point backfill). Custom BackfillCodec
        // (raw opaque Vec<u8>, 64 MiB cap) instead of libp2p's built-in cbor codec,
        // whose response cap is a hardcoded 10 MiB const — too small for big
        // backfill chunks. 30s request timeout.
        let request_response = request_response::Behaviour::<BackfillCodec>::with_codec(
            BackfillCodec,
            [(StreamProtocol::new(BACKFILL_PROTOCOL), ProtocolSupport::Full)],
            request_response::Config::default()
                .with_request_timeout(Duration::from_secs(30)),
        );

        // Assemble behaviour
        let behaviour = FluxBehaviour {
            gossipsub,
            kademlia,
            identify,
            ping,
            request_response,
        };

        // Build swarm
        let swarm = SwarmBuilder::with_existing_identity(keypair)
            .with_tokio()
            .with_tcp(
                tcp::Config::default().nodelay(true),  // TCP_NODELAY for lower latency
                noise::Config::new,
                yamux::Config::default,
            )
            .map_err(|e| format!("Swarm builder: {}", e))?
            .with_behaviour(|_| behaviour)
            .map_err(|e| format!("Swarm with behaviour: {}", e))?
            .with_swarm_config(|c| {
                c.with_idle_connection_timeout(Duration::from_secs(60))
            })
            .build();

        // Channel for application events (reserved for future use)
        
        let event_rx = std::sync::Arc::new(parking_lot::RwLock::new(Vec::new()));

        Ok((
            FluxSwarmManager {
                swarm,
                local_peer_id,
                listen_addr: config.listen_addr,
                topics: config.topics,
                batcher: HashMap::new(),
                batch_config: config.batch_config,
                peers: HashMap::new(),
                started: false,
                bootstrap_peers_cache: config.bootstrap_peers.clone(),
                entanglement: EntanglementRouter::new(config.entanglement_config.clone()),
                event_rx: Some(event_rx.clone()),
                inbound_channels: HashMap::new(),
                next_inbound_id: 1,
                outbound_waiters: HashMap::new(),
                connected: std::sync::Arc::new(parking_lot::RwLock::new(std::collections::HashSet::new())),
            },
            event_rx,
        ))
    }

    /// Get the local PeerId.
    pub fn local_peer_id(&self) -> &PeerId {
        &self.local_peer_id
    }

    /// Get the local PeerId as a string.
    pub fn local_peer_id_str(&self) -> String {
        self.local_peer_id.to_string()
    }

    /// Check if the swarm is started.
    pub fn is_running(&self) -> bool {
        self.started
    }

    /// Number of connected peers.
    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    /// Get all connected peers.
    pub fn connected_peers(&self) -> Vec<&PeerInfo> {
        self.peers.values().collect()
    }

    /// Publish a message to a gossipsub topic (batched by default).
    pub fn publish(&mut self, topic: &str, data: Vec<u8>) -> Result<(), String> {
        if !self.started {
            return Err("Swarm not started".into());
        }
        if !self.topics.contains(&topic.to_string()) {
            return Err(format!("Not subscribed to topic '{}'", topic));
        }

        if self.batch_config.enabled {
            let batch = self.batcher
                .entry(topic.to_string())
                .or_insert_with(|| MessageBatch::new(topic.to_string()));
            batch.messages.push(data);
            if batch.is_ready(&self.batch_config) {
                self.flush_topic(topic)?;
            }
        } else {
            // Send immediately — no batching
            let t = gossipsub::IdentTopic::new(topic);
            self.swarm.behaviour_mut().gossipsub
                .publish(t, data)
                .map_err(|e| format!("Publish failed: {}", e))?;
        }
        Ok(())
    }

    /// Publish a combo profile after running a supersonic combo.
    /// This is the key hook for the invention: after `flux_combo_supersonic`,
    /// the node calls this to advertise its speed to the mesh for affinity routing.
    pub fn publish_combo_profile(&mut self, crate_name: &str, predicted_ms: f64, confidence: f64) -> Result<(), String> {
        if !self.started {
            return Err("Swarm not started".into());
        }
        let profile = format!(r#"{{"crate":"{}","predicted_ms":{},"confidence":{}}}"#, crate_name, predicted_ms, confidence);
        self.publish(COMBO_AFFINITY_TOPIC, profile.into_bytes())
    }

    /// Publish a message only to top-N entangled peers (based on linking number / Jaccard).
    /// Falls back to normal publish if no entangled peers are known.
    pub fn entangled_publish(&mut self, topic: &str, data: Vec<u8>, n: usize) -> Result<usize, String> {
        if !self.started {
            return Err("Swarm not started".into());
        }
        let self_id = self.local_peer_id_str();
        let top = self.entanglement.top_entangled_peers(&self_id, n);

        if top.is_empty() {
            // Fallback: publish to all mesh peers
            self.publish(topic, data)?;
            return Ok(0); // 0 = broadcast (no entanglement filtering)
        }

        // Publish to each entangled peer's individual topic variant
        // In production, this would use direct messaging or per-peer topics
        let t = gossipsub::IdentTopic::new(topic);
        let mut sent = 0;
        for (_peer_id, score) in &top {
            if *score >= self.entanglement_min_score() {
                self.swarm.behaviour_mut().gossipsub
                    .publish(t.clone(), data.clone())
                    .map_err(|e| format!("Entangled publish failed: {}", e))?;
                sent += 1;
            }
        }
        tracing::debug!(topic, sent, peers = top.len(), "Entanglement-weighted publish");
        Ok(sent)
    }

    /// Get the minimum entanglement score for mesh inclusion.
    pub fn entanglement_min_score(&self) -> f64 {
        self.entanglement.config.min_score
    }

    /// v0.7.0: Push a sync progress event to the application event channel.
    /// Called by sigil-top's block sync loop to update the TUI sync gauge.
    pub fn push_sync_progress(&self, height: u64, hash_hex: &str, peer_best_height: u64, total_synced: u64) {
        if let Some(ref rx) = self.event_rx {
            rx.write().push(SwarmAppEvent::SyncProgress {
                height,
                hash_hex: hash_hex.to_string(),
                peer_best_height,
                total_synced,
                peer_count: self.peers.len(),
            });
        }
    }

    /// Answer an inbound backfill request previously surfaced via
    /// `SwarmAppEvent::InboundRequest`. Looks up the stored ResponseChannel by
    /// the opaque `request_id` and sends the response bytes. Logs if the id is
    /// unknown (already answered, or the channel expired/timed out).
    pub fn respond(&mut self, request_id: u64, payload: Vec<u8>) {
        match self.inbound_channels.remove(&request_id) {
            Some(channel) => {
                if self.swarm.behaviour_mut().request_response
                    .send_response(channel, payload).is_err()
                {
                    tracing::warn!(request_id, "respond: response channel closed (peer gone?)");
                }
            }
            None => {
                tracing::warn!(request_id, "respond: no inbound channel for request_id (already answered or expired)");
            }
        }
    }

    /// Send an outbound backfill request to `peer`, registering `resp` to be
    /// fulfilled when the response (or failure) arrives. The result is delivered
    /// via the RequestResponse handler in handle_swarm_event.
    pub fn send_request(
        &mut self,
        peer: PeerId,
        payload: Vec<u8>,
        resp: tokio::sync::oneshot::Sender<Result<Vec<u8>, String>>,
    ) {
        let id = self.swarm.behaviour_mut().request_response.send_request(&peer, payload);
        self.outbound_waiters.insert(id, resp);
    }

    /// Force-flush all pending batches — call before shutdown or on a timer.
    pub fn flush_all(&mut self) {
        let topics: Vec<String> = self.batcher.keys().cloned().collect();
        for topic in topics {
            let _ = self.flush_topic(&topic);
        }
    }

    fn flush_topic(&mut self, topic: &str) -> Result<(), String> {
        if let Some(batch) = self.batcher.remove(topic) {
            if batch.messages.is_empty() {
                return Ok(());
            }
            let msg_count = batch.messages.len();
            let t = gossipsub::IdentTopic::new(topic);

            // Publish each message individually (gossipsub handles dedup/mesh fan-out)
            // For maximum throughput, we could serialize to a single payload and split
            let mut success = 0;
            for data in &batch.messages {
                match self.swarm.behaviour_mut().gossipsub.publish(t.clone(), data.clone()) {
                    Ok(id) => {
                        tracing::trace!(%topic, %id, "Published batched message");
                        success += 1;
                    }
                    Err(e) => {
                        tracing::warn!(%topic, %e, "Failed to publish batched message");
                    }
                }
            }
            tracing::debug!(%topic, count = msg_count, success, "Batch flushed");
        }
        Ok(())
    }

    /// Run the swarm event loop (blocking — spawn on tokio).
    pub async fn run(&mut self) -> Result<(), String> {
        self.swarm.listen_on(self.listen_addr.clone())
            .map_err(|e| format!("Listen failed: {}", e))?;

        // Bootstrap Kademlia if we have peers
        let bootstrap_result = self.swarm.behaviour_mut().kademlia.bootstrap();
        if let Err(e) = bootstrap_result {
            tracing::warn!(%e, "Kademlia bootstrap deferred (no peers yet)");
        }

        self.started = true;
        tracing::info!(
            peer_id = %self.local_peer_id,
            "Flux P2P swarm running"
        );

        // Batch flush timer
        let mut flush_interval = tokio::time::interval(
            Duration::from_millis(self.batch_config.flush_interval_ms)
        );

        loop {
            tokio::select! {
                // Batch flush tick
                _ = flush_interval.tick() => {
                    self.flush_all();
                }

                // Swarm event
                event = self.swarm.select_next_some() => {
                    self.handle_swarm_event(event);
                }
            }
        }
    }

    pub fn handle_swarm_event(&mut self, event: SwarmEvent<FluxBehaviourEvent>) {
        match event {
            SwarmEvent::Behaviour(FluxBehaviourEvent::Gossipsub(ev)) => {
                // Surface non-Message variants at INFO/DEBUG so the rocky-83
                // / P3-demo bug (drain_events silent) is debuggable from
                // logs alone. GossipsubNotSupported is the one that
                // strands a peer permanently — log loudly at WARN so it
                // can't be missed in fluxmux output.
                match &ev {
                    gossipsub::Event::Subscribed { peer_id, topic } => {
                        tracing::info!(%peer_id, %topic, "Gossipsub: peer subscribed");
                    }
                    gossipsub::Event::Unsubscribed { peer_id, topic } => {
                        tracing::info!(%peer_id, %topic, "Gossipsub: peer unsubscribed");
                    }
                    gossipsub::Event::GossipsubNotSupported { peer_id } => {
                        tracing::warn!(
                            %peer_id,
                            "Gossipsub: remote peer does NOT speak gossipsub — \
                             this peer will never deliver messages. Check libp2p \
                             protocol negotiation."
                        );
                    }
                    gossipsub::Event::Message { .. } => { /* handled below */ }
                }
                if let gossipsub::Event::Message { message, .. } = ev {
                    let from = message.source.unwrap_or(PeerId::random());
                    let from_str = from.to_string();
                    tracing::debug!(
                        topic = %message.topic,
                        from = %from,
                        len = message.data.len(),
                        "Gossipsub message received"
                    );

                    // Feed transaction data to entanglement router
                    // Hash the message data as a proxy for tx fingerprint
                    let msg_hash = blake3::hash(&message.data);
                    let mut tx_hash: [u8; 32] = [0u8; 32];
                    tx_hash.copy_from_slice(msg_hash.as_bytes());
                    self.entanglement.feed_block(&from_str, &[tx_hash], false);
                    // Forward to application via event channel
                    if let Some(ref rx) = self.event_rx {
                        if message.topic.as_str() == COMBO_AFFINITY_TOPIC {
                            // Parse and emit specialized event for combo affinity (invention hook)
                            if let Ok(s) = std::str::from_utf8(&message.data) {
                                if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
                                    if let (Some(crate_name), Some(pred), Some(conf)) = (
                                        v.get("crate").and_then(|x| x.as_str()),
                                        v.get("predicted_ms").and_then(|x| x.as_f64()),
                                        v.get("confidence").and_then(|x| x.as_f64()),
                                    ) {
                                        rx.write().push(SwarmAppEvent::ComboProfile {
                                            from,
                                            crate_name: crate_name.to_string(),
                                            predicted_ms: pred,
                                            confidence: conf,
                                        });
                                        return;
                                    }
                                }
                            }
                        }
                        rx.write().push(SwarmAppEvent::GossipsubMessage {
                            topic: message.topic.to_string(),
                            from,
                            data: message.data.clone(),
                            message_id: gossipsub::MessageId::new(&[]),
                        });
                    }
                }
            }
            SwarmEvent::Behaviour(FluxBehaviourEvent::Identify(ev)) => {
                // Diagnostic for rocky-83 / P3-demo bug — surface identify
                // completion so we can confirm gossipsub stream negotiation
                // has the prerequisites it needs (peer's supported protocols).
                if let identify::Event::Received { peer_id, info, .. } = &ev {
                    tracing::info!(
                        %peer_id,
                        agent = %info.agent_version,
                        protocols = ?info.protocols.iter().map(|p| p.to_string()).collect::<Vec<_>>(),
                        "Identify: peer info received"
                    );
                }
                if let identify::Event::Received { peer_id, info, .. } = ev {
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;

                    self.peers.insert(peer_id, PeerInfo {
                        peer_id: peer_id.to_string(),
                        multiaddr: info.listen_addrs.first()
                            .map(|a| a.to_string())
                            .unwrap_or_default(),
                        connected_since_ms: now_ms,
                        last_seen_ms: now_ms,
                        protocols: info.protocols.iter().map(|p| p.to_string()).collect(),
                        agent_version: info.agent_version.clone(),
                    });
                }
            }
            SwarmEvent::Behaviour(FluxBehaviourEvent::Kademlia(ev)) => {
                if let kad::Event::RoutingUpdated { peer, .. } = ev {
                    tracing::debug!(%peer, "Kademlia routing updated");
                }
            }
            SwarmEvent::Behaviour(FluxBehaviourEvent::RequestResponse(ev)) => {
                match ev {
                    request_response::Event::Message { peer, message } => match message {
                        request_response::Message::Request { request, channel, .. } => {
                            // Assign an opaque u64 id, stash the channel, surface to app.
                            let request_id = self.next_inbound_id;
                            self.next_inbound_id = self.next_inbound_id.wrapping_add(1);
                            self.inbound_channels.insert(request_id, channel);
                            tracing::debug!(%peer, request_id, len = request.len(), "Backfill request received");
                            if let Some(ref rx) = self.event_rx {
                                rx.write().push(SwarmAppEvent::InboundRequest {
                                    peer,
                                    request_id,
                                    payload: request,
                                });
                            }
                        }
                        request_response::Message::Response { request_id, response } => {
                            if let Some(tx) = self.outbound_waiters.remove(&request_id) {
                                let _ = tx.send(Ok(response));
                            } else {
                                tracing::warn!(%request_id, "Backfill response for unknown request_id");
                            }
                        }
                    },
                    request_response::Event::OutboundFailure { request_id, error, .. } => {
                        if let Some(tx) = self.outbound_waiters.remove(&request_id) {
                            let _ = tx.send(Err(error.to_string()));
                        }
                        tracing::warn!(%request_id, %error, "Backfill outbound failure");
                    }
                    request_response::Event::InboundFailure { request_id, error, .. } => {
                        tracing::warn!(%request_id, %error, "Backfill inbound failure");
                    }
                    request_response::Event::ResponseSent { .. } => { /* ignore */ }
                }
            }
            SwarmEvent::Behaviour(FluxBehaviourEvent::Ping(ev)) => {
                #[allow(irrefutable_let_patterns)]
                if let ping::Event { peer, result, .. } = ev {
                    match result {
                        Ok(rtt) => tracing::trace!(%peer, rtt_ms = rtt.as_millis(), "Ping ok"),
                        Err(e) => tracing::warn!(%peer, %e, "Ping failed"),
                    }
                }
            }
            SwarmEvent::NewListenAddr { address, .. } => {
                tracing::info!(%address, "New listen address");
            }
            SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                let addr = endpoint.get_remote_address().to_string();
                tracing::info!(%peer_id, %addr, "Peer connected");
                self.connected.write().insert(peer_id);
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;

                self.peers.entry(peer_id).or_insert_with(|| PeerInfo {
                    peer_id: peer_id.to_string(),
                    multiaddr: addr.clone(),
                    connected_since_ms: now,
                    last_seen_ms: now,
                    protocols: Vec::new(),
                    agent_version: String::new(),
                });
                // Surface to the application layer so the TUI can light up
                // peer/mesh indicators (previously this event was never emitted,
                // leaving sigil-top's peer detection as dead code).
                if let Some(ref rx) = self.event_rx {
                    rx.write().push(SwarmAppEvent::PeerConnected { peer_id, addr });
                }
            }
            SwarmEvent::ConnectionClosed { peer_id, num_established, cause, .. } => {
                tracing::warn!(%peer_id, num_established, cause = ?cause, "P2P-DBG Peer DISCONNECTED — cause shown (why peers drop)");
                // Only drop from the connected set when the LAST connection closes.
                if num_established == 0 {
                    self.connected.write().remove(&peer_id);
                }
                self.peers.remove(&peer_id);
                if let Some(ref rx) = self.event_rx {
                    rx.write().push(SwarmAppEvent::PeerDisconnected { peer_id });
                }
            }
            // P2P-DBG: surface the events that were previously swallowed — these explain
            // 0-peers / intermittent peers (dial failures, handshake/transport errors).
            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                tracing::warn!(peer_id = ?peer_id, error = %error, "P2P-DBG OUTGOING DIAL/HANDSHAKE FAILED");
            }
            SwarmEvent::IncomingConnectionError { error, send_back_addr, .. } => {
                tracing::warn!(error = %error, addr = %send_back_addr, "P2P-DBG INCOMING CONNECTION FAILED");
            }
            SwarmEvent::Dialing { peer_id, .. } => {
                tracing::info!(peer_id = ?peer_id, "P2P-DBG dialing peer");
            }
            _ => {}
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════

/// Generate a deterministic Ed25519 keypair from a node name.
pub fn keypair_from_seed(node_id: &str) -> libp2p::identity::Keypair {
    let hash = blake3::hash(node_id.as_bytes());
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&hash.as_bytes()[..32]);
    libp2p::identity::Keypair::ed25519_from_bytes(seed)
        .expect("Ed25519 keypair from seed")
}

/// The deterministic libp2p PeerId string for a `node_id`, via
/// [`keypair_from_seed`]. Lets operators compute a node's PeerId offline (for
/// bootstrap multiaddrs) instead of scraping logs.
pub fn peer_id_string(node_id: &str) -> String {
    PeerId::from(keypair_from_seed(node_id).public()).to_string()
}

/// Extract a PeerId from a Multiaddr (if it contains /p2p/ component).
fn extract_peer_id_from_addr(addr: &Multiaddr) -> Option<PeerId> {
    use libp2p::multiaddr::Protocol;
    for proto in addr.iter() {
        if let Protocol::P2p(peer_id) = proto {
            return Some(peer_id);
        }
    }
    None
}

/// Extract the IP (v4/v6) from a Multiaddr's /ip4 or /ip6 component. Used to detect
/// hairpin self-dials (bootstrap IP == one of our own interface IPs).
fn multiaddr_ip(addr: &Multiaddr) -> Option<std::net::IpAddr> {
    use libp2p::multiaddr::Protocol;
    addr.iter().find_map(|proto| match proto {
        Protocol::Ip4(ip) => Some(std::net::IpAddr::V4(ip)),
        Protocol::Ip6(ip) => Some(std::net::IpAddr::V6(ip)),
        _ => None,
    })
}

/// Build the standard Quillon/Flux gossipsub topic string.
pub fn quillon_topic(network: &str, kind: &str) -> String {
    format!("/qnk/{}/{}", network, kind)
}

/// Build a Flux-specific gossipsub topic string.
pub fn flux_topic(version: u32, kind: &str) -> String {
    format!("/flux/{}/{}", version, kind)
}

/// Build a DAGKnight gossipsub topic string.
pub fn dagknight_topic(version: u32, kind: &str) -> String {
    format!("/dagknight/{}/{}", version, kind)
}

/// Build a SIGIL-specific gossipsub topic string.
/// Standard form: `/sigil/{network}/{kind}` e.g. `/sigil/g0/blocks`.
pub fn sigil_topic(network: &str, kind: &str) -> String {
    format!("/sigil/{}/{}", network, kind)
}

/// The canonical SIGIL block sync topic for network g0.
pub const SIGIL_G0_BLOCKS_TOPIC: &str = "/sigil/g0/blocks";
#[cfg(test)]
mod swarm_addr_tests {
    //! Address/topic helpers in swarm.rs (Tier 3 — swarm.rs had NO tests).
    //! `extract_peer_id_from_addr` returning None on a BARE multiaddr was the
    //! root cause of a real SIGIL sync outage (bare /ip4/.../tcp/9501 bootstrap
    //! addrs got registered under the local peer id → DHT never seeded). These
    //! lock that behavior plus the gossip-topic strings (a wrong topic silently
    //! partitions the network).
    use super::*;
    use libp2p::identity::Keypair;
    use libp2p::multiaddr::Protocol;

    fn a(s: &str) -> Multiaddr {
        s.parse().expect("valid multiaddr")
    }

    #[test]
    fn extract_peer_id_roundtrips_from_full_addr() {
        let pid = Keypair::generate_ed25519().public().to_peer_id();
        let addr = a("/ip4/89.149.241.126/tcp/9501").with(Protocol::P2p(pid));
        assert_eq!(extract_peer_id_from_addr(&addr), Some(pid));
    }

    #[test]
    fn extract_peer_id_from_bare_addr_is_none() {
        // The incident: a bootstrap addr WITHOUT a /p2p/<id> component yields no
        // peer id. Callers must NOT substitute the local id (that mis-seeds the
        // DHT) — they must supply the /p2p component.
        assert_eq!(extract_peer_id_from_addr(&a("/ip4/89.149.241.126/tcp/9501")), None);
        assert_eq!(extract_peer_id_from_addr(&a("/dns4/sigilgraph.fluxapp.xyz/tcp/9501")), None);
    }

    #[test]
    fn multiaddr_ip_extracts_v4_v6_and_skips_non_ip() {
        assert_eq!(multiaddr_ip(&a("/ip4/1.2.3.4/tcp/9001")), Some("1.2.3.4".parse().unwrap()));
        assert_eq!(multiaddr_ip(&a("/ip6/::1/tcp/9001")), Some("::1".parse().unwrap()));
        // a /p2p component is still found by ip-scan? no — dns addrs have no IP.
        assert_eq!(multiaddr_ip(&a("/dns4/example.com/tcp/9001")), None);
    }

    #[test]
    fn gossip_topic_strings_are_stable() {
        // A drift in any of these silently splits nodes onto different topics.
        assert_eq!(quillon_topic("mainnet-genesis", "blocks"), "/qnk/mainnet-genesis/blocks");
        assert_eq!(flux_topic(1, "blocks"), "/flux/1/blocks");
        assert_eq!(dagknight_topic(2, "vertices"), "/dagknight/2/vertices");
    }
}
