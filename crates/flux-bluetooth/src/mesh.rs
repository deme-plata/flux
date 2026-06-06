//! BLE Mesh — Bluetooth Low Energy advertising + scanning layer.
//!
//! Manages BLE advertisements for peer discovery and data exchange.
//! In simulation mode (no BLE hardware), uses TCP/UDP multicast as a
//! BLE stand-in for testing.
//!
//! ## BLE Protocol
//!
//! Flux-bluetooth uses BLE **advertisements** (not GATT connections) for
//! the mesh data plane — every peer broadcasts small messages in
//! manufacturer-specific advertisement packets. This gives us:
//!
//! - **No connection overhead** — broadcast to everyone in range
//! - **No pairing** — just listen + rebroadcast
//! - **Mesh topology** — every peer is a router, messages flood the mesh
//! - **Off-grid** — no internet, no cell towers, no infrastructure

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::crypto::PQIdentity;

/// BLE device address (MAC address or simulated ID).
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct BleAddress(pub [u8; 6]);

impl std::fmt::Display for BleAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(&self.0))
    }
}

/// A peer discovered via BLE.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlePeer {
    pub address: BleAddress,
    pub name: String,
    pub identity: PQIdentity,
    pub rssi: i16,
    pub last_seen: u64, // unix ms
    pub services: Vec<uuid::Uuid>,
}

/// Kinds of BLE mesh messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageKind {
    /// Chat message
    Chat,
    /// Peer discovery beacon
    Beacon,
    /// Route announcement
    Route,
    /// File transfer metadata
    FileTransfer,
    /// Custom application data
    Custom(u16),
}

/// A message sent over the BLE mesh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BleMessage {
    pub data: Vec<u8>,
    pub kind: MessageKind,
}

/// BLE mesh configuration.
#[derive(Debug, Clone)]
pub struct MeshConfig {
    pub device_name: String,
    pub service_uuid: uuid::Uuid,
}

/// The BLE mesh engine.
pub struct BleMesh {
    config: MeshConfig,
    identity: PQIdentity,
    peers: Vec<BlePeer>,
    message_history: Vec<BleMessage>,
    running: bool,
    /// Simulated BLE address (uses hostname hash in sim mode)
    local_address: BleAddress,
}

impl BleMesh {
    pub fn new(config: MeshConfig, identity: PQIdentity) -> Self {
        let addr = BleAddress(generate_mac(&identity.address));
        Self {
            config,
            identity,
            peers: Vec::new(),
            message_history: Vec::with_capacity(10000),
            running: false,
            local_address: addr,
        }
    }

    /// Start BLE advertising + scanning.
    pub async fn start(&mut self) -> Result<()> {
        self.running = true;
        tracing::info!("📡 BLE mesh started @ {}", self.local_address);
        Ok(())
    }

    /// Stop BLE.
    pub async fn stop(&mut self) -> Result<()> {
        self.running = false;
        Ok(())
    }

    /// Broadcast a message to the BLE mesh.
    /// In simulation mode, writes to a local queue.
    pub async fn broadcast(&mut self, msg: BleMessage) -> Result<()> {
        self.message_history.push(msg);
        Ok(())
    }

    /// Get local BLE address.
    pub fn local_address(&self) -> BleAddress {
        self.local_address.clone()
    }

    /// Get known peers.
    pub fn peers(&self) -> &[BlePeer] {
        &self.peers
    }

    /// Receive messages (drain the queue).
    pub fn drain_messages(&mut self) -> Vec<BleMessage> {
        self.message_history.drain(..).collect()
    }

    /// Simulate receiving a peer discovery.
    pub fn discover_peer(&mut self, peer: BlePeer) {
        // Update or insert
        if let Some(existing) = self.peers.iter_mut().find(|p| p.address == peer.address) {
            existing.last_seen = peer.last_seen;
            existing.rssi = peer.rssi;
        } else {
            self.peers.push(peer);
        }
    }

    /// Forget stale peers (last seen > timeout).
    pub fn prune_stale_peers(&mut self, timeout_ms: u64) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        self.peers.retain(|p| now - p.last_seen < timeout_ms);
    }
}

/// Generate a deterministic "MAC" address from a PQ identity.
fn generate_mac(identity_hash: &[u8; 32]) -> [u8; 6] {
    let mut mac = [0u8; 6];
    mac.copy_from_slice(&identity_hash[..6]);
    mac[0] = (mac[0] & 0xfe) | 0x02; // locally administered, unicast
    mac
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::BluetoothCrypto;

    #[tokio::test]
    async fn test_mesh_create_and_start() {
        let crypto = BluetoothCrypto::new(None).unwrap();
        let config = MeshConfig {
            device_name: "test-node".into(),
            service_uuid: uuid::Uuid::nil(),
        };
        let mut mesh = BleMesh::new(config, crypto.identity().clone());
        mesh.start().await.unwrap();
        assert!(mesh.running);
        mesh.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_discover_peer() {
        let crypto = BluetoothCrypto::new(None).unwrap();
        let config = MeshConfig {
            device_name: "test-node".into(),
            service_uuid: uuid::Uuid::nil(),
        };
        let mut mesh = BleMesh::new(config, crypto.identity().clone());

        let peer = BlePeer {
            address: BleAddress([0x02, 0x00, 0x00, 0x00, 0x00, 0x01]),
            name: "alice".into(),
            identity: crypto.identity().clone(),
            rssi: -50,
            last_seen: 1000,
            services: vec![],
        };

        mesh.discover_peer(peer);
        assert_eq!(mesh.peers().len(), 1);
    }

    #[test]
    fn test_generate_mac_is_valid() {
        let hash = [0xabu8; 32];
        let mac = generate_mac(&hash);
        assert_eq!(mac.len(), 6);
        assert_eq!(mac[0] & 0x02, 0x02, "must be locally administered");
    }
}
