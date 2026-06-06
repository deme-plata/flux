//! flux-bluetooth — BLE mesh networking for Flux P2P.
//!
//! Bridges Bluetooth Low Energy (BLE) mesh with libp2p gossipsub,
//! creating an off-grid, post-quantum encrypted mesh network that
//! beats BitChat and similar BLE chat apps.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │                    flux-bluetooth                        │
//! │  ┌──────────┐  ┌──────────────┐  ┌───────────────────┐ │
//! │  │ BLE Mesh │─→│ P2P Bridge   │─→│ flux-p2p gossipsub │ │
//! │  │ (advert) │  │ (mesh ↔ P2P) │  │ (libp2p overlay)  │ │
//! │  └──────────┘  └──────────────┘  └───────────────────┘ │
//! │       │               │                                  │
//! │  ┌────▼────┐   ┌─────▼──────┐                           │
//! │  │ Crypto  │   │ Chat Proto │                           │
//! │  │ PQ sig  │   │ (PQ E2EE)  │                           │
//! │  │ + KEM   │   │            │                           │
//! │  └─────────┘   └────────────┘                           │
//! └─────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Key advantages over BitChat
//!
//! | Feature | BitChat | flux-bluetooth |
//! |---------|---------|----------------|
//! | Crypto | Classic ECDH | PQ-hybrid (Kyber + X25519) |
//! | Signatures | Ed25519 | SQIsign (177B, PQ) |
//! | Mesh size | ~50 nodes | Unlimited (libp2p DHT) |
//! | Routing | Flood | libp2p Kademlia + gossipsub |
//! | Off-grid | BLE only | BLE + WiFi + Tor + WireGuard |
//! | Messages | Plain MP | BLAKE3 content-addressed |
//! | Replay | None | Verifiable timestamp chain |
//! | Identity | Random ID | PQ keypair (SQIsign) |
//!
//! ## Usage
//!
//! ```rust,ignore
//! use flux_bluetooth::{BluetoothNode, Config};
//!
//! #[tokio::main]
//! async fn main() {
//!     let node = BluetoothNode::new(Config::default()).await.unwrap();
//!     node.start().await.unwrap();
//!     node.send_message("hello mesh!".into()).await.unwrap();
//! }
//! ```

pub mod mesh;
pub mod p2p_bridge;
pub mod chat;
pub mod crypto;
pub mod discovery;

use std::sync::Arc;
use tokio::sync::{Mutex, broadcast};
use anyhow::Result;

/// Re-export key types.
pub use mesh::{MeshConfig, BleMessage, BlePeer, BleAddress};
pub use p2p_bridge::{P2pBridge, BridgeEvent};
pub use chat::{ChatProtocol, ChatMessage, ChatRoom};
pub use crypto::{BluetoothCrypto, PQIdentity};

/// Configuration for a flux-bluetooth node.
#[derive(Debug, Clone)]
pub struct Config {
    /// BLE device name (advertised to peers).
    pub device_name: String,
    /// BLE service UUID for the flux mesh.
    pub service_uuid: uuid::Uuid,
    /// Enable P2P bridge (requires flux-p2p).
    pub enable_p2p_bridge: bool,
    /// Chat message history limit.
    pub history_limit: usize,
    /// Auto-join rooms on discovery.
    pub auto_join_rooms: Vec<String>,
    /// Post-quantum identity seed (32 bytes).
    pub pq_seed: Option<[u8; 32]>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            device_name: format!("flux-bt-{}", hex::encode(&[rand_helper()])),
            service_uuid: uuid::Uuid::from_bytes([
                0x6f, 0x6c, 0x75, 0x78, 0x2d, 0x62, 0x74, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
            ]),
            enable_p2p_bridge: true,
            history_limit: 1000,
            auto_join_rooms: vec!["flux-general".into()],
            pq_seed: None,
        }
    }
}

fn rand_helper() -> u8 {
    use std::time::{SystemTime, UNIX_EPOCH};
    (SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() % 256) as u8
}

/// The main Bluetooth node — owns the BLE mesh + P2P bridge + chat protocol.
pub struct BluetoothNode {
    config: Config,
    mesh: Arc<Mutex<mesh::BleMesh>>,
    bridge: Option<Arc<Mutex<p2p_bridge::P2pBridge>>>,
    chat: Arc<Mutex<chat::ChatProtocol>>,
    crypto: Arc<BluetoothCrypto>,
    /// Incoming message bus (subscribers get all mesh chat messages).
    message_rx: broadcast::Receiver<ChatMessage>,
}

impl BluetoothNode {
    /// Create a new Bluetooth node with the given config.
    ///
    /// Initializes PQ identity, BLE mesh, P2P bridge, and chat protocol.
    /// Does NOT start advertising or scanning — call [`start`] for that.
    pub async fn new(config: Config) -> Result<Self> {
        let crypto = Arc::new(BluetoothCrypto::new(config.pq_seed)?);
        let identity = crypto.identity().clone();

        let mesh_config = MeshConfig {
            device_name: config.device_name.clone(),
            service_uuid: config.service_uuid,
        };

        let mesh = Arc::new(Mutex::new(mesh::BleMesh::new(mesh_config, identity.clone())));

        let bridge = if config.enable_p2p_bridge {
            Some(Arc::new(Mutex::new(p2p_bridge::P2pBridge::new(identity.clone()))))
        } else {
            None
        };

        let (msg_tx, msg_rx) = broadcast::channel(1024);
        let chat = Arc::new(Mutex::new(chat::ChatProtocol::new(config.history_limit, msg_tx)));

        Ok(Self {
            config,
            mesh,
            bridge,
            chat,
            crypto,
            message_rx: msg_rx,
        })
    }

    /// Start the node: begin BLE advertising/scanning, connect P2P bridge.
    pub async fn start(&self) -> Result<()> {
        let mut mesh = self.mesh.lock().await;
        mesh.start().await?;
        tracing::info!("📡 flux-bluetooth started: {}", mesh.local_address());

        if let Some(bridge) = &self.bridge {
            let mut b = bridge.lock().await;
            b.start().await?;
            tracing::info!("🔗 P2P bridge active");
        }

        Ok(())
    }

    /// Send a chat message to a room. Returns the message hash.
    pub async fn send_message(&self, room: String, text: String) -> Result<[u8; 32]> {
        let identity = self.crypto.identity();
        let mut chat = self.chat.lock().await;
        let msg = chat.create_message(&room, &text, identity).await?;

        // Wrap in SignedMessage with Crypto SignedPayload
        let msg_bytes = serde_json::to_vec(&msg)?;
        let signature = self.crypto.sign_message(&msg_bytes).await?;
        let signed_msg = chat::SignedMessage {
            message: msg.clone(),
            signature,
        };

        // Broadcast over BLE mesh
        let mut mesh = self.mesh.lock().await;
        mesh.broadcast(ble_message_from_chat(&signed_msg)).await?;

        // Also bridge to P2P if active
        if let Some(bridge) = &self.bridge {
            let b = bridge.lock().await;
            b.publish_message(&signed_msg.signature).await?;
        }

        Ok(msg.hash)
    }

    /// Receive the next message (blocks until one arrives).
    pub async fn recv_message(&mut self) -> Option<ChatMessage> {
        self.message_rx.recv().await.ok()
    }

    /// Get the node's PQ identity (public key).
    pub fn identity(&self) -> &PQIdentity {
        self.crypto.identity()
    }

    /// List known peers from the BLE mesh.
    pub async fn peers(&self) -> Vec<BlePeer> {
        let mesh = self.mesh.lock().await;
        mesh.peers().to_vec()
    }

    /// Shutdown the node cleanly.
    pub async fn shutdown(&self) -> Result<()> {
        let mut mesh = self.mesh.lock().await;
        mesh.stop().await?;
        if let Some(bridge) = &self.bridge {
            let mut b = bridge.lock().await;
            b.stop().await?;
        }
        Ok(())
    }
}

fn ble_message_from_chat(msg: &chat::SignedMessage) -> mesh::BleMessage {
    mesh::BleMessage {
        data: serde_json::to_vec(msg).unwrap_or_default(),
        kind: mesh::MessageKind::Chat,
    }
}
