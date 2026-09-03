//! The mesh — a real one this time.
//!
//! This module exists because of the single most important thing the analysis
//! turned up. `void-walker::tor_mesh` presents a complete networking API:
//! `AquaMesh::spawn`, `broadcast_block`, `broadcast_analytics`, `add_peer`,
//! `run_service`, peer tables with trust and connection-quality scores, a
//! `SecureAquaMessage` envelope with a signature and a nonce. It is 418 lines of
//! plausible network code and **it has never sent a byte**:
//!
//! ```text
//! async fn send_to_peer(&self, peer_onion: &str, message: &SecureAquaMessage) -> Result<()> {
//!     if let Some(tor_client) = &self.tor_client {
//!         let serialized = serde_json::to_vec(message)?;
//!         debug!("Sending {} bytes to {}", serialized.len(), peer_onion);
//!         // TODO: Integrate with QTorClient's message sending
//!     } else {
//!         debug!("Mock sending message to {}", peer_onion);
//!     }
//!     Ok(())
//! }
//! ```
//!
//! It serialises the message, logs the length, drops it, and returns `Ok(())`.
//! `tor_client` is `Some(format!("tor-client-{onion}"))` — a string, not a
//! client. `signature: vec![0; 64]` carries a `TODO`. `encrypted: false` carries
//! a `TODO`. `run_service` sleeps 100 ms and returns success. Every caller sees
//! `Ok`, so nothing anywhere reports a failure. This is the "committed but never
//! called" failure class, in its purest form: the code exists, the tests pass —
//! `test_message_broadcasting` asserts exactly `result.is_ok()` — and the
//! feature does not work.
//!
//! The fix is not to finish the Tor integration. It is to delete the fake
//! transport and hand the colony a transport that already works: `flux-p2p`'s
//! libp2p gossipsub stack, the same one carrying SIGIL blocks today.
//!
//! The codec below is always compiled and always tested. Only the transport is
//! behind the `p2p` feature.

use serde::{Deserialize, Serialize};

use crate::colony::ColonyMetrics;
use crate::ledger::BridgeBlock;
use crate::species::Species;

/// Gossip topic carrying new bridge blocks.
pub const TOPIC_BRIDGE: &str = "/flux/aqua/1/bridge";
/// Gossip topic carrying droplet announcements.
pub const TOPIC_DROPLET: &str = "/flux/aqua/1/droplet";
/// Gossip topic carrying colony health ("cosmic weather") reports.
pub const TOPIC_WEATHER: &str = "/flux/aqua/1/weather";

/// Every Aqua gossip topic, for subscription.
pub const TOPICS: &[&str] = &[TOPIC_BRIDGE, TOPIC_DROPLET, TOPIC_WEATHER];

/// Wire format version. Bumped on any breaking change to [`AquaMessage`].
pub const WIRE_VERSION: u16 = 1;

/// A message on the Aqua mesh.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AquaMessage {
    /// A droplet announcing itself.
    Announce {
        droplet_id: String,
        species: Species,
        /// Advisory fan-out this droplet's role wants.
        fan_out: u32,
    },
    /// A newly committed bridge block.
    Bridge(Box<BridgeBlock>),
    /// A colony health report.
    Weather {
        colony_id: String,
        metrics: Box<ColonyMetrics>,
    },
}

impl AquaMessage {
    /// Which topic this message belongs on.
    pub fn topic(&self) -> &'static str {
        match self {
            AquaMessage::Announce { .. } => TOPIC_DROPLET,
            AquaMessage::Bridge(_) => TOPIC_BRIDGE,
            AquaMessage::Weather { .. } => TOPIC_WEATHER,
        }
    }
}

/// A versioned envelope. The source's envelope carried a `signature` field it
/// filled with 64 zero bytes and an `encrypted` flag it hard-coded to `false`;
/// both are omitted here rather than faked. Gossipsub already authenticates the
/// sender's `PeerId`, and when Aqua needs payload signatures the right answer is
/// `flux-sigil`, not a zeroed vector.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub version: u16,
    pub sender: String,
    pub sent_at_ms: u64,
    pub message: AquaMessage,
}

/// Why a received frame was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WireError {
    /// Payload was not valid JSON for an [`Envelope`].
    Malformed,
    /// Envelope version this build does not speak.
    UnsupportedVersion(u16),
    /// Message arrived on a topic it does not belong to.
    TopicMismatch,
}

/// Encode a message for the wire.
pub fn encode(sender: &str, sent_at_ms: u64, message: AquaMessage) -> (&'static str, Vec<u8>) {
    let topic = message.topic();
    let env = Envelope {
        version: WIRE_VERSION,
        sender: sender.to_string(),
        sent_at_ms,
        message,
    };
    // Serialization of an Envelope cannot fail — every field is a plain owned
    // type — but we never unwrap on a network path.
    let bytes = serde_json::to_vec(&env).unwrap_or_default();
    (topic, bytes)
}

/// Decode a frame received on `topic`, rejecting anything malformed, from a
/// future version, or delivered on the wrong topic.
pub fn decode(topic: &str, bytes: &[u8]) -> Result<Envelope, WireError> {
    let env: Envelope = serde_json::from_slice(bytes).map_err(|_| WireError::Malformed)?;
    if env.version != WIRE_VERSION {
        return Err(WireError::UnsupportedVersion(env.version));
    }
    if env.message.topic() != topic {
        return Err(WireError::TopicMismatch);
    }
    Ok(env)
}

// ═══════════════════════════════════════════════════════════════
// Real transport (feature = "p2p")
// ═══════════════════════════════════════════════════════════════

#[cfg(feature = "p2p")]
mod transport {
    use super::*;
    use flux_p2p::{NetworkConfig, NetworkManager, SwarmAppEvent};

    /// A colony's connection to the Aqua mesh, over libp2p gossipsub.
    pub struct AquaMesh {
        node_id: String,
        net: NetworkManager,
        sent: u64,
        received: u64,
        rejected: u64,
    }

    impl AquaMesh {
        /// Build a mesh node. `listen_addr` is a libp2p multiaddr, e.g.
        /// `/ip4/0.0.0.0/tcp/9013`.
        pub fn new(node_id: &str, listen_addr: &str, bootstrap: Vec<String>) -> Self {
            let config = NetworkConfig {
                node_id: node_id.to_string(),
                listen_addr: listen_addr.to_string(),
                bootstrap_peers: bootstrap,
                dagknight_enabled: false,
                sap_enabled: true,
                x_algo_enabled: false,
                entanglement_enabled: false,
                gossipsub_topics: TOPICS.iter().map(|t| t.to_string()).collect(),
            };
            Self {
                node_id: node_id.to_string(),
                net: NetworkManager::new(config),
                sent: 0,
                received: 0,
                rejected: 0,
            }
        }

        /// Start the swarm. Unlike the source's `run_service` — which slept
        /// 100 ms and returned `Ok(())` — a failure here is reported.
        pub async fn start(&mut self) -> Result<(), String> {
            self.net.start().await
        }

        pub async fn stop(&self) -> Result<(), String> {
            self.net.stop().await
        }

        pub fn node_id(&self) -> &str {
            &self.node_id
        }

        pub fn peer_count(&self) -> u32 {
            self.net.peer_count()
        }

        /// Publish a message. Propagates the transport's error instead of
        /// swallowing it.
        pub fn publish(&mut self, sent_at_ms: u64, message: AquaMessage) -> Result<(), String> {
            let (topic, bytes) = encode(&self.node_id, sent_at_ms, message);
            self.net.publish(topic, bytes)?;
            self.sent += 1;
            Ok(())
        }

        /// Announce a droplet to the mesh.
        pub fn announce(
            &mut self,
            sent_at_ms: u64,
            droplet_id: &str,
            species: Species,
        ) -> Result<(), String> {
            let fan_out = species.fan_out(self.net.peer_count());
            self.publish(
                sent_at_ms,
                AquaMessage::Announce {
                    droplet_id: droplet_id.to_string(),
                    species,
                    fan_out,
                },
            )
        }

        /// Announce a committed bridge block.
        pub fn publish_bridge(
            &mut self,
            sent_at_ms: u64,
            block: BridgeBlock,
        ) -> Result<(), String> {
            self.publish(sent_at_ms, AquaMessage::Bridge(Box::new(block)))
        }

        /// Publish a colony health report.
        pub fn publish_weather(
            &mut self,
            sent_at_ms: u64,
            colony_id: &str,
            metrics: ColonyMetrics,
        ) -> Result<(), String> {
            self.publish(
                sent_at_ms,
                AquaMessage::Weather {
                    colony_id: colony_id.to_string(),
                    metrics: Box::new(metrics),
                },
            )
        }

        /// Drain inbound gossip, decoding Aqua frames and discarding everything
        /// else. Frames that fail to decode are counted, not silently dropped.
        pub fn drain(&mut self) -> Vec<Envelope> {
            let mut out = Vec::new();
            for ev in self.net.drain_events() {
                if let SwarmAppEvent::GossipsubMessage { topic, data, .. } = ev {
                    if !TOPICS.contains(&topic.as_str()) {
                        continue;
                    }
                    match decode(&topic, &data) {
                        Ok(env) => {
                            self.received += 1;
                            out.push(env);
                        }
                        Err(_) => self.rejected += 1,
                    }
                }
            }
            out
        }

        /// Counters: `(sent, received, rejected)`.
        pub fn counters(&self) -> (u64, u64, u64) {
            (self.sent, self.received, self.rejected)
        }

        /// Access the underlying network manager, for SAP scores and mesh health.
        pub fn network(&self) -> &NetworkManager {
            &self.net
        }
    }
}

#[cfg(feature = "p2p")]
pub use transport::AquaMesh;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::colony::AquaColony;
    use crate::ledger::BridgeChain;

    fn sample_block() -> BridgeBlock {
        BridgeChain::new()
            .next_block(
                0.42,
                3,
                [7u8; 32],
                Species::Scout,
                "scout-abc",
                0.61,
                1_700_000_000_000,
            )
            .expect("sample inputs are in range")
    }

    #[test]
    fn messages_route_to_their_own_topic() {
        assert_eq!(
            AquaMessage::Bridge(Box::new(sample_block())).topic(),
            TOPIC_BRIDGE
        );
        assert_eq!(
            AquaMessage::Announce {
                droplet_id: "d".into(),
                species: Species::Trader,
                fan_out: 3
            }
            .topic(),
            TOPIC_DROPLET
        );
        let m = AquaColony::new(1).metrics();
        assert_eq!(
            AquaMessage::Weather {
                colony_id: "c".into(),
                metrics: Box::new(m)
            }
            .topic(),
            TOPIC_WEATHER
        );
    }

    #[test]
    fn encode_decode_roundtrip() {
        let msg = AquaMessage::Bridge(Box::new(sample_block()));
        let (topic, bytes) = encode("node-a", 1234, msg.clone());
        let env = decode(topic, &bytes).unwrap();
        assert_eq!(env.version, WIRE_VERSION);
        assert_eq!(env.sender, "node-a");
        assert_eq!(env.sent_at_ms, 1234);
        assert_eq!(env.message, msg);
    }

    #[test]
    fn a_bridge_survives_the_wire_with_its_hash_intact() {
        // The property that matters for consensus: a block must hash the same
        // after a round-trip, or peers will disagree about the chain.
        let block = sample_block();
        let before = block.hash();
        let (topic, bytes) = encode("n", 1, AquaMessage::Bridge(Box::new(block)));
        match decode(topic, &bytes).unwrap().message {
            AquaMessage::Bridge(b) => assert_eq!(b.hash(), before),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn garbage_is_rejected_not_accepted() {
        assert_eq!(decode(TOPIC_BRIDGE, b""), Err(WireError::Malformed));
        assert_eq!(decode(TOPIC_BRIDGE, b"not json"), Err(WireError::Malformed));
        assert_eq!(
            decode(TOPIC_BRIDGE, br#"{"version":1}"#),
            Err(WireError::Malformed)
        );
    }

    #[test]
    fn a_future_wire_version_is_rejected() {
        let (_, bytes) = encode(
            "n",
            1,
            AquaMessage::Announce {
                droplet_id: "d".into(),
                species: Species::Scout,
                fan_out: 1,
            },
        );
        let mut env: Envelope = serde_json::from_slice(&bytes).unwrap();
        env.version = 999;
        let raw = serde_json::to_vec(&env).unwrap();
        assert_eq!(
            decode(TOPIC_DROPLET, &raw),
            Err(WireError::UnsupportedVersion(999))
        );
    }

    #[test]
    fn a_message_on_the_wrong_topic_is_rejected() {
        let (_, bytes) = encode("n", 1, AquaMessage::Bridge(Box::new(sample_block())));
        assert_eq!(decode(TOPIC_WEATHER, &bytes), Err(WireError::TopicMismatch));
    }

    #[test]
    fn topics_are_distinct_and_namespaced() {
        let mut t = TOPICS.to_vec();
        t.sort_unstable();
        let n = t.len();
        t.dedup();
        assert_eq!(n, t.len());
        assert!(TOPICS.iter().all(|t| t.starts_with("/flux/aqua/1/")));
    }
}
