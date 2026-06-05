//! Vizily-inspired learning-to-rank scorer.
//!
//! This is intentionally deterministic: no model files, no service calls, no
//! hidden state. It brings over Vizily's feature mix (BM25-ish text relevance,
//! title/phrase match, PageRank, quality, freshness, behavior placeholders) as a
//! Flux-native scoring surface that can later be tuned by MCP feedback.

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SearchSignals {
    pub tf_idf: f64,
    pub semantic: f64,
    pub title_match: f64,
    pub exact_phrase: f64,
    pub term_coverage: f64,
    pub page_rank: f64,
    pub domain_authority: f64,
    pub quality: f64,
    pub freshness: f64,
    pub sap: f64,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct LearningRanker {
    pub tf_idf_weight: f64,
    pub semantic_weight: f64,
    pub title_weight: f64,
    pub phrase_weight: f64,
    pub coverage_weight: f64,
    pub authority_weight: f64,
    pub quality_weight: f64,
    pub freshness_weight: f64,
    pub sap_weight: f64,
}

impl LearningRanker {
    pub fn new() -> Self {
        Self {
            tf_idf_weight: 0.23,
            semantic_weight: 0.16,
            title_weight: 0.12,
            phrase_weight: 0.09,
            coverage_weight: 0.08,
            authority_weight: 0.10,
            quality_weight: 0.08,
            freshness_weight: 0.07,
            sap_weight: 0.07,
        }
    }

    pub fn score(&self, s: &SearchSignals) -> f64 {
        let authority = ((s.page_rank * 5.0).clamp(0.0, 1.0) * 0.65)
            + (s.domain_authority.clamp(0.0, 1.0) * 0.35);
        let raw = s.tf_idf.clamp(0.0, 1.0) * self.tf_idf_weight
            + s.semantic.clamp(0.0, 1.0) * self.semantic_weight
            + s.title_match.clamp(0.0, 1.0) * self.title_weight
            + s.exact_phrase.clamp(0.0, 1.0) * self.phrase_weight
            + s.term_coverage.clamp(0.0, 1.0) * self.coverage_weight
            + authority * self.authority_weight
            + s.quality.clamp(0.0, 1.0) * self.quality_weight
            + s.freshness.clamp(0.0, 1.0) * self.freshness_weight
            + s.sap.clamp(0.0, 1.0) * self.sap_weight;
        raw.clamp(0.0, 1.0)
    }
}

impl Default for LearningRanker {
    fn default() -> Self {
        Self::new()
    }
}

pub fn title_match_score(query: &str, title: &str) -> f64 {
    let q = query.to_lowercase();
    let t = title.to_lowercase();
    if q.is_empty() || t.is_empty() {
        return 0.0;
    }
    if t == q {
        return 1.0;
    }
    if t.contains(&q) {
        return 0.82;
    }
    let q_terms = crate::tokenize(&q);
    if q_terms.is_empty() {
        return 0.0;
    }
    let matched = q_terms.iter().filter(|term| t.contains(term.as_str())).count();
    matched as f64 / q_terms.len() as f64
}

pub fn exact_phrase_score(query: &str, content: &str) -> f64 {
    let q = query.trim().to_lowercase();
    if q.len() < 3 {
        return 0.0;
    }
    let haystack = content.to_lowercase();
    if haystack.contains(&q) { 1.0 } else { 0.0 }
}

pub fn term_coverage(query_terms: &[String], title: &str, content: &str) -> f64 {
    if query_terms.is_empty() {
        return 0.0;
    }
    let haystack = format!("{} {}", title, content).to_lowercase();
    let matched = query_terms
        .iter()
        .filter(|term| haystack.contains(term.as_str()))
        .count();
    matched as f64 / query_terms.len() as f64
}

pub fn domain_authority(url: &str) -> f64 {
    let domain = url
        .split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url)
        .to_lowercase();
    if domain.contains("wikipedia.org") || domain.ends_with(".gov") {
        0.95
    } else if domain.contains("github.com") || domain.contains("docs.rs") || domain.ends_with(".edu") {
        0.82
    } else if domain.contains("arxiv") || domain.contains("research") {
        0.78
    } else if domain.contains("quillon") || domain.contains("flux") || domain.contains("vizily") {
        0.72
    } else {
        0.50
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_exact_match_scores_high() {
        assert!(title_match_score("flux search", "Flux Search") > 0.9);
    }

    #[test]
    fn ranker_prefers_semantic_and_phrase_hits() {
        let ranker = LearningRanker::new();
        let weak = SearchSignals {
            tf_idf: 0.2, semantic: 0.1, title_match: 0.0, exact_phrase: 0.0,
            term_coverage: 0.2, page_rank: 0.1, domain_authority: 0.5,
            quality: 0.5, freshness: 0.5, sap: 0.5,
        };
        let strong = SearchSignals { semantic: 0.9, title_match: 0.8, exact_phrase: 1.0, ..weak.clone() };
        assert!(ranker.score(&strong) > ranker.score(&weak));
    }
}
