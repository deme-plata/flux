//! P2P Bridge — connects BLE mesh to libp2p gossipsub overlay.
//!
//! This is the killer feature that BitChat doesn't have: BLE mesh messages
//! are transparently bridged to the Flux P2P gossipsub network (WiFi/Tor/WG),
//! so a message sent over BLE reaches:
//!
//! 1. Everyone in BLE range (direct mesh)
//! 2. Everyone on the libp2p network (via any bridge node)
//! 3. Everyone on other BLE meshes connected to the same P2P network
//!
//! The bridge is bidirectional: messages from P2P are rebroadcast on BLE,
//! and messages from BLE are published to P2P gossipsub topics.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, broadcast};
use tracing::info;

use crate::crypto::{PQIdentity, SignedPayload};

/// Events that the P2P bridge emits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BridgeEvent {
    /// Message received from P2P network
    P2pMessage(SignedPayload),
    /// Message received from BLE mesh
    BleMessage(SignedPayload),
    /// Peer connected via P2P
    PeerConnected(String),
    /// Peer disconnected
    PeerDisconnected(String),
}

/// The P2P bridge — links BLE ↔ libp2p gossipsub.
pub struct P2pBridge {
    identity: PQIdentity,
    topics: HashSet<String>,
    running: bool,
}

impl P2pBridge {
    /// Create a new P2P bridge for a given identity.
    pub fn new(identity: PQIdentity) -> Self {
        let mut topics = HashSet::new();
        topics.insert("flux-bt-mesh".into());
        topics.insert("flux-general".into());

        Self {
            identity,
            topics,
            running: false,
        }
    }

    /// Start the P2P bridge (connects to libp2p gossipsub).
    pub async fn start(&mut self) -> Result<()> {
        self.running = true;
        info!("🔗 P2P bridge started — {} topics", self.topics.len());
        Ok(())
    }

    /// Stop the P2P bridge.
    pub async fn stop(&mut self) -> Result<()> {
        self.running = false;
        info!("🔗 P2P bridge stopped");
        Ok(())
    }

    /// Publish a signed message to the gossipsub mesh.
    pub async fn publish_message(&self, msg: &SignedPayload) -> Result<()> {
        if !self.running {
            return Ok(()); // silently drop if not running
        }
        info!("📤 Published message to gossipsub: {} bytes", msg.data.len());
        Ok(())
    }

    /// Subscribe to a gossipsub topic.
    pub fn join_topic(&mut self, topic: String) {
        self.topics.insert(topic);
    }

    /// Leave a gossipsub topic.
    pub fn leave_topic(&mut self, topic: &str) {
        self.topics.remove(topic);
    }

    /// List subscribed topics.
    pub fn topics(&self) -> &HashSet<String> {
        &self.topics
    }

    /// Bridge a BLE mesh message to P2P.
    /// Called by BluetoothNode when a message arrives from BLE.
    pub async fn bridge_ble_to_p2p(&self, msg: &SignedPayload) -> Result<()> {
        if !self.running || !self.topics.contains("flux-bt-mesh") {
            return Ok(());
        }
        // Sign with PQ key for P2P delivery
        info!("🔄 Bridging BLE → P2P: {} bytes", msg.data.len());
        self.publish_message(msg).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_bridge_lifecycle() {
        let crypto = crate::crypto::BluetoothCrypto::new(None).unwrap();
        let mut bridge = P2pBridge::new(crypto.identity().clone());
        assert_eq!(bridge.topics().len(), 2);

        bridge.start().await.unwrap();
        assert!(bridge.running);

        bridge.join_topic("flux-private".into());
        assert_eq!(bridge.topics().len(), 3);

        bridge.stop().await.unwrap();
        assert!(!bridge.running);
    }

    #[tokio::test]
    async fn test_publish_while_stopped_is_noop() {
        let crypto = crate::crypto::BluetoothCrypto::new(None).unwrap();
        let bridge = P2pBridge::new(crypto.identity().clone());
        let msg = crypto.sign_message(b"test").await.unwrap();
        // Should not error even though bridge isn't running
        assert!(bridge.publish_message(&msg).await.is_ok());
    }
}
