//! flux-p2p frame relay.
//!
//! **Why this module exists at all.** The agent that wants to *look* at a frame
//! usually is not running on the machine with the camera. Epsilon is a
//! headless datacenter host with no `/dev/video*`; the camera is on the
//! operator's desk. So a frame has to cross the mesh, and the moment it does,
//! two questions stop being theoretical:
//!
//! 1. *Is this the frame the capturing node actually saw?* — answered by
//!    content-addressing: the announce carries a BLAKE3 that must re-derive
//!    over the delivered bytes.
//! 2. *Is this peer worth listening to?* — answered by SAP: delivery latency
//!    feeds the peer's latency component, and a hash mismatch is recorded as an
//!    equivocation, which is the strongest negative signal the table has.
//!
//! The relay deliberately does **not** open sockets. It is the pure
//! announce/verify/score layer; binding a transport is the caller's job
//! (`flux_p2p::NetworkManager`), which keeps this testable without a network.

use crate::frame::{Frame, FrameFormat};
use flux_p2p::sap::{PeerId, SAPComponents, ScoreTable};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// What a capturing node broadcasts when it has a frame available.
///
/// Note it carries the hash and the metadata but **not** the bytes — a mesh
/// should be able to gossip "I have a frame" cheaply, and only the agent that
/// actually wants to look pays for the payload.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FrameAnnounce {
    /// Peer that captured it.
    pub origin: String,
    /// BLAKE3 hex of the frame bytes.
    pub hash: String,
    pub width: u32,
    pub height: u32,
    pub format: FrameFormat,
    pub bytes: u64,
    pub captured_at_ms: u64,
}

impl FrameAnnounce {
    pub fn for_frame(origin: impl Into<String>, frame: &Frame) -> Self {
        FrameAnnounce {
            origin: origin.into(),
            hash: frame.hash.clone(),
            width: frame.width,
            height: frame.height,
            format: frame.format,
            bytes: frame.len() as u64,
            captured_at_ms: frame.captured_at_ms,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    pub fn from_json(s: &str) -> Option<Self> {
        serde_json::from_str(s).ok()
    }
}

/// Why a delivered frame was rejected.
#[derive(Debug, PartialEq, Eq)]
pub enum RelayReject {
    /// Delivered bytes did not hash to the announced value.
    HashMismatch { announced: String, actual: String },
    /// The frame's own self-hash does not re-derive (corrupted in transit).
    SelfHashInvalid,
    /// Announce metadata contradicts the payload.
    MetadataMismatch(String),
}

/// Per-peer delivery record, the basis for that peer's SAP components.
#[derive(Clone, Copy, Debug, Default)]
pub struct PeerDelivery {
    pub accepted: u64,
    pub rejected: u64,
    /// Sticky: once a peer has provably lied, accuracy stays zero.
    pub equivocated: bool,
}

impl PeerDelivery {
    fn total(&self) -> u64 {
        self.accepted + self.rejected
    }

    /// Share of this peer's deliveries that verified.
    fn uptime(&self) -> f64 {
        if self.total() == 0 {
            0.0
        } else {
            self.accepted as f64 / self.total() as f64
        }
    }
}

/// Verifies inbound frames and scores the peers that deliver them.
pub struct FrameRelay {
    pub table: ScoreTable,
    peers: HashMap<String, PeerDelivery>,
    accepted: u64,
    rejected: u64,
}

impl Default for FrameRelay {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameRelay {
    pub fn new() -> Self {
        FrameRelay {
            table: ScoreTable::new(),
            peers: HashMap::new(),
            accepted: 0,
            rejected: 0,
        }
    }

    /// Ensure the peer has a row in the SAP table.
    ///
    /// This is load-bearing, not defensive clutter: `ScoreTable`'s
    /// `update_latency`, `record_participation` and `mark_equivocation` all
    /// guard with `if let Some(..)` and therefore **silently do nothing** for a
    /// peer that has never been through `update()`. Without seeding the row
    /// first, a misbehaving peer's equivocation would be dropped on the floor.
    fn ensure_peer(&mut self, peer: &PeerId) {
        if self.table.get(peer).is_none() {
            self.table.update(
                peer.clone(),
                SAPComponents {
                    contribution: 0.0,
                    latency: 0.0,
                    stake: 0.0,
                    accuracy: 1.0, // innocent until it equivocates
                    uptime: 0.0,
                },
            );
        }
    }

    /// Recompute a peer's components from its delivery record.
    fn resync_components(&mut self, peer: &PeerId) {
        let record = self.peers.get(&peer.0).copied().unwrap_or_default();
        // Preserve whatever stake the operator has assigned; the relay does not
        // know about stake and must not clobber it.
        let stake = self.table.get_full(peer).map(|s| s.components.stake).unwrap_or(0.0);
        let latency = self.table.get_full(peer).map(|s| s.components.latency).unwrap_or(0.0);
        self.table.update(
            peer.clone(),
            SAPComponents {
                contribution: if record.accepted > 0 { 1.0 } else { 0.0 },
                latency,
                stake,
                accuracy: if record.equivocated { 0.0 } else { 1.0 },
                uptime: record.uptime(),
            },
        );
    }

    /// Record a rejection and burn the peer's accuracy.
    fn penalise(&mut self, peer: &PeerId) {
        self.rejected += 1;
        let entry = self.peers.entry(peer.0.clone()).or_default();
        entry.rejected += 1;
        entry.equivocated = true;
        self.ensure_peer(peer);
        self.resync_components(peer);
        self.table.mark_equivocation(peer);
    }

    /// Delivery record for one peer.
    pub fn peer_record(&self, origin: &str) -> PeerDelivery {
        self.peers.get(origin).copied().unwrap_or_default()
    }

    pub fn accepted(&self) -> u64 {
        self.accepted
    }

    pub fn rejected(&self) -> u64 {
        self.rejected
    }

    /// Accept a frame delivered against a prior announce.
    ///
    /// `delivery_ms` is the observed time from announce to payload — the real
    /// measurement that feeds the peer's SAP latency component.
    pub fn accept(
        &mut self,
        announce: &FrameAnnounce,
        frame: &Frame,
        delivery_ms: f64,
    ) -> Result<(), RelayReject> {
        let peer = PeerId(announce.origin.clone());

        // 1. The frame must be internally consistent.
        if !frame.verify() {
            self.penalise(&peer);
            return Err(RelayReject::SelfHashInvalid);
        }

        // 2. It must be the frame that was announced. A peer announcing one
        //    hash and delivering another is equivocating, full stop.
        if frame.hash != announce.hash {
            self.penalise(&peer);
            return Err(RelayReject::HashMismatch {
                announced: announce.hash.clone(),
                actual: frame.hash.clone(),
            });
        }

        // 3. Metadata must not lie about the payload either.
        if announce.bytes != frame.len() as u64 {
            let detail =
                format!("announced {} bytes, delivered {}", announce.bytes, frame.len());
            self.penalise(&peer);
            return Err(RelayReject::MetadataMismatch(detail));
        }
        if announce.format != frame.format {
            let detail =
                format!("announced {:?}, delivered {:?}", announce.format, frame.format);
            self.penalise(&peer);
            return Err(RelayReject::MetadataMismatch(detail));
        }

        // Good delivery. Seed the row first — the ScoreTable mutators below are
        // all no-ops on a peer that has never been through `update()`.
        self.accepted += 1;
        self.peers.entry(peer.0.clone()).or_default().accepted += 1;
        self.ensure_peer(&peer);
        self.resync_components(&peer);
        self.table.update_latency(&peer, delivery_ms, delivery_ms);
        self.table.record_participation(&peer);
        Ok(())
    }

    /// SAP total for a peer, if it has one yet.
    pub fn peer_score(&self, origin: &str) -> Option<f64> {
        self.table.get(&PeerId(origin.to_string()))
    }

    /// Best frame providers, most trusted first.
    pub fn best_providers(&self, n: usize) -> Vec<(String, f64)> {
        self.table
            .top_peers(n)
            .into_iter()
            .map(|s| (s.peer.0.clone(), s.total))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{FrameSource, SyntheticSource};

    fn a_frame() -> Frame {
        SyntheticSource::new(16, 16).capture().unwrap()
    }

    #[test]
    fn announce_round_trips_through_json() {
        let f = a_frame();
        let ann = FrameAnnounce::for_frame("epsilon", &f);
        let decoded = FrameAnnounce::from_json(&ann.to_json()).expect("should decode");
        assert_eq!(ann, decoded);
        assert_eq!(decoded.hash, f.hash);
    }

    #[test]
    fn honest_delivery_is_accepted_and_scored() {
        let f = a_frame();
        let ann = FrameAnnounce::for_frame("delta", &f);
        let mut relay = FrameRelay::new();
        assert!(relay.accept(&ann, &f, 12.0).is_ok());
        assert_eq!(relay.accepted(), 1);
        assert_eq!(relay.rejected(), 0);
        assert!(relay.peer_score("delta").is_some(), "a delivering peer must get a SAP entry");
    }

    #[test]
    fn hash_mismatch_is_rejected_as_equivocation() {
        let f = a_frame();
        let mut ann = FrameAnnounce::for_frame("liar", &f);
        ann.hash = "0".repeat(64); // announce something we did not deliver
        let mut relay = FrameRelay::new();
        let err = relay.accept(&ann, &f, 5.0).unwrap_err();
        assert!(matches!(err, RelayReject::HashMismatch { .. }));
        assert_eq!(relay.rejected(), 1);
        assert_eq!(relay.accepted(), 0);
    }

    #[test]
    fn corrupted_payload_is_rejected() {
        let mut f = a_frame();
        let ann = FrameAnnounce::for_frame("noisy", &f);
        f.data[10] ^= 0xFF; // flip a bit in transit; self-hash no longer derives
        let mut relay = FrameRelay::new();
        assert_eq!(relay.accept(&ann, &f, 5.0).unwrap_err(), RelayReject::SelfHashInvalid);
    }

    #[test]
    fn byte_count_lies_are_caught() {
        let f = a_frame();
        let mut ann = FrameAnnounce::for_frame("fibber", &f);
        ann.bytes += 1;
        let mut relay = FrameRelay::new();
        assert!(matches!(
            relay.accept(&ann, &f, 5.0).unwrap_err(),
            RelayReject::MetadataMismatch(_)
        ));
    }

    #[test]
    fn faster_peers_rank_above_slower_ones() {
        let f = a_frame();
        let mut relay = FrameRelay::new();
        relay.accept(&FrameAnnounce::for_frame("quick", &f), &f, 3.0).unwrap();
        relay.accept(&FrameAnnounce::for_frame("sluggish", &f), &f, 800.0).unwrap();
        let quick = relay.peer_score("quick").unwrap();
        let slow = relay.peer_score("sluggish").unwrap();
        assert!(quick > slow, "latency must move the ranking ({quick} vs {slow})");
        assert_eq!(relay.best_providers(1)[0].0, "quick");
    }
}
