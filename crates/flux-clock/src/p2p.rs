//! The decentralized clock: one hand per peer.
//!
//! A peer holds ONE hand (one coprime period), reads its own time source, and gossips only
//! `t mod x_j` on [`CLOCK_TOPIC`] over flux-p2p (libp2p gossipsub). Nobody publishes the
//! time. A reader that hears `m` hands whose periods multiply past the known bound
//! reconstructs it with the paper's protocol; with one hand more than needed it can also say
//! WHICH peer disagrees ([`crt::reconstruct_bounded`]). This is the paper's optimality result
//! used as an architecture: independent, unentangled hands, the time in the composition.
//!
//! Wire format: JSON (never bincode — an internally-tagged enum broke the SIGIL block wire
//! once, see sigil f4051bd8), with a BLAKE3 digest over the canonical fields. v1 envelopes
//! are NOT signed: a peer can lie, and the redundancy check is what catches it. Signing with
//! the peer's hybrid key is the v2 step.

use crate::crt;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const CLOCK_TOPIC: &str = "/sigil/g2/clock";
pub const CLOCK_ID: &str = "sigil-crt-v1";
/// Recommended live hands: four primes near 10^4 — any TWO multiply past 10^8 (the default
/// height bound) and any THREE to ~10^12, so four peers carry one hand of redundancy and can
/// name a peer that lies with a false-accept probability of ~10^-4.
pub const RECOMMENDED_PERIODS: [u64; 4] = [10_007, 10_009, 10_037, 10_039];
pub const DEFAULT_BOUND: u128 = 100_000_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HandEnvelope {
    pub v: u8,
    pub clock: String,
    pub node: String,
    /// "block" (agreed ticks) or "second" (UT1 seconds since the Unix epoch).
    pub unit: String,
    pub period: u64,
    /// `t̃ mod period`, fractional part allowed.
    pub remainder: f64,
    pub z: u32,
    pub ts_ms: u64,
    pub digest: String,
}

impl HandEnvelope {
    pub fn new(node: &str, unit: &str, period: u64, remainder: f64, z: u32) -> Self {
        let mut e = HandEnvelope { v: 1, clock: CLOCK_ID.into(), node: node.into(), unit: unit.into(), period, remainder: remainder.rem_euclid(period as f64), z: z.max(1), ts_ms: crate::now_ms(), digest: String::new() };
        e.digest = e.digest_of();
        e
    }
    fn canonical(&self) -> String {
        format!("{}|{}|{}|{}|{}|{:.9}|{}|{}", self.v, self.clock, self.node, self.unit, self.period, self.remainder, self.z, self.ts_ms)
    }
    pub fn digest_of(&self) -> String { hex::encode(blake3::hash(self.canonical().as_bytes()).as_bytes()) }
    pub fn verify(&self) -> bool { self.clock == CLOCK_ID && self.period >= 2 && self.remainder >= 0.0 && self.remainder < self.period as f64 && self.digest == self.digest_of() }
    pub fn encode(&self) -> Vec<u8> { serde_json::to_vec(self).unwrap_or_default() }
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let e: HandEnvelope = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if !e.verify() { return Err("envelope failed verification".into()); }
        Ok(e)
    }
}

/// Collects hands by period (one per period — a second peer on the same period replaces the
/// older reading) and reconstructs when the range covers the bound.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HandCollector {
    pub unit: String,
    pub hands: BTreeMap<u64, HandEnvelope>,
    pub rejected: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectorReading {
    pub unit: String,
    pub hands: usize,
    pub peers: Vec<String>,
    pub periods: Vec<u64>,
    pub remainders: Vec<f64>,
    pub range: String,
    pub bound: String,
    pub covers_bound: bool,
    pub result: crt::BoundedReconstruction,
    pub note: String,
}

impl HandCollector {
    pub fn new(unit: &str) -> Self { HandCollector { unit: unit.into(), hands: BTreeMap::new(), rejected: 0 } }

    pub fn ingest(&mut self, e: HandEnvelope) -> Result<(), String> {
        if !e.verify() { self.rejected += 1; return Err("bad envelope".into()); }
        if e.unit != self.unit { self.rejected += 1; return Err(format!("unit {} ≠ {}", e.unit, self.unit)); }
        // a hand whose period shares a factor with one we hold cannot join the composition
        for &p in self.hands.keys() { if p != e.period && crt::gcd(p as u128, e.period as u128) != 1 { self.rejected += 1; return Err(format!("period {} not coprime with {}", e.period, p)); } }
        self.hands.insert(e.period, e);
        Ok(())
    }

    pub fn periods(&self) -> Vec<u64> { self.hands.keys().copied().collect() }
    pub fn range(&self) -> u128 { crt::range(&self.periods()) }

    /// Reconstruct against a known bound (e.g. "heights are below 10^8").
    pub fn reading(&self, bound: u128) -> CollectorReading {
        let periods = self.periods();
        let remainders: Vec<f64> = self.hands.values().map(|h| h.remainder).collect();
        let peers: Vec<String> = self.hands.values().map(|h| h.node.clone()).collect();
        let range = self.range();
        let covers = range >= bound && !periods.is_empty();
        let result = if periods.is_empty() {
            crt::BoundedReconstruction { reconstruction: None, consistent: false, suspect: None, bound, false_accept_p: 1.0, note: "no hands heard".into() }
        } else if covers {
            crt::reconstruct_bounded(&remainders, &periods, bound)
        } else {
            // partial time: t mod ∏x — honest, and all one gets from too few peers
            let r = crt::reconstruct(&remainders, &periods).ok();
            crt::BoundedReconstruction { reconstruction: r, consistent: false, suspect: None, bound, false_accept_p: 1.0, note: format!("only {} hand(s): time known modulo {} (< bound {}) — need more independent peers", periods.len(), range, bound) }
        };
        let note = if covers { "the time exists only as this composition; no single peer published it".into() } else { "partial composition".into() };
        CollectorReading { unit: self.unit.clone(), hands: periods.len(), peers, periods, remainders, range: range.to_string(), bound: bound.to_string(), covers_bound: covers, result, note }
    }
}

/// Network side (tokio + flux-p2p).
pub mod net {
    use super::*;
    use flux_p2p::{NetworkConfig, NetworkManager};

    /// A clock peer: gossipsub on the clock topic only, no DAGKnight/SAP/X-algo/entanglement
    /// (the 2026-09-12 lesson: entangled publish routed every message through top-N peers).
    pub fn config(node_id: &str, listen: &str, bootstrap: Vec<String>) -> NetworkConfig {
        NetworkConfig { node_id: node_id.into(), listen_addr: listen.into(), bootstrap_peers: bootstrap, dagknight_enabled: false, sap_enabled: false, x_algo_enabled: false, entanglement_enabled: false, gossipsub_topics: vec![CLOCK_TOPIC.into()] }
    }

    /// The peer id a node_id maps to (flux-p2p derives the keypair from the node_id).
    pub fn peer_id_of(node_id: &str) -> String { flux_p2p::swarm::peer_id_string(node_id) }

    pub async fn start(cfg: NetworkConfig) -> Result<NetworkManager, String> {
        let mut nm = NetworkManager::new(cfg);
        nm.start().await?;
        Ok(nm)
    }

    /// Publish one hand `ticks` times, `interval` apart, reading the time from `read_time`.
    pub async fn run_hand<F: Fn() -> Option<f64>>(nm: &NetworkManager, node: &str, unit: &str, period: u64, z: u32, ticks: u32, interval: std::time::Duration, read_time: F) -> Vec<HandEnvelope> {
        let mut out = Vec::new();
        for _ in 0..ticks {
            if let Some(t) = read_time() {
                let e = HandEnvelope::new(node, unit, period, t.rem_euclid(period as f64), z);
                if nm.publish(CLOCK_TOPIC, e.encode()).is_ok() { out.push(e); }
            }
            tokio::time::sleep(interval).await;
        }
        out
    }

    /// Listen for hands for `secs` and hand back the collector.
    pub async fn collect(nm: &NetworkManager, unit: &str, secs: f64) -> HandCollector {
        let mut rx = nm.subscribe(CLOCK_TOPIC);
        let mut col = HandCollector::new(unit);
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs_f64(secs);
        loop {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Some((_, data))) => { if let Ok(e) = HandEnvelope::decode(&data) { let _ = col.ingest(e); } }
                Ok(None) | Err(_) => break,
            }
        }
        col
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_round_trips_and_tamper_is_caught() {
        let e = HandEnvelope::new("epsilon", "block", 2003, 19_040_287.0, 1);
        assert!(e.verify());
        assert_eq!(e.remainder, (19_040_287 % 2003) as f64);
        let d = HandEnvelope::decode(&e.encode()).unwrap();
        assert_eq!(d, e);
        let mut bad = e.clone();
        bad.remainder += 1.0;
        assert!(!bad.verify());
        assert!(HandEnvelope::decode(&bad.encode()).is_err());
    }

    #[test]
    fn three_peers_compose_a_height_none_of_them_published() {
        let h: u64 = 19_040_287;
        let mut col = HandCollector::new("block");
        for (node, p) in [("epsilon", 10_007u64), ("happysrv", 10_009), ("node3", 10_037)] {
            col.ingest(HandEnvelope::new(node, "block", p, (h % p) as f64, 1)).unwrap();
        }
        let r = col.reading(100_000_000);
        assert!(r.covers_bound);
        assert!(r.result.consistent);
        assert_eq!(r.result.reconstruction.unwrap().integer, h as u128);
        // two peers only against a larger bound: partial (time known modulo their product)
        let mut two = HandCollector::new("block");
        two.ingest(HandEnvelope::new("a", "block", 2003, (h % 2003) as f64, 1)).unwrap();
        two.ingest(HandEnvelope::new("b", "block", 2011, (h % 2011) as f64, 1)).unwrap();
        let r2 = two.reading(100_000_000);
        assert!(!r2.covers_bound);
        assert_eq!(r2.result.reconstruction.unwrap().integer, (h as u128) % (2003 * 2011));
        // a non-coprime hand is refused
        assert!(two.ingest(HandEnvelope::new("c", "block", 4022, 1.0, 1)).is_err());
    }

    #[test]
    fn a_lagging_follower_is_named() {
        let h: u64 = 19_040_287;
        let mut col = HandCollector::new("block");
        col.ingest(HandEnvelope::new("epsilon", "block", 10_007, (h % 10_007) as f64, 1)).unwrap();
        col.ingest(HandEnvelope::new("happysrv", "block", 10_009, ((h - 5) % 10_009) as f64, 1)).unwrap();
        col.ingest(HandEnvelope::new("node3", "block", 10_037, (h % 10_037) as f64, 1)).unwrap();
        col.ingest(HandEnvelope::new("sigil-top", "block", 10_039, (h % 10_039) as f64, 1)).unwrap();
        let r = col.reading(100_000_000);
        assert!(!r.result.consistent);
        let s = r.result.suspect.unwrap();
        assert_eq!(r.peers[s], "happysrv", "{}", r.result.note);
        assert_eq!(r.result.reconstruction.unwrap().integer, h as u128);
    }

    /// Two real libp2p peers on loopback: one hand, one collector. Gossipsub needs a mesh, so
    /// the collector subscribes first and the hand keeps publishing for a while.
    #[test]
    fn loopback_hand_reaches_a_collector_over_flux_p2p() {
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let port_a = 39_611u16;
            let port_b = 39_612u16;
            let id_a = "flux-clock-test-collector";
            let id_b = "flux-clock-test-hand";
            let addr_a = format!("/ip4/127.0.0.1/tcp/{port_a}/p2p/{}", net::peer_id_of(id_a));
            let a = net::start(net::config(id_a, &format!("/ip4/127.0.0.1/tcp/{port_a}"), vec![])).await.expect("collector starts");
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let b = net::start(net::config(id_b, &format!("/ip4/127.0.0.1/tcp/{port_b}"), vec![addr_a])).await.expect("hand starts");
            let h = 19_040_287u64;
            let hand = tokio::spawn(async move {
                let out = net::run_hand(&b, "hand-peer", "block", 5003, 1, 40, std::time::Duration::from_millis(500), || Some(h as f64)).await;
                (b, out)
            });
            let col = net::collect(&a, "block", 18.0).await;
            let (b, sent) = hand.await.unwrap();
            let _ = a.stop().await;
            let _ = b.stop().await;
            assert!(!sent.is_empty(), "hand published nothing");
            assert_eq!(col.hands.len(), 1, "collector heard {} hands over loopback (rejected {})", col.hands.len(), col.rejected);
            let e = &col.hands[&5003];
            assert_eq!(e.node, "hand-peer");
            assert_eq!(e.remainder, (h % 5003) as f64);
        });
    }
}
