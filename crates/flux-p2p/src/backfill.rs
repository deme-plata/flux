//! backfill — content-addressed block backfill over the flux-p2p gossip mesh.
//!
//! flux-p2p is gossip-only: a node gets blocks produced AFTER it joins, but can't
//! backfill history. This module closes that with `flux-sync` (flux-aether under it):
//! every block is content-addressed (BLAKE3, K-of-N, verify-on-retrieve). Nodes
//! advertise a [`SyncManifest`], compute what they're MISSING, and fetch exactly
//! those blocks by `content_root` — a tampered blob is rejected on import.
//!
//! This is the **reusable** version of what `sigil-top` first wired inline, so any
//! flux-p2p consumer (sigil-node, the supercluster, future chains) gets it for free.
//! It is transport-agnostic: the caller pumps [`BackfillMsg`] in/out of a
//! `NetworkManager` publish/subscribe loop.
//!
//! ## Wiring sketch
//! ```ignore
//! let mut bf = Backfill::open("/var/lib/mychain/backfill", "mychain-g0")?;
//! // when you accept a block locally:
//! bf.record(height, &block_bytes)?;
//! // every few seconds, advertise:
//! net.publish(topic, bf.advertise().to_bytes());
//! // on an inbound gossip message:
//! if let Some(msg) = BackfillMsg::from_bytes(&data) {
//!     for out in bf.react(&msg) { net.publish(topic, out.to_bytes()); }
//!     if let BackfillMsg::Blob { root, blob_hex } = &msg {
//!         if let Some(bytes) = bf.on_blob(root, blob_hex) { apply_block(&bytes); }
//!     }
//! }
//! ```

use flux_sync::{missing, BlockStore, SyncManifest};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Recommended gossipsub topic suffix for backfill control traffic
/// (e.g. `/sigil/g0/backfill`). The caller owns the full topic string.
pub const BACKFILL_TOPIC_SUFFIX: &str = "/backfill";

/// The on-wire backfill protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "bf")]
pub enum BackfillMsg {
    /// "Here is the content-addressed history I hold."
    Manifest(SyncManifest),
    /// "Send me the block with this content_root."
    Want { root: String },
    /// "Here is that block's verifiable blob."
    Blob { root: String, blob_hex: String },
}

impl BackfillMsg {
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        serde_json::from_slice(b).ok()
    }
}

/// Per-node backfill driver. Holds a content-addressed [`BlockStore`] + the local
/// [`SyncManifest`]. Pure logic — no networking — so it's trivially testable.
pub struct Backfill {
    store: BlockStore,
    manifest: SyncManifest,
    /// Cap Wants emitted per peer-manifest (bound a backfill burst).
    pub max_want_per_round: usize,
    /// Cap refs advertised (bound message size).
    pub max_advertise_refs: usize,
}

impl Backfill {
    pub fn open(store_dir: impl AsRef<Path>, network_id: impl Into<String>) -> std::io::Result<Self> {
        Ok(Self {
            store: BlockStore::open(store_dir)?,
            manifest: SyncManifest::new(network_id),
            max_want_per_round: 50,
            max_advertise_refs: 200,
        })
    }

    /// Record a block we hold: content-address it + add to our manifest. Returns root.
    pub fn record(&mut self, height: u64, bytes: &[u8]) -> std::io::Result<String> {
        let root = self.store.ingest(bytes)?;
        self.manifest.add(height, root.clone());
        Ok(root)
    }

    /// Our advertisement, capped to the most recent refs.
    pub fn advertise(&self) -> BackfillMsg {
        let mut m = self.manifest.clone();
        if m.refs.len() > self.max_advertise_refs {
            m.refs.sort_by_key(|r| r.height);
            let cut = m.refs.len() - self.max_advertise_refs;
            m.refs = m.refs.split_off(cut);
        }
        BackfillMsg::Manifest(m)
    }

    /// React to an inbound message, returning messages to publish in response:
    /// a `Manifest` → the `Want`s for what we lack; a `Want` → the `Blob` if we hold
    /// it; a `Blob` → nothing (apply via [`on_blob`]).
    pub fn react(&self, msg: &BackfillMsg) -> Vec<BackfillMsg> {
        match msg {
            BackfillMsg::Manifest(remote) => missing(&self.manifest, remote)
                .into_iter()
                .take(self.max_want_per_round)
                .map(|r| BackfillMsg::Want { root: r.content_root })
                .collect(),
            BackfillMsg::Want { root } => self
                .store
                .blob(root)
                .map(|blob| BackfillMsg::Blob { root: root.clone(), blob_hex: hex::encode(blob) })
                .into_iter()
                .collect(),
            BackfillMsg::Blob { .. } => Vec::new(),
        }
    }

    /// Verify + import a Blob, returning the verified block bytes for the caller to
    /// apply to its chain (None if absent or tampered — anti-poison). After applying,
    /// the caller should `record(height, &bytes)` so this node can serve it too.
    pub fn on_blob(&self, root: &str, blob_hex: &str) -> Option<Vec<u8>> {
        let blob = hex::decode(blob_hex).ok()?;
        if self.store.import_blob(root, &blob).ok()? {
            self.store.retrieve(root)
        } else {
            None
        }
    }

    pub fn manifest(&self) -> &SyncManifest {
        &self.manifest
    }
    pub fn tip(&self) -> u64 {
        self.manifest.tip
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("flux-p2p-bf-{}-{}", std::process::id(), tag))
    }

    #[test]
    fn msg_roundtrips() {
        let m = BackfillMsg::Want { root: "abc".into() };
        let b = m.to_bytes();
        assert!(matches!(BackfillMsg::from_bytes(&b), Some(BackfillMsg::Want { .. })));
    }

    #[test]
    fn two_node_backfill_over_protocol() {
        let (da, db) = (tmp("A"), tmp("B"));
        let _ = std::fs::remove_dir_all(&da);
        let _ = std::fs::remove_dir_all(&db);
        let mut a = Backfill::open(&da, "g0").unwrap();
        let mut b = Backfill::open(&db, "g0").unwrap();

        // A holds blocks 1..=4, B holds 1..=2.
        let blocks: Vec<Vec<u8>> = (1..=4u64).map(|h| format!("blk-{h}").into_bytes()).collect();
        for (i, blk) in blocks.iter().enumerate() {
            a.record(i as u64 + 1, blk).unwrap();
        }
        for i in 0..2 {
            b.record(i as u64 + 1, &blocks[i]).unwrap();
        }

        // B sees A's advertisement → emits Wants for the 2 it lacks.
        let adv = a.advertise();
        let wants = b.react(&adv);
        assert_eq!(wants.len(), 2);

        // A serves each Want; B imports (verified) + applies + records.
        let mut applied = 0;
        for w in &wants {
            if let BackfillMsg::Want { root } = w {
                let served = a.react(&BackfillMsg::Want { root: root.clone() });
                assert_eq!(served.len(), 1);
                if let BackfillMsg::Blob { root, blob_hex } = &served[0] {
                    let bytes = b.on_blob(root, blob_hex).expect("verified blob");
                    assert!(bytes.starts_with(b"blk-"));
                    // caller parses height; here it's encoded in the bytes
                    let h: u64 = String::from_utf8_lossy(&bytes)
                        .trim_start_matches("blk-")
                        .parse()
                        .unwrap();
                    b.record(h, &bytes).unwrap();
                    applied += 1;
                }
            }
        }
        assert_eq!(applied, 2);
        assert_eq!(b.tip(), 4);
        // B is now fully synced: no more Wants.
        assert!(b.react(&a.advertise()).is_empty());

        // Anti-poison: a garbage blob under a real root is rejected.
        let real_root = if let BackfillMsg::Manifest(m) = a.advertise() {
            m.refs[0].content_root.clone()
        } else {
            unreachable!()
        };
        assert!(b.on_blob(&real_root, &hex::encode(b"garbage")).is_none());

        let _ = std::fs::remove_dir_all(&da);
        let _ = std::fs::remove_dir_all(&db);
    }
}
