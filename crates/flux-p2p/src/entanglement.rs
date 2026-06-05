// flux-p2p entanglement — QtFT-inspired P2P routing via transaction similarity
//
// Based on QtFT Path C (qtft-quillon-integration.tex §1.3):
// "Use the linking number lk(L_i, L_j) between transaction knots to
//  optimize P2P peer discovery and block propagation."
//
// Two-layer routing:
//   Layer 1 (Primary):   Linking number lk(K_A, K_B) from Gauss codes.
//                        O(|C|) per peer, detects structural tx patterns.
//   Layer 2 (Fallback):  Jaccard similarity of Bloom filters.
//                        O(k) approximation when Gauss codes unavailable.
//
// Weight(P_i, P_j) = |lk(K_{P_i}, K_{P_j})| / max_k |lk(K_{P_i}, K_{P_k})|
//
// This replaces round-robin gossip mesh selection with entanglement-weighted
// peer selection, reducing redundant block transmissions by ~40% (estimated).

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// A single crossing in a transaction knot's Gauss code.
/// Each transaction in a block contributes one crossing.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Crossing {
    /// Transaction hash (BLAKE3, 32 bytes).
    pub tx_hash: [u8; 32],
    /// Crossing sign: +1 for standard tx, -1 for CoinJoin/deanonymization tx.
    pub sign: i8,
}

/// Gauss code: an ordered sequence of crossings describing a transaction knot.
/// The linking number between two knots is computed from their Gauss codes.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GaussCode {
    pub crossings: Vec<Crossing>,
    /// Number of blocks this code aggregates.
    pub block_count: u64,
    /// When this code was computed.
    pub computed_at_ms: u64,
}

impl GaussCode {
    pub fn new() -> Self {
        GaussCode {
            crossings: Vec::new(),
            block_count: 0,
            computed_at_ms: now_ms(),
        }
    }

    pub fn len(&self) -> usize {
        self.crossings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.crossings.is_empty()
    }
}

/// A transaction knot: the topological representation of a peer's recent
/// transaction activity. Two peers' knots are "entangled" if their
/// linking number is non-zero.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TransactionKnot {
    pub peer_id: String,
    pub gauss_code: GaussCode,
    /// Cached linking numbers with other peers.
    pub linking_cache: HashMap<String, i64>,
}

impl TransactionKnot {
    pub fn new(peer_id: String) -> Self {
        TransactionKnot {
            peer_id,
            gauss_code: GaussCode::new(),
            linking_cache: HashMap::new(),
        }
    }

    /// Compute the linking number between this knot and another.
    ///
    /// The Gauss linking integral, discretized:
    ///   lk(K₁, K₂) = sum over shared tx hashes of sign₁(tx) * sign₂(tx)
    ///
    /// This is O(|C₁| * |C₂|) in the worst case, but we use a hash set
    /// for the smaller knot's tx hashes to make it O(|C₁| + |C₂|).
    ///
    /// Returns the raw linking number (can be negative for anti-correlated).
    pub fn linking_number(&mut self, other: &TransactionKnot) -> i64 {
        if self.gauss_code.is_empty() || other.gauss_code.is_empty() {
            return 0;
        }

        // Check cache
        if let Some(&cached) = self.linking_cache.get(&other.peer_id) {
            return cached;
        }

        // Build a hash set of (tx_hash, sign) for the smaller knot
        let (smaller, larger) = if self.gauss_code.len() <= other.gauss_code.len() {
            (&self.gauss_code, &other.gauss_code)
        } else {
            (&other.gauss_code, &self.gauss_code)
        };

        let mut tx_signs: HashMap<[u8; 32], i8> = HashMap::new();
        for c in &smaller.crossings {
            tx_signs.insert(c.tx_hash, c.sign);
        }

        let mut lk: i64 = 0;
        for c in &larger.crossings {
            if let Some(&other_sign) = tx_signs.get(&c.tx_hash) {
                lk += (c.sign as i64) * (other_sign as i64);
            }
        }

        // Cache the result
        self.linking_cache.insert(other.peer_id.clone(), lk);

        lk
    }

    /// Normalized entanglement weight: |lk| / max_possible.
    /// max_possible is the number of crossings in the smaller knot (all shared).
    pub fn entanglement_weight(&mut self, other: &TransactionKnot) -> f64 {
        let lk = self.linking_number(other);
        if lk == 0 {
            return 0.0;
        }
        let max_possible = self.gauss_code.len().min(other.gauss_code.len()) as i64;
        if max_possible == 0 {
            return 0.0;
        }
        (lk.abs() as f64 / max_possible as f64).min(1.0)
    }

    /// Add crossings from a block's transactions.
    pub fn add_block_txs(&mut self, tx_hashes: &[[u8; 32]], is_coinjoin: bool) {
        let sign: i8 = if is_coinjoin { -1 } else { 1 };
        for tx_hash in tx_hashes {
            self.gauss_code.crossings.push(Crossing {
                tx_hash: *tx_hash,
                sign,
            });
        }
        self.gauss_code.block_count += 1;
        self.gauss_code.computed_at_ms = now_ms();

        // Prune old crossings if we exceed the window
        let max_crossings = 500; // ~100 blocks * 5 tx/block
        while self.gauss_code.crossings.len() > max_crossings {
            self.gauss_code.crossings.remove(0);
        }

        // Invalidate cache (our knot changed)
        self.linking_cache.clear();
    }
}

// ═══════════════════════════════════════════════════════════════
// Configuration
// ═══════════════════════════════════════════════════════════════

/// Configuration for entanglement routing.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct EntanglementConfig {
    /// Bloom filter size (bits) — fallback layer.
    pub filter_size: usize,
    /// Number of hash functions for Bloom filter.
    pub hash_count: usize,
    /// How many recent blocks to track per peer.
    pub window_blocks: usize,
    /// Minimum entanglement score to include peer in priority mesh.
    pub min_score: f64,
    /// Enable entanglement routing (feature flag).
    pub enabled: bool,
    /// Prefer linking-number routing over Bloom when Gauss codes are available.
    pub prefer_knot_routing: bool,
}

impl Default for EntanglementConfig {
    fn default() -> Self {
        EntanglementConfig {
            filter_size: 1024,
            hash_count: 4,
            window_blocks: 100,
            min_score: 0.1,
            enabled: true,
            prefer_knot_routing: true,
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Bloom filter fallback layer
// ═══════════════════════════════════════════════════════════════

/// A peer's transaction fingerprint — Bloom filter of recent tx hashes.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TxFingerprint {
    pub peer_id: String,
    pub bits: Vec<u8>,
    pub tx_count: u64,
    pub updated_at_ms: u64,
}

impl TxFingerprint {
    pub fn new(peer_id: String, filter_size: usize) -> Self {
        let byte_count = (filter_size + 7) / 8;
        TxFingerprint {
            peer_id,
            bits: vec![0u8; byte_count],
            tx_count: 0,
            updated_at_ms: now_ms(),
        }
    }

    pub fn insert(&mut self, tx_hash: &[u8; 32]) {
        for i in 0..4 {
            let mut hasher = blake3::Hasher::new();
            hasher.update(&(i as u32).to_le_bytes());
            hasher.update(tx_hash);
            let hash = hasher.finalize();
            let idx = (u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap())
                % self.bits.len() as u64 * 8) as usize;
            let byte_idx = idx / 8;
            let bit_idx = idx % 8;
            if byte_idx < self.bits.len() {
                self.bits[byte_idx] |= 1 << bit_idx;
            }
        }
        self.tx_count += 1;
        self.updated_at_ms = now_ms();
    }

    fn bit_count(&self) -> usize {
        self.bits.iter().map(|b| b.count_ones() as usize).sum()
    }
}

// ═══════════════════════════════════════════════════════════════
// Two-layer Entanglement Router
// ═══════════════════════════════════════════════════════════════

/// Routing table — maps peer_id → entanglement score using two layers:
///   Layer 1: Linking number from Gauss codes (primary, precise)
///   Layer 2: Jaccard similarity of Bloom filters (fallback)
#[derive(Clone, Debug)]
pub struct EntanglementRouter {
    pub config: EntanglementConfig,
    /// Bloom fingerprints — fallback layer.
    fingerprints: HashMap<String, TxFingerprint>,
    /// Transaction knots — primary routing layer.
    knots: HashMap<String, TransactionKnot>,
    /// Pre-computed entanglement scores: (peer_a, peer_b) → score.
    scores: HashMap<(String, String), f64>,
    /// Top-N peers by entanglement (cached, invalidated on update).
    cached_top: Vec<(String, f64)>,
}

impl EntanglementRouter {
    pub fn new(config: EntanglementConfig) -> Self {
        EntanglementRouter {
            config,
            fingerprints: HashMap::new(),
            knots: HashMap::new(),
            scores: HashMap::new(),
            cached_top: Vec::new(),
        }
    }

    /// Register/update a peer with Bloom filter tx hashes (fallback layer).
    pub fn update_peer(&mut self, peer_id: &str, tx_hashes: &[[u8; 32]]) {
        if !self.config.enabled { return; }

        let fp = self.fingerprints
            .entry(peer_id.to_string())
            .or_insert_with(|| TxFingerprint::new(peer_id.to_string(), self.config.filter_size));

        for tx_hash in tx_hashes.iter().take(self.config.window_blocks * 10) {
            fp.insert(tx_hash);
        }

        if fp.tx_count > self.config.window_blocks as u64 * 50 {
            fp.bits = vec![0u8; (self.config.filter_size + 7) / 8];
            fp.tx_count = 0;
        }

        self.scores.retain(|(a, b), _| a != peer_id && b != peer_id);
        self.cached_top.clear();
    }

    /// Register/update a peer with a transaction knot (primary layer).
    pub fn update_knot(&mut self, knot: TransactionKnot) {
        if !self.config.enabled { return; }
        let peer_id = knot.peer_id.clone();
        self.knots.insert(peer_id.clone(), knot);
        self.scores.retain(|(a, b), _| a != &peer_id && b != &peer_id);
        self.cached_top.clear();
    }

    /// Get or create a TransactionKnot for a peer.
    pub fn get_or_create_knot(&mut self, peer_id: &str) -> &mut TransactionKnot {
        self.knots.entry(peer_id.to_string())
            .or_insert_with(|| TransactionKnot::new(peer_id.to_string()))
    }

    /// Add transactions to a peer's knot from a block notification.
    pub fn feed_block(&mut self, peer_id: &str, tx_hashes: &[[u8; 32]], is_coinjoin: bool) {
        if !self.config.enabled { return; }
        let knot = self.get_or_create_knot(peer_id);
        knot.add_block_txs(tx_hashes, is_coinjoin);

        // Also update Bloom fallback
        self.update_peer(peer_id, tx_hashes);
    }

    /// Compute entanglement score between two peers.
    ///
    /// Uses linking number (Layer 1) when Gauss codes are available for both peers,
    /// falls back to Jaccard similarity (Layer 2) otherwise.
    pub fn entanglement_score(&mut self, peer_a: &str, peer_b: &str) -> f64 {
        if peer_a == peer_b { return 1.0; }

        let key = if peer_a < peer_b {
            (peer_a.to_string(), peer_b.to_string())
        } else {
            (peer_b.to_string(), peer_a.to_string())
        };

        if let Some(&score) = self.scores.get(&key) {
            return score;
        }

        // Layer 1: linking number (preferred, more precise)
        if self.config.prefer_knot_routing {
            let has_both_knots = self.knots.contains_key(peer_a) && self.knots.contains_key(peer_b);
            if has_both_knots {
                // We need both knots simultaneously — clone to avoid borrow issues
                let knot_a = self.knots.get(peer_a).cloned();
                let knot_b = self.knots.get(peer_b).cloned();
                if let (Some(mut ka), Some(kb)) = (knot_a, knot_b) {
                    let weight = ka.entanglement_weight(&kb);
                    if weight > 0.0 {
                        self.scores.insert(key, weight);
                        return weight;
                    }
                }
            }
        }

        // Layer 2: Jaccard similarity (fallback)
        let fp_a = match self.fingerprints.get(peer_a) {
            Some(fp) => fp,
            None => return 0.0,
        };
        let fp_b = match self.fingerprints.get(peer_b) {
            Some(fp) => fp,
            None => return 0.0,
        };

        let intersection = fp_a.bits.iter().zip(fp_b.bits.iter())
            .map(|(a, b)| (a & b).count_ones() as usize)
            .sum::<usize>();

        let union = fp_a.bit_count() + fp_b.bit_count() - intersection;

        let score = if union > 0 {
            intersection as f64 / union as f64
        } else {
            0.0
        };

        self.scores.insert(key, score);
        score
    }

    /// Get the top-N peers by entanglement with the target peer.
    /// Replaces round-robin selection in the gossip mesh.
    pub fn top_entangled_peers(&mut self, self_id: &str, n: usize) -> Vec<(String, f64)> {
        if !self.config.enabled || self.fingerprints.len() <= 1 && self.knots.len() <= 1 {
            return Vec::new();
        }

        if !self.cached_top.is_empty() && self.cached_top.len() >= n {
            return self.cached_top.iter().take(n).cloned().collect();
        }

        // Collect all peer IDs except self
        let mut peer_ids: Vec<String> = Vec::new();
        for pid in self.fingerprints.keys() {
            if pid != self_id { peer_ids.push(pid.clone()); }
        }
        for pid in self.knots.keys() {
            if pid != self_id && !peer_ids.contains(pid) {
                peer_ids.push(pid.clone());
            }
        }

        let mut scored: Vec<(String, f64)> = peer_ids.iter()
            .map(|pid| {
                let score = self.entanglement_score(self_id, pid);
                (pid.clone(), score)
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(n);

        self.cached_top = scored.clone();
        scored
    }

    /// Number of tracked peers (Bloom + knot layers combined).
    pub fn peer_count(&self) -> usize {
        let mut count = self.fingerprints.len();
        for k in self.knots.keys() {
            if !self.fingerprints.contains_key(k) {
                count += 1;
            }
        }
        count
    }

    /// Get the Gauss code for a peer (for DAGKnight integration).
    pub fn get_gauss_code(&self, peer_id: &str) -> Option<&GaussCode> {
        self.knots.get(peer_id).map(|k| &k.gauss_code)
    }

    /// Get the linking number between two peers (for diagnostics).
    pub fn get_linking_number(&self, peer_a: &str, peer_b: &str) -> i64 {
        if let (Some(ka), Some(kb)) = (self.knots.get(peer_a), self.knots.get(peer_b)) {
            // Clone to get mutable access for caching
            let mut ka_clone = ka.clone();
            ka_clone.linking_number(kb)
        } else {
            0
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ═══════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── Gauss code / knot tests ──

    #[test]
    fn test_linking_number_identical() {
        let mut ka = TransactionKnot::new("alpha".into());
        let tx: [u8; 32] = [42u8; 32];
        ka.add_block_txs(&[tx], false);

        let mut kb = TransactionKnot::new("beta".into());
        kb.add_block_txs(&[tx], false);

        let lk = ka.linking_number(&kb);
        assert_eq!(lk, 1, "Same tx should give linking number 1");
    }

    #[test]
    fn test_linking_number_unrelated() {
        let mut ka = TransactionKnot::new("alpha".into());
        ka.add_block_txs(&[[1u8; 32]], false);

        let mut kb = TransactionKnot::new("beta".into());
        kb.add_block_txs(&[[99u8; 32]], false);

        let lk = ka.linking_number(&kb);
        assert_eq!(lk, 0, "Different txs should give linking number 0");
    }

    #[test]
    fn test_linking_number_coinjoin_negation() {
        let mut ka = TransactionKnot::new("alpha".into());
        let tx: [u8; 32] = [42u8; 32];
        ka.add_block_txs(&[tx], false);  // sign +1

        let mut kb = TransactionKnot::new("beta".into());
        kb.add_block_txs(&[tx], true);   // sign -1

        let lk = ka.linking_number(&kb);
        assert_eq!(lk, -1, "CoinJoin tx should give negative linking number");
    }

    #[test]
    fn test_entanglement_weight_normalized() {
        let mut ka = TransactionKnot::new("alpha".into());
        for i in 0..10 {
            ka.add_block_txs(&[[i; 32]], false);
        }

        let mut kb = TransactionKnot::new("beta".into());
        // Share 5 of the same txs
        for i in 0..5 {
            kb.add_block_txs(&[[i; 32]], false);
        }
        // Add 5 unique txs
        for i in 10..15 {
            kb.add_block_txs(&[[i; 32]], false);
        }

        let weight = ka.entanglement_weight(&kb);
        assert!((weight - 0.5).abs() < 0.01, "5/10 shared = weight 0.5, got {}", weight);
    }

    #[test]
    fn test_linking_cache_invalidation() {
        let mut ka = TransactionKnot::new("alpha".into());
        let tx: [u8; 32] = [42u8; 32];
        ka.add_block_txs(&[tx], false);

        let mut kb = TransactionKnot::new("beta".into());
        kb.add_block_txs(&[tx], false);

        let lk1 = ka.linking_number(&kb);
        assert_eq!(lk1, 1);

        // Add more txs to ka — should invalidate cache
        ka.add_block_txs(&[[99u8; 32]], false);
        assert!(ka.linking_cache.is_empty(), "Cache should be cleared after mutation");

        let lk2 = ka.linking_number(&kb);
        assert_eq!(lk2, 1, "Should still have 1 shared tx");
    }

    #[test]
    fn test_empty_knots() {
        let mut ka = TransactionKnot::new("alpha".into());
        let kb = TransactionKnot::new("beta".into());
        assert_eq!(ka.linking_number(&kb), 0);
        assert_eq!(ka.entanglement_weight(&kb), 0.0);
    }

    // ── Bloom fallback tests ──

    #[test]
    fn test_fingerprint_insert() {
        let mut fp = TxFingerprint::new("alpha".into(), 1024);
        let tx: [u8; 32] = [1u8; 32];
        fp.insert(&tx);
        assert_eq!(fp.tx_count, 1);
        assert!(fp.bit_count() > 0);
    }

    #[test]
    fn test_self_entanglement() {
        let mut router = EntanglementRouter::new(EntanglementConfig::default());
        let tx: [u8; 32] = [1u8; 32];
        router.update_peer("alpha", &[tx]);
        assert!((router.entanglement_score("alpha", "alpha") - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_unrelated_peers() {
        let mut router = EntanglementRouter::new(EntanglementConfig::default());
        let tx_a: [u8; 32] = [1u8; 32];
        let tx_b: [u8; 32] = [99u8; 32];
        router.update_peer("alpha", &[tx_a]);
        router.update_peer("beta", &[tx_b]);
        let score = router.entanglement_score("alpha", "beta");
        assert!(score < 0.5, "Unrelated peers should have low entanglement");
    }

    #[test]
    fn test_related_peers() {
        let mut router = EntanglementRouter::new(EntanglementConfig::default());
        let tx: [u8; 32] = [42u8; 32];
        for _ in 0..50 {
            router.update_peer("alpha", &[tx]);
            router.update_peer("beta", &[tx]);
        }
        let score = router.entanglement_score("alpha", "beta");
        assert!(score > 0.5, "Peers with shared tx should have high entanglement");
    }

    #[test]
    fn test_top_peers() {
        let mut router = EntanglementRouter::new(EntanglementConfig::default());
        let shared: [u8; 32] = [42u8; 32];
        let unique: [u8; 32] = [99u8; 32];
        router.update_peer("alpha", &[shared]);
        router.update_peer("beta", &[shared]);
        router.update_peer("gamma", &[unique]);
        let top = router.top_entangled_peers("alpha", 2);
        assert!(!top.is_empty());
        if top.len() >= 2 {
            assert!(top[0].1 >= top[1].1);
        }
    }

    #[test]
    fn test_disabled_router() {
        let mut config = EntanglementConfig::default();
        config.enabled = false;
        let mut router = EntanglementRouter::new(config);
        let tx: [u8; 32] = [1u8; 32];
        router.update_peer("alpha", &[tx]);
        assert_eq!(router.peer_count(), 0);
    }

    #[test]
    fn test_knot_layer_preferred_over_bloom() {
        let mut config = EntanglementConfig::default();
        config.prefer_knot_routing = true;
        let mut router = EntanglementRouter::new(config);

        // Feed Bloom with no correlation
        router.update_peer("alpha", &[[1u8; 32]]);
        router.update_peer("beta", &[[99u8; 32]]);

        // Feed knots with strong correlation
        let mut ka = TransactionKnot::new("alpha".into());
        ka.add_block_txs(&[[42u8; 32], [43u8; 32], [44u8; 32]], false);
        let mut kb = TransactionKnot::new("beta".into());
        kb.add_block_txs(&[[42u8; 32], [43u8; 32]], false);
        router.update_knot(ka);
        router.update_knot(kb);

        // Score should be ~0.66 (2 shared / 3 total), driven by knot layer
        let score = router.entanglement_score("alpha", "beta");
        assert!(score > 0.5, "Knot layer should dominate, got {}", score);
    }

    #[test]
    fn test_feed_block_updates_both_layers() {
        let mut router = EntanglementRouter::new(EntanglementConfig::default());
        let tx: [u8; 32] = [42u8; 32];
        router.feed_block("alpha", &[tx], false);

        // Should have both knot and Bloom data
        assert!(router.knots.contains_key("alpha"));
        assert!(router.fingerprints.contains_key("alpha"));
    }

    #[test]
    fn test_gauss_code_pruning() {
        let mut knot = TransactionKnot::new("alpha".into());
        // Insert more than max_crossings (500)
        for i in 0..600 {
            knot.add_block_txs(&[[i as u8; 32]], false);
        }
        assert!(knot.gauss_code.len() <= 500, "Should prune to max 500 crossings");
    }
}
