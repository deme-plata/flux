// Flux Search — PageRank Calculator
//
// Ported from vizily-search pagerank.rs.
// Uses a directed graph with damping_factor 0.85, iterative convergence.
// BLAKE3-native: document URLs are hashed for node identity.

use std::collections::HashMap;

/// A directed graph node index.
pub type NodeIndex = usize;

/// PageRank calculator — iterative algorithm on directed link graph.
#[derive(Clone, Debug)]
pub struct PageRankCalculator {
    /// Node URLs.
    node_urls: Vec<String>,
    /// URL → node index.
    url_to_index: HashMap<String, NodeIndex>,
    /// Adjacency list: source_index → [(target_index, weight)]
    out_edges: Vec<Vec<(NodeIndex, f64)>>,
    /// Incoming edges (for efficient PageRank): target_index → [(source_index, weight)]
    in_edges: Vec<Vec<(NodeIndex, f64)>>,
    /// Damping factor (probability of following a link).
    damping_factor: f64,
    /// Convergence threshold.
    convergence_threshold: f64,
    /// Maximum iterations.
    max_iterations: usize,
}

impl PageRankCalculator {
    /// Create a new PageRank calculator with default parameters.
    pub fn new() -> Self {
        PageRankCalculator {
            node_urls: Vec::new(),
            url_to_index: HashMap::new(),
            out_edges: Vec::new(),
            in_edges: Vec::new(),
            damping_factor: 0.85,
            convergence_threshold: 1e-6,
            max_iterations: 100,
        }
    }

    /// Create with custom damping factor.
    pub fn with_damping_factor(mut self, d: f64) -> Self {
        self.damping_factor = d;
        self
    }

    /// Create with custom convergence threshold.
    pub fn with_convergence_threshold(mut self, t: f64) -> Self {
        self.convergence_threshold = t;
        self
    }

    /// Create with custom max iterations.
    pub fn with_max_iterations(mut self, n: usize) -> Self {
        self.max_iterations = n;
        self
    }

    /// Add a page to the graph. Returns its node index.
    pub fn add_page(&mut self, url: &str) -> NodeIndex {
        if let Some(&idx) = self.url_to_index.get(url) {
            return idx;
        }
        let idx = self.node_urls.len();
        self.node_urls.push(url.to_string());
        self.url_to_index.insert(url.to_string(), idx);
        self.out_edges.push(Vec::new());
        self.in_edges.push(Vec::new());
        idx
    }

    /// Add a directed link with weight.
    pub fn add_link(&mut self, from_url: &str, to_url: &str, weight: f64) {
        let from = self.add_page(from_url);
        let to = self.add_page(to_url);

        // Update or insert out-edge
        if let Some(existing) = self.out_edges[from].iter_mut().find(|(t, _)| *t == to) {
            existing.1 = weight;
        } else {
            self.out_edges[from].push((to, weight));
        }

        // Update or insert in-edge
        if let Some(existing) = self.in_edges[to].iter_mut().find(|(s, _)| *s == from) {
            existing.1 = weight;
        } else {
            self.in_edges[to].push((from, weight));
        }
    }

    /// Calculate PageRank values for all pages.
    /// Returns URL → PageRank score (0.0–1.0, sum = 1.0).
    pub fn calculate_pagerank(&self) -> Result<HashMap<String, f64>, String> {
        let n = self.node_urls.len();
        if n == 0 {
            return Ok(HashMap::new());
        }

        // Initialize: equal rank for all nodes
        let initial_rank = 1.0 / n as f64;
        let mut ranks: Vec<f64> = vec![initial_rank; n];

        // Out-degree counts (cached for efficiency)
        let out_degrees: Vec<usize> = (0..n)
            .map(|i| self.out_edges[i].len())
            .collect();

        for _iteration in 0..self.max_iterations {
            let mut new_ranks = vec![(1.0 - self.damping_factor) / n as f64; n];

            for target in 0..n {
                for &(source, weight) in &self.in_edges[target] {
                    let source_rank = ranks[source];
                    let out_count = out_degrees[source];

                    if out_count > 0 {
                        new_ranks[target] += self.damping_factor * (source_rank * weight) / out_count as f64;
                    } else {
                        // Dangling node: distribute rank to all nodes
                        new_ranks[target] += self.damping_factor * source_rank / n as f64;
                    }
                }
            }

            // Check convergence
            let max_change: f64 = ranks.iter()
                .zip(new_ranks.iter())
                .map(|(old, new)| (new - old).abs())
                .fold(0.0, f64::max);

            ranks = new_ranks;

            if max_change < self.convergence_threshold {
                break;
            }
        }

        // Normalize so sum = 1.0
        let total: f64 = ranks.iter().sum();
        if total > 0.0 {
            for r in &mut ranks {
                *r /= total;
            }
        }

        // Build result map
        let result: HashMap<String, f64> = self.node_urls.iter()
            .enumerate()
            .map(|(i, url)| (url.clone(), ranks[i]))
            .collect();

        Ok(result)
    }

    /// Get the top N pages by PageRank.
    pub fn get_top_pages(&self, ranks: &HashMap<String, f64>, limit: usize) -> Vec<(String, f64)> {
        let mut ranked: Vec<(String, f64)> = ranks.iter()
            .map(|(url, &rank)| (url.clone(), rank))
            .collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        ranked.truncate(limit);
        ranked
    }

    /// Number of nodes in the graph.
    pub fn node_count(&self) -> usize {
        self.node_urls.len()
    }

    /// Total number of edges.
    pub fn edge_count(&self) -> usize {
        self.out_edges.iter().map(|e| e.len()).sum()
    }

    /// Clear the graph.
    pub fn clear(&mut self) {
        self.node_urls.clear();
        self.url_to_index.clear();
        self.out_edges.clear();
        self.in_edges.clear();
    }
}

impl Default for PageRankCalculator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_pagerank() {
        let mut calc = PageRankCalculator::new();

        // A → B → C → A (symmetric cycle)
        calc.add_link("A", "B", 1.0);
        calc.add_link("B", "C", 1.0);
        calc.add_link("C", "A", 1.0);

        let ranks = calc.calculate_pagerank().unwrap();
        assert_eq!(ranks.len(), 3);

        // All pages should have approximately equal rank
        for (_, &rank) in &ranks {
            assert!((rank - 1.0 / 3.0).abs() < 0.01,
                "Expected ~0.333, got {}", rank);
        }
    }

    #[test]
    fn test_authority_page() {
        let mut calc = PageRankCalculator::new();

        // D is linked to by A, B, C → D should have higher rank
        calc.add_link("A", "D", 1.0);
        calc.add_link("B", "D", 1.0);
        calc.add_link("C", "D", 1.0);
        calc.add_link("D", "A", 1.0);

        let ranks = calc.calculate_pagerank().unwrap();
        let d_rank = ranks.get("D").unwrap();
        let a_rank = ranks.get("A").unwrap();

        assert!(d_rank > a_rank,
            "Authority page D ({} linked) should rank higher than A ({})",
            d_rank, a_rank);
    }

    #[test]
    fn test_dangling_node() {
        let mut calc = PageRankCalculator::new();

        // Dangling node: E has no outgoing links
        calc.add_link("A", "B", 1.0);
        calc.add_link("B", "C", 1.0);
        calc.add_page("E"); // Dangling node

        let ranks = calc.calculate_pagerank().unwrap();
        assert_eq!(ranks.len(), 4);
        // All ranks should still sum to 1.0
        let total: f64 = ranks.values().sum();
        assert!((total - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_top_pages() {
        let mut calc = PageRankCalculator::new();
        calc.add_link("popular", "a", 1.0);
        calc.add_link("popular", "b", 1.0);
        calc.add_link("popular", "c", 1.0);

        let ranks = calc.calculate_pagerank().unwrap();
        let top = calc.get_top_pages(&ranks, 2);
        assert_eq!(top.len(), 2);
    }
}
