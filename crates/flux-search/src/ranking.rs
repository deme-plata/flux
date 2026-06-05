// Flux Search — Ranking Engine
//
// Ported from vizily-search ranking.rs.
// Combines TF-IDF, PageRank, content quality, freshness into final score.
// SAP/X-Algo boost applied as a fifth scoring dimension.

use std::collections::HashMap;

/// Ranking weights — configurable per search domain.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RankingWeights {
    pub tf_idf_weight: f64,
    pub page_rank_weight: f64,
    pub freshness_weight: f64,
    pub quality_weight: f64,
    pub sap_weight: f64,
}

impl Default for RankingWeights {
    fn default() -> Self {
        RankingWeights {
            tf_idf_weight: 0.25,
            page_rank_weight: 0.25,
            freshness_weight: 0.20,
            quality_weight: 0.20,
            sap_weight: 0.10,
        }
    }
}

/// All ranking factors for a single result.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RankingFactors {
    pub tf_idf_score: f64,
    pub page_rank_score: f64,
    pub freshness_score: f64,
    pub quality_score: f64,
    pub sap_boost: f64,
    pub final_score: f64,
}

/// The ranking engine — computes final relevance scores.
#[derive(Clone, Debug)]
pub struct RankingEngine {
    weights: RankingWeights,
}

impl RankingEngine {
    /// Create a new ranking engine with default weights.
    pub fn new() -> Self {
        RankingEngine {
            weights: RankingWeights::default(),
        }
    }

    /// Create with custom weights.
    pub fn with_weights(weights: RankingWeights) -> Self {
        RankingEngine { weights }
    }

    /// Compute all ranking factors and return the final score.
    pub fn compute_factors(
        &self,
        tf_idf: f64,
        page_rank: f64,
        freshness: f64,
        quality: f64,
        sap_boost: f64,
    ) -> RankingFactors {
        // Normalize TF-IDF into 0-1 range
        let tfidf_norm = tf_idf.min(1.0).max(0.0);

        // Normalize PageRank (multiply by 5, clamp to 0-1)
        let pr_norm = (page_rank * 5.0).min(1.0).max(0.0);

        let final_score = tfidf_norm * self.weights.tf_idf_weight
            + pr_norm * self.weights.page_rank_weight
            + freshness * self.weights.freshness_weight
            + quality * self.weights.quality_weight
            + sap_boost * self.weights.sap_weight;

        RankingFactors {
            tf_idf_score: tfidf_norm,
            page_rank_score: pr_norm,
            freshness_score: freshness,
            quality_score: quality,
            sap_boost,
            final_score: final_score.min(1.0).max(0.0),
        }
    }

    /// Apply a category boost to results.
    /// `category`: "images" boosts image URLs, "news" boosts recent content.
    pub fn apply_category_boost(
        &self,
        scores: &mut [(f64, &str, Option<u64>)], // (score, url, last_crawled_ms)
        category: &str,
    ) {
        let now_ms = super::now_ms();

        for (score, url, last_crawled) in scores.iter_mut() {
            let boost = match category {
                "images" => {
                    if url.contains("/images/") || url.ends_with(".jpg") || url.ends_with(".png") {
                        0.2
                    } else {
                        0.0
                    }
                }
                "news" => {
                    if url.contains("/news/") || url.contains("/articles/") {
                        0.3
                    } else if let Some(lc) = last_crawled {
                        let age_days = (now_ms - *lc) as f64 / 86_400_000.0;
                        if age_days < 7.0 { 0.2 } else { 0.0 }
                    } else {
                        0.0
                    }
                }
                "tech" => {
                    if url.contains("github.com") || url.contains("docs.rs") {
                        0.15
                    } else {
                        0.0
                    }
                }
                _ => 0.0,
            };
            *score += boost;
        }

        // Re-sort by boosted score
        scores.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    }

    /// Compute TF-IDF score for a single document against query terms.
    pub fn compute_tf_idf(
        &self,
        doc_content: &str,
        query_terms: &[String],
        corpus_size: usize,
        term_doc_freqs: &HashMap<String, usize>,
    ) -> f64 {
        let doc_lower = doc_content.to_lowercase();
        let doc_words: Vec<&str> = doc_lower.split_whitespace().collect();
        let doc_len = doc_words.len().max(1) as f64;

        let mut score = 0.0;

        for term in query_terms {
            let term_lower = term.to_lowercase();

            // TF: term frequency in document
            let term_count = doc_words.iter().filter(|&&w| w == term_lower).count() as f64;
            let tf = term_count / doc_len;

            // IDF: inverse document frequency
            let doc_freq = term_doc_freqs.get(&term_lower).copied().unwrap_or(0).max(1) as f64;
            let idf = ((corpus_size as f64 + 1.0) / (doc_freq + 1.0)).ln();

            score += tf * idf;
        }

        score
    }

    /// Get the current weights.
    pub fn weights(&self) -> &RankingWeights {
        &self.weights
    }

    /// Update weights (e.g., based on ML optimization).
    pub fn set_weights(&mut self, weights: RankingWeights) {
        self.weights = weights;
    }
}

impl Default for RankingEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_factors() {
        let engine = RankingEngine::new();

        let factors = engine.compute_factors(
            0.8,  // tf_idf
            0.5,  // page_rank
            0.9,  // freshness
            0.7,  // quality
            0.6,  // sap_boost
        );

        // Expected: 0.8*0.25 + (0.5*5).min(1)*0.25 + 0.9*0.20 + 0.7*0.20 + 0.6*0.10
        // = 0.20 + 0.25 + 0.18 + 0.14 + 0.06 = 0.83
        assert!((factors.final_score - 0.83).abs() < 0.02,
            "Expected ~0.83, got {}", factors.final_score);
        assert!(factors.sap_boost == 0.6);
    }

    #[test]
    fn test_category_boost_images() {
        let engine = RankingEngine::new();
        let mut data = vec![
            (0.5, "https://example.com/images/photo.jpg", None),
            (0.5, "https://example.com/text/article", None),
        ];

        engine.apply_category_boost(&mut data, "images");

        // Image URL should now rank higher
        assert!(data[0].0 > data[1].0,
            "Image URL should get a boost for 'images' category");
    }

    #[test]
    fn test_tf_idf_computation() {
        let engine = RankingEngine::new();
        let mut term_freqs = HashMap::new();
        term_freqs.insert("rust".to_string(), 3usize);
        term_freqs.insert("systems".to_string(), 5usize);

        let score = engine.compute_tf_idf(
            "Rust is a systems programming language for safe systems programming",
            &["rust".to_string(), "systems".to_string()],
            10,
            &term_freqs,
        );

        // Should return a positive score
        assert!(score > 0.0, "TF-IDF score should be positive for matching terms");
    }

    #[test]
    fn test_custom_weights() {
        let weights = RankingWeights {
            tf_idf_weight: 0.5,
            page_rank_weight: 0.2,
            freshness_weight: 0.1,
            quality_weight: 0.1,
            sap_weight: 0.1,
        };
        let engine = RankingEngine::with_weights(weights);

        let factors = engine.compute_factors(1.0, 0.0, 0.0, 0.0, 0.0);
        assert!((factors.final_score - 0.5).abs() < 0.01,
            "With TF-IDF weight 0.5 and score 1.0, final should be ~0.5");
    }
}
