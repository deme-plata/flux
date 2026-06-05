//! sync — propagate revisions over **flux-p2p** (gossipsub), not git push/pull.
//!
//! Because objects are content-addressed + immutable, propagation is just object exchange:
//!   1. a node **announces** its HEAD + the revision's object `closure` (hashes it transitively needs)
//!   2. a peer that's missing some of those objects publishes a **want** with the missing hashes
//!   3. any holder replies with **have** {hash, bytes}; the receiver verifies `hash(bytes)==hash`
//!      before storing (a peer can never inject a mismatched object), then `checkout`s when complete.
//!
//! No branches, no merge, no "diverged" — a peer either holds object X or asks for it. This module is
//! the pure protocol (message types + the verify/missing helpers); the event loop lives in the
//! `flux-rev-sync` bin driving a real `flux_p2p::NetworkManager`.

use serde::{Deserialize, Serialize};
use crate::{verify_object, Store};

pub const TOPIC: &str = "/flux-rev/sync/v1";

/// One wire message on the sync topic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum Msg {
    /// "my HEAD is `head` and it needs these objects" (the revision's transitive closure).
    Announce { head: String, ver: String, closure: Vec<String> },
    /// "send me these objects I'm missing".
    Want { hashes: Vec<String> },
    /// "here is object `hash`" — hex-encoded bytes; the receiver MUST verify before trusting.
    Have { hash: String, hex: String },
}

impl Msg {
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("serialize msg")
    }
    pub fn decode(b: &[u8]) -> Option<Msg> {
        serde_json::from_slice(b).ok()
    }
}

/// Which of `hashes` the store does NOT yet hold — exactly what to `Want`.
pub fn missing(store: &Store, hashes: &[String]) -> Vec<String> {
    hashes.iter().filter(|h| !store.has(h)).cloned().collect()
}

/// Store a received object ONLY if its bytes hash to the claimed address. Returns true if accepted.
/// This is the integrity gate: content-addressing means a tampered object can't keep its name.
pub fn store_verified(store: &Store, hash: &str, bytes: &[u8]) -> bool {
    if !verify_object(hash, bytes) {
        return false;
    }
    store.put_at(hash, bytes).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash_bytes;
    use std::path::PathBuf;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("flux-rev-sync-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn msg_roundtrips() {
        let m = Msg::Announce { head: "abc".into(), ver: "0.18.0".into(), closure: vec!["x".into(), "y".into()] };
        assert_eq!(Msg::decode(&m.encode()).unwrap(), m);
        let w = Msg::Want { hashes: vec!["x".into()] };
        assert_eq!(Msg::decode(&w.encode()).unwrap(), w);
    }

    #[test]
    fn missing_reports_only_absent_objects() {
        let work = tmp("missing");
        let store = Store::open(&work).unwrap();
        let h = store.put(b"present").unwrap();
        let want = missing(&store, &[h.clone(), "deadbeef".into()]);
        assert_eq!(want, vec!["deadbeef".to_string()], "only the absent hash is wanted");
    }

    #[test]
    fn store_verified_rejects_tampered_bytes() {
        let work = tmp("verify");
        let store = Store::open(&work).unwrap();
        let good = b"trusted payload";
        let h = hash_bytes(good);
        assert!(store_verified(&store, &h, good), "matching bytes accepted");
        assert!(store.has(&h));
        // a peer claims hash `h` but ships different bytes → rejected, never stored under h
        let work2 = tmp("verify2");
        let store2 = Store::open(&work2).unwrap();
        assert!(!store_verified(&store2, &h, b"EVIL swapped payload"), "mismatched bytes rejected");
        assert!(!store2.has(&h), "tampered object never lands in the store");
    }
}
