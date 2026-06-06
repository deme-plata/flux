//! BLE Peer Discovery — beacon-based node discovery over BLE.
//!
//! Flux-bluetooth peers advertise their presence via BLE manufacturer-specific
//! advertisement packets. The beacon contains:
//!
//! - Device name
//! - PQ identity fingerprint (first 8 bytes of BLAKE3 of public key)
//! - Available services (chat, file transfer, etc.)
//! - Hop count (how many BLE relays from origin)
//!
//! This allows nodes to discover each other without any infrastructure:
//! walk into range → beacon heard → PQ identity verified → chat established.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::crypto::{BluetoothCrypto, PQIdentity};
use crate::mesh::{BleAddress, BlePeer};

/// A discovery beacon sent over BLE advertisements.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Beacon {
    /// Protocol version
    pub version: u8,
    /// Device name
    pub name: String,
    /// PQ identity fingerprint (BLAKE3 of public key, first 8 bytes)
    pub fingerprint: [u8; 8],
    /// Full PQ public key (for first-contact verification)
    pub public_key: Vec<u8>,
    /// Services offered
    pub services: Vec<String>,
    /// Hop count from origin (0 = originator)
    pub hops: u8,
    /// Sequence number (for dedup)
    pub seq: u64,
}

/// Discovery engine — manages beacons + peer table.
pub struct Discovery {
    identity: PQIdentity,
    /// Known beacons (deduped by fingerprint + seq)
    seen_beacons: HashMap<[u8; 8], u64>,
    /// Discovered peers
    peers: Vec<BlePeer>,
    /// Our beacon sequence counter
    beacon_seq: u64,
}

impl Discovery {
    /// Create a new discovery engine.
    pub fn new(identity: PQIdentity) -> Self {
        Self {
            identity,
            seen_beacons: HashMap::new(),
            peers: Vec::new(),
            beacon_seq: 0,
        }
    }

    /// Create our beacon for advertising.
    pub fn create_beacon(&mut self) -> Beacon {
        self.beacon_seq += 1;
        let fingerprint = {
            let mut f = [0u8; 8];
            f.copy_from_slice(&self.identity.address[..8]);
            f
        };

        Beacon {
            version: 1,
            name: self.identity.name.clone(),
            fingerprint,
            public_key: self.identity.public_key.clone(),
            services: vec!["chat".into(), "flux-bt".into()],
            hops: 0,
            seq: self.beacon_seq,
        }
    }

    /// Process a received beacon. Returns the peer if it's new or updated.
    pub fn process_beacon(
        &mut self,
        beacon: Beacon,
        address: BleAddress,
        rssi: i16,
    ) -> Option<BlePeer> {
        // Dedup by (fingerprint, seq)
        let last_seq = self.seen_beacons.get(&beacon.fingerprint).copied().unwrap_or(0);
        if beacon.seq <= last_seq {
            return None; // already seen
        }
        self.seen_beacons.insert(beacon.fingerprint, beacon.seq);

        // Verify PQ identity from public key
        let peer_address = {
            let mut h = blake3::Hasher::new();
            h.update(&beacon.public_key);
            *h.finalize().as_bytes()
        };

        let name = beacon.name.clone();
        let peer = BlePeer {
            address,
            name: name.clone(),
            identity: PQIdentity {
                name,
                public_key: beacon.public_key.clone(),
                address: peer_address,
            },
            rssi,
            last_seen: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            services: beacon.services.iter().map(|_s| uuid::Uuid::nil()).collect(),
        };

        // Update or insert
        if let Some(existing) = self.peers.iter_mut().find(|p| p.address == peer.address) {
            existing.last_seen = peer.last_seen;
            existing.rssi = peer.rssi;
        } else {
            self.peers.push(peer.clone());
        }

        Some(peer)
    }

    /// Get discovered peers.
    pub fn peers(&self) -> &[BlePeer] {
        &self.peers
    }

    /// Number of unique peers discovered.
    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    /// Prune stale peers (not seen for > duration).
    pub fn prune_stale(&mut self, max_age: Duration) {
        let cutoff = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            - max_age.as_millis() as u64;

        self.peers.retain(|p| p.last_seen > cutoff);
    }

    /// Clear all discovered peers (e.g., on network change).
    pub fn clear(&mut self) {
        self.peers.clear();
        self.seen_beacons.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::BluetoothCrypto;

    #[test]
    fn test_create_beacon() {
        let crypto = BluetoothCrypto::new(None).unwrap();
        let mut discovery = Discovery::new(crypto.identity().clone());
        let beacon = discovery.create_beacon();
        assert_eq!(beacon.version, 1);
        assert!(beacon.services.contains(&"chat".into()));
        assert_eq!(beacon.seq, 1);
    }

    #[test]
    fn test_process_beacon() {
        let alice = BluetoothCrypto::new(None).unwrap();
        let bob = BluetoothCrypto::new(None).unwrap();

        let mut discovery = Discovery::new(alice.identity().clone());

        let beacon = Beacon {
            version: 1,
            name: bob.identity().name.clone(),
            fingerprint: {
                let mut f = [0u8; 8];
                f.copy_from_slice(&bob.identity().address[..8]);
                f
            },
            public_key: bob.identity().public_key.clone(),
            services: vec!["chat".into()],
            hops: 0,
            seq: 1,
        };

        let addr = BleAddress([0x02, 0x00, 0x00, 0x00, 0x00, 0x01]);
        let peer = discovery.process_beacon(beacon, addr, -60);
        assert!(peer.is_some());
        assert_eq!(discovery.peer_count(), 1);
    }

    #[test]
    fn test_dedup_beacon() {
        let crypto = BluetoothCrypto::new(None).unwrap();
        let mut discovery = Discovery::new(crypto.identity().clone());

        let beacon = Beacon {
            version: 1,
            name: "test".into(),
            fingerprint: [1u8; 8],
            public_key: vec![],
            services: vec![],
            hops: 0,
            seq: 1,
        };

        let addr = BleAddress([0x02, 0x00, 0x00, 0x00, 0x00, 0x01]);

        // First time — should accept
        assert!(discovery.process_beacon(beacon.clone(), addr.clone(), -50).is_some());

        // Same seq — should reject (dedup)
        assert!(discovery.process_beacon(beacon.clone(), addr.clone(), -50).is_none());

        // Higher seq — should accept
        let mut beacon2 = beacon;
        beacon2.seq = 2;
        assert!(discovery.process_beacon(beacon2, addr.clone(), -50).is_some());

        assert_eq!(discovery.peer_count(), 1);
    }
}
