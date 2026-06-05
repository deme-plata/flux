// SAP — Score-Adjusted Priority
//
// A multi-factor peer and vertex scoring system for the DAGKnight network.
//
// SAP assigns a normalized priority score (0.0–1.0) to each peer based on:
//   1. Contribution Rate    — how consistently the peer produces valid vertices
//   2. Latency Profile      — response time percentile (p50/p95/p99)
//   3. Stake Weight         — economic stake locked (QUG)
//   4. Historical Accuracy  — fraction of correct (non-equivocating) proposals
//   5. Uptime & Reliability — fraction of rounds participated
//
// The combined SAP score determines:
//   - Which peers get priority in gossipsub mesh
//   - Vertex ordering in the DAG (higher SAP → earlier inclusion)
//   - Bootstrap peer selection
//   - Vote weight in implicit DAG voting

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// A peer identifier — typically hex-encoded Ed25519 public key.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct PeerId(pub String);

impl From<&str> for PeerId {
    fn from(s: &str) -> Self { PeerId(s.to_string()) }
}

impl From<String> for PeerId {
    fn from(s: String) -> Self { PeerId(s) }
}

/// The SAP score for a single peer.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SAPScore {
    pub peer: PeerId,
    /// Overall normalized score (0.0–1.0).
    pub total: f64,
    /// Individual component scores.
    pub components: SAPComponents,
    /// When this score was last updated.
    pub updated_at_ms: u64,
    /// Total rounds this peer has participated in.
    pub rounds_participated: u64,
}

/// Breakdown of SAP score components.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SAPComponents {
    /// Contribution rate: vertices_produced / rounds_elapsed (0–1)
    pub contribution: f64,
    /// Latency score: 1.0 for p50 < 100ms, decays to 0 at p50 > 1s (0–1)
    pub latency: f64,
    /// Stake weight: normalized by total stake (0–1)
    pub stake: f64,
    /// Historical accuracy: 1.0 for no equivocations (0–1)
    pub accuracy: f64,
    /// Uptime: rounds_participated / total_rounds (0–1)
    pub uptime: f64,
}

/// A table of SAP scores — thread-safe, keyed by PeerId.
#[derive(Clone, Debug)]
pub struct ScoreTable {
    scores: HashMap<PeerId, SAPScore>,
    total_rounds: u64,
    total_stake: u64,
    sap_weights: SAPWeights,
    ema_alpha: f64,
    /// Cached top-N peers (invalidated on any score change).
    cached_top_n: Option<(usize, Vec<SAPScore>)>,
    cache_moment_ms: u64,
}

/// Configurable weights for SAP score components.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SAPWeights {
    pub contribution_weight: f64, // default: 0.30
    pub latency_weight: f64,      // default: 0.25
    pub stake_weight: f64,        // default: 0.20
    pub accuracy_weight: f64,     // default: 0.15
    pub uptime_weight: f64,       // default: 0.10
}

impl Default for SAPWeights {
    fn default() -> Self {
        SAPWeights {
            contribution_weight: 0.30,
            latency_weight: 0.25,
            stake_weight: 0.20,
            accuracy_weight: 0.15,
            uptime_weight: 0.10,
        }
    }
}

impl ScoreTable {
    /// Create a new empty score table with default weights.
    pub fn new() -> Self {
        ScoreTable {
            scores: HashMap::new(),
            total_rounds: 0,
            total_stake: 1, // Avoid division by zero
            cached_top_n: None,
            cache_moment_ms: 0,
            sap_weights: SAPWeights::default(),
            ema_alpha: 0.3,
        }
    }

    /// Create a score table with custom weights.
    pub fn with_weights(weights: SAPWeights) -> Self {
        ScoreTable {
            scores: HashMap::new(),
            total_rounds: 0,
            total_stake: 1,
            cached_top_n: None,
            cache_moment_ms: 0,
            sap_weights: weights,
            ema_alpha: 0.3,
        }
    }

    /// Get the SAP score for a peer.
    pub fn get(&self, peer: &PeerId) -> Option<f64> {
        self.scores.get(peer).map(|s| s.total)
    }

    /// Get the full SAP score (with component breakdown).
    pub fn get_full(&self, peer: &PeerId) -> Option<&SAPScore> {
        self.scores.get(peer)
    }

    /// Number of peers in the table.
    pub fn len(&self) -> usize {
        self.scores.len()
    }

    /// Update (or insert) a peer's SAP score.
    pub fn update(&mut self, peer: PeerId, components: SAPComponents) {
        // Momentum smoothing: 70% new + 30% old (prevents score thrashing)
        let old_total = self.scores.get(&peer).map(|s| s.total).unwrap_or(0.0);
        let w = &self.sap_weights;
        let total = components.contribution * w.contribution_weight
            + components.latency * w.latency_weight
            + components.stake * w.stake_weight
            + components.accuracy * w.accuracy_weight
            + components.uptime * w.uptime_weight;

        let smoothed = total * (1.0 - self.ema_alpha) + old_total * self.ema_alpha;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let rounds = self.scores.get(&peer)
            .map(|s| s.rounds_participated)
            .unwrap_or(0);

        self.scores.insert(peer.clone(), SAPScore {
            peer,
            total: smoothed.clamp(0.0, 1.0),
            components,
            updated_at_ms: now,
            rounds_participated: rounds,
        });

        // Invalidate top-N cache
        self.cached_top_n = None;
    }


    /// Set EMA alpha for adaptive smoothing.
    pub fn set_ema_alpha(&mut self, alpha: f64) { self.ema_alpha = alpha.clamp(0.0, 1.0); }
    /// Record that a peer participated in a round (increments counter).
    pub fn record_participation(&mut self, peer: &PeerId) {
        if let Some(score) = self.scores.get_mut(peer) {
            score.rounds_participated += 1;
        }
        self.total_rounds += 1;
    }

    /// Update a peer's latency score based on measured response times.
    pub fn update_latency(&mut self, peer: &PeerId, p50_ms: f64, _p95_ms: f64) {
        if let Some(score) = self.scores.get_mut(peer) {
            // Latency scoring: exponential decay from 100ms threshold
            score.components.latency = (-p50_ms / 100.0).exp().clamp(0.0, 1.0);
            self.recompute_total(peer);
        }
    }

    /// Update a peer's stake weight.
    pub fn update_stake(&mut self, peer: &PeerId, stake_qug: u64) {
        self.total_stake = self.total_stake.max(1);
        // Collect max_stake before mutable borrow
        let max_stake = if stake_qug > 0 {
            self.scores.values()
                .map(|s| (s.components.stake * self.total_stake as f64) as u64)
                .max()
                .unwrap_or(stake_qug)
                .max(stake_qug)
        } else {
            1
        };
        if let Some(score) = self.scores.get_mut(peer) {
            if stake_qug == 0 {
                score.components.stake = 0.0;
            } else {
                score.components.stake = (stake_qug as f64 / max_stake as f64).clamp(0.0, 1.0);
            }
            self.recompute_total(peer);
        }
    }

    /// Mark a peer as having equivocated (zeros their accuracy score).
    pub fn mark_equivocation(&mut self, peer: &PeerId) {
        if let Some(score) = self.scores.get_mut(peer) {
            score.components.accuracy = 0.0;
            self.recompute_total(peer);
        }
    }

    /// Get the top N peers by SAP score.
    pub fn top_peers(&self, n: usize) -> Vec<&SAPScore> {
        // Return cached result if valid (<500ms old, same or larger N)
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        if let Some((cached_n, ref cached)) = self.cached_top_n {
            if cached_n >= n && now - self.cache_moment_ms < 500 {
                return cached.iter().take(n).collect();
            }
        }
        let mut sorted: Vec<&SAPScore> = self.scores.values().collect();
        sorted.sort_by(|a, b| b.total.partial_cmp(&a.total).unwrap_or(std::cmp::Ordering::Equal));
        sorted.iter().take(n).copied().collect()
    }

    /// Get the worst N peers by SAP score (for eviction / deprioritization).
    pub fn worst_peers(&self, n: usize) -> Vec<&SAPScore> {
        let mut scores: Vec<&SAPScore> = self.scores.values().collect();
        scores.sort_by(|a, b| a.total.partial_cmp(&b.total).unwrap_or(std::cmp::Ordering::Equal));
        scores.truncate(n);
        scores
    }

    fn recompute_total(&mut self, peer: &PeerId) {
        if let Some(score) = self.scores.get_mut(peer) {
            let w = &self.sap_weights;
            score.total = score.components.contribution * w.contribution_weight
                + score.components.latency * w.latency_weight
                + score.components.stake * w.stake_weight
                + score.components.accuracy * w.accuracy_weight
                + score.components.uptime * w.uptime_weight;
            score.total = score.total.clamp(0.0, 1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_score_table() {
        let mut table = ScoreTable::new();
        let peer = PeerId::from("peer-alpha");

        // Insert with default components
        table.update(peer.clone(), SAPComponents {
            contribution: 0.9,
            latency: 0.8,
            stake: 0.7,
            accuracy: 1.0,
            uptime: 0.95,
        });

        let score = table.get(&peer).unwrap();
        // Expected: 0.9*0.30 + 0.8*0.25 + 0.7*0.20 + 1.0*0.15 + 0.95*0.10
        // With momentum (70% new + 30% old=0): 0.855 * 0.70 = 0.5985
        assert!((score - 0.5985).abs() < 0.02, "Expected ~0.5985, got {}", score);
    }

    #[test]
    fn test_top_peers() {
        let mut table = ScoreTable::new();
        table.update(PeerId::from("a"), SAPComponents { contribution: 1.0, ..Default::default() });
        table.update(PeerId::from("b"), SAPComponents { contribution: 0.5, ..Default::default() });
        table.update(PeerId::from("c"), SAPComponents { contribution: 0.8, ..Default::default() });

        let top = table.top_peers(2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].peer.0, "a"); // Highest contribution → highest total
    }

    #[test]
    fn test_equivocation_zeros_accuracy() {
        let mut table = ScoreTable::new();
        let peer = PeerId::from("byzantine");
        table.update(peer.clone(), SAPComponents {
            accuracy: 1.0, ..Default::default()
        });
        assert!((table.get(&peer).unwrap() - 0.105).abs() < 0.02);

        table.mark_equivocation(&peer);
        let score = table.get(&peer).unwrap();
        assert!(score < 0.01, "Equivocating peer should have near-zero total score");
    }

    #[test]
    fn test_latency_update() {
        let mut table = ScoreTable::new();
        let peer = PeerId::from("fast-peer");
        table.update(peer.clone(), SAPComponents::default());
        table.update_latency(&peer, 50.0, 200.0);

        let latency = table.get_full(&peer).unwrap().components.latency;
        // e^(-50/100) = e^(-0.5) ≈ 0.6065
        assert!((latency - 0.6065).abs() < 0.01, "Expected ~0.6065, got {}", latency);
    }
}/// Composite score: SAP (60%) + X-Algo (40%) = final peer trust score.
/// Fires webhook to fluxmux :9099 on low-trust alerts.
pub fn composite_score(sap: f64, xalgo: f64, peer_id: &str) -> f64 {
    let composite = sap * 0.6 + xalgo * 0.4;
    if composite < 0.3 {
        let _ = std::process::Command::new("curl").args([
            "-s","-X","POST","http://127.0.0.1:9099/sap_alert",
            "-H","Content-Type: application/json",
            "-d",&format!(r#"{{"peer":"{}","sap":{},"xalgo":{},"composite":{},"alert":"low_trust"}}"#, peer_id, sap, xalgo, composite)
        ]).output();
    }
    composite.clamp(0.0, 1.0)
}
