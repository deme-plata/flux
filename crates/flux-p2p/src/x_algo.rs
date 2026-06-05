// X-Algo — Cross-Algorithm Multi-Dimensional Scoring
//
// Extends SAP scoring with cross-algorithmic dimensions that combine
// multiple data sources into a composite trust/quality score.
//
// X-Algo dimensions:
//   1. Temporal Trust Decay   — recent behavior weighted more than history
//   2. Cross-Validator Consensus — agreement with supermajority on block validity
//   3. Transaction Quality    — fraction of valid, non-spam transactions
//   4. Network Topology Rank  — PageRank-like score from the peer graph
//   5. Economic Efficiency    — gas/QUG spent vs. value produced
//
// The X-Algo score complements SAP by capturing emergent properties
// that cannot be measured by individual metrics alone.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use super::sap;

/// A peer identifier (shared type with SAP).
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct PeerId(pub String);

impl From<&str> for PeerId {
    fn from(s: &str) -> Self { PeerId(s.to_string()) }
}

impl From<String> for PeerId {
    fn from(s: String) -> Self { PeerId(s) }
}

/// Cross-algorithm composite score.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CrossScore {
    pub peer: PeerId,
    /// Overall cross-score (0.0–1.0).
    pub total: f64,
    /// Individual dimension scores.
    pub dimensions: CrossDimensions,
    /// Correlation with SAP score (for validation of scoring consistency).
    pub sap_correlation: f64,
    /// When this score was computed.
    pub computed_at_ms: u64,
}

/// All cross-algorithm scoring dimensions.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct CrossDimensions {
    /// Temporal trust: recent 100 rounds weighted exponentially (0–1).
    pub temporal_trust: f64,
    /// Consensus alignment: fraction of blocks agreed with supermajority (0–1).
    pub consensus_align: f64,
    /// Transaction quality: fraction of non-spam, valid txs (0–1).
    pub tx_quality: f64,
    /// Topology rank: normalized PageRank in the peer graph (0–1).
    pub topology_rank: f64,
    /// Economic efficiency: value_produced / cost_spent (0–∞, clamped to 0–1).
    pub econ_efficiency: f64,
}

/// Configurable weights for X-Algo dimensions.
#[derive(Clone, Debug)]
pub struct CrossWeights {
    /// Temporal trust: recent behavior weighted more (0.30).
    pub temporal: f64,
    /// Consensus alignment: agreement with supermajority (0.25).
    pub consensus: f64,
    /// Transaction quality: non-spam fraction (0.20).
    pub tx_quality: f64,
    /// Topology rank: PageRank in peer graph (0.15).
    pub topology: f64,
    /// Economic efficiency: ROI ratio (0.10).
    pub econ: f64,
}

impl Default for CrossWeights {
    fn default() -> Self {
        CrossWeights { temporal: 0.30, consensus: 0.25, tx_quality: 0.20, topology: 0.15, econ: 0.10 }
    }
}

/// A table of cross-scores, keyed by PeerId.
#[derive(Clone, Debug)]
pub struct CrossScoreTable {
    scores: HashMap<PeerId, CrossScore>,
    /// Historical window: recent N rounds for temporal trust.
    history_window: usize,
    dim_weights: CrossWeights,
    /// Per-peer history: (round_number, was_correct, tx_quality_at_time).
    peer_history: HashMap<PeerId, Vec<(u64, bool, f64)>>,
}

impl CrossScoreTable {
    /// Create a new cross-score table.
    pub fn new() -> Self {
        CrossScoreTable {
            scores: HashMap::new(),
            dim_weights: CrossWeights::default(),
            history_window: 200,
            peer_history: HashMap::new(),
        }
    }

    /// Create with custom history window size.
    pub fn with_window(history_window: usize) -> Self {
        CrossScoreTable {
            scores: HashMap::new(),
            dim_weights: CrossWeights::default(),
            history_window: history_window.max(50),
            peer_history: HashMap::new(),
        }
    }

    /// Get a peer's cross-score.
    pub fn get(&self, peer: &PeerId) -> Option<CrossScore> {
        self.scores.get(peer).cloned()
    }

    /// Number of peers in the table.
    pub fn len(&self) -> usize {
        self.scores.len()
    }

    /// Record a round outcome for a peer.
    /// `correct`: whether the peer's proposal agreed with supermajority.
    /// `tx_quality`: fraction of valid transactions (0–1).
    pub fn record_round(&mut self, peer: PeerId, round: u64, correct: bool, tx_quality: f64) {
        let history = self.peer_history.entry(peer.clone()).or_default();
        history.push((round, correct, tx_quality));

        // Prune old history
        if history.len() > self.history_window * 2 {
            let cutoff = round.saturating_sub(self.history_window as u64);
            history.retain(|(r, _, _)| *r >= cutoff);
        }

        // Recompute score
        self.recompute(&peer);
    }

    /// Update topology rank from a peer graph (PageRank-like).
    /// `ranks`: PeerId → normalized rank (0–1).
    pub fn update_topology(&mut self, ranks: &HashMap<PeerId, f64>) {
        for (peer, rank) in ranks {
            if let Some(score) = self.scores.get_mut(peer) {
                score.dimensions.topology_rank = rank.clamp(0.0, 1.0);
            } else {
                // Create new entry with just topology rank
                self.scores.insert(peer.clone(), CrossScore {
                    peer: peer.clone(),
                    total: *rank,
                    dimensions: CrossDimensions {
                        topology_rank: *rank,
                        ..Default::default()
                    },
                    sap_correlation: 0.0,
                    computed_at_ms: now_ms(),
                });
            }
        }
        // Recompute totals
        let peers: Vec<PeerId> = self.scores.keys().cloned().collect();
        for peer in &peers {
            self.recompute_total(peer);
        }
    }

    /// Compute economic efficiency: value produced vs. cost.
    pub fn update_econ(&mut self, peer: &PeerId, value_produced_qug: u64, cost_spent_qug: u64) {
        if let Some(score) = self.scores.get_mut(peer) {
            score.dimensions.econ_efficiency = if cost_spent_qug > 0 {
                (value_produced_qug as f64 / cost_spent_qug as f64).clamp(0.0, 1.0)
            } else if value_produced_qug > 0 {
                1.0 // Pure profit
            } else {
                0.0
            };
            self.recompute_total(peer);
        }
    }

    /// Get the top N peers by cross-score.
    pub fn top_peers(&self, n: usize) -> Vec<&CrossScore> {
        let mut scores: Vec<&CrossScore> = self.scores.values().collect();
        scores.sort_by(|a, b| b.total.partial_cmp(&a.total).unwrap_or(std::cmp::Ordering::Equal));
        scores.truncate(n);
        scores
    }

    /// Correlate X-Algo scores with SAP scores.
    pub fn correlate_with_sap(&mut self, sap_table: &sap::ScoreTable) {
        for (peer, score) in &mut self.scores {
            let sap_peer = sap::PeerId(peer.0.clone());
            if let Some(sap_score) = sap_table.get(&sap_peer) {
                // Simple correlation: product (both normalized 0–1)
                score.sap_correlation = sap_score * score.total;
            }
        }
    }

    // ── Internal ──

    fn recompute(&mut self, peer: &PeerId) {
        let history = self.peer_history.get(peer);
        let dimensions = CrossDimensions {
            temporal_trust: self.compute_temporal_trust(history),
            consensus_align: self.compute_consensus_align(history),
            tx_quality: self.compute_tx_quality(history),
            topology_rank: self.scores.get(peer)
                .map(|s| s.dimensions.topology_rank)
                .unwrap_or(0.5),
            econ_efficiency: self.scores.get(peer)
                .map(|s| s.dimensions.econ_efficiency)
                .unwrap_or(0.5),
        };

        let existing = self.scores.get(peer).map(|s| s.sap_correlation).unwrap_or(0.0);

        self.scores.insert(peer.clone(), CrossScore {
            peer: peer.clone(),
            total: 0.0, // Will be set by recompute_total
            dimensions,
            sap_correlation: existing,
            computed_at_ms: now_ms(),
        });
        self.recompute_total(peer);
    }

    fn recompute_total(&mut self, peer: &PeerId) {
        let w = &self.dim_weights;
        if let Some(score) = self.scores.get_mut(peer) {
            let d = &score.dimensions;
            score.total = d.temporal_trust * w.temporal
                + d.consensus_align * w.consensus
                + d.tx_quality * w.tx_quality
                + d.topology_rank * w.topology
                + d.econ_efficiency * w.econ;
        }
    }

    fn compute_temporal_trust(&self, history: Option<&Vec<(u64, bool, f64)>>) -> f64 {
        let history = match history {
            Some(h) if !h.is_empty() => h,
            _ => return 0.5, // Neutral for unknown peers
        };

        let window = self.history_window as f64;
        let latest_round = history.last().map(|(r, _, _)| *r).unwrap_or(0);

        let mut weighted_sum = 0.0;
        let mut weight_total = 0.0;

        for (round, correct, _) in history.iter().rev().take(self.history_window) {
            let age = (latest_round - round) as f64;
            let weight = (-age / window).exp(); // Exponential decay
            weighted_sum += if *correct { weight } else { 0.0 };
            weight_total += weight;
        }

        if weight_total > 0.0 { weighted_sum / weight_total } else { 0.5 }
    }

    fn compute_consensus_align(&self, history: Option<&Vec<(u64, bool, f64)>>) -> f64 {
        let history = match history {
            Some(h) if !h.is_empty() => h,
            _ => return 0.5,
        };

        let correct_count = history.iter().filter(|(_, correct, _)| *correct).count();
        correct_count as f64 / history.len() as f64
    }

    fn compute_tx_quality(&self, history: Option<&Vec<(u64, bool, f64)>>) -> f64 {
        let history = match history {
            Some(h) if !h.is_empty() => h,
            _ => return 0.5,
        };

        let recent: Vec<_> = history.iter().rev().take(self.history_window).collect();
        if recent.is_empty() { return 0.5; }

        let avg_quality: f64 = recent.iter().map(|(_, _, q)| q).sum::<f64>() / recent.len() as f64;
        avg_quality.clamp(0.0, 1.0)
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_temporal_trust_decay() {
        let mut table = CrossScoreTable::with_window(10);
        let peer = PeerId::from("test-peer");

        // Record 10 rounds of correct behavior
        for r in 0..10 {
            table.record_round(peer.clone(), r, true, 1.0);
        }

        let score = table.get(&peer).unwrap();
        assert!(score.dimensions.temporal_trust > 0.9,
            "Consistently correct peer should have high temporal trust");
        assert!(score.dimensions.consensus_align > 0.9);
    }

    #[test]
    fn test_temporal_trust_plummets_after_equivocation() {
        let mut table = CrossScoreTable::with_window(10);
        let peer = PeerId::from("byzantine-peer");

        // 5 good rounds
        for r in 0..5 {
            table.record_round(peer.clone(), r, true, 1.0);
        }
        // 1 bad round
        table.record_round(peer.clone(), 5, false, 0.0);

        let score = table.get(&peer).unwrap();
        assert!(score.dimensions.temporal_trust < 0.9,
            "Recent equivocation should decrease trust");
    }

    #[test]
    fn test_econ_efficiency() {
        let mut table = CrossScoreTable::new();
        let peer = PeerId::from("efficient-peer");

        // First record a round to create the entry
        table.record_round(peer.clone(), 0, true, 1.0);
        table.update_econ(&peer, 1000, 100); // 10× ROI

        let score = table.get(&peer).unwrap();
        assert!((score.dimensions.econ_efficiency - 1.0).abs() < 0.01,
            "10x ROI should give max efficiency");
    }
}