//! Vizily-style query intelligence, Flux-native and dependency-light.
//!
//! This ports the practical pieces from Vizily's spell correction and synonym
//! manager without dragging in Redis, notify, or fuzzy-matcher. The index learns
//! vocabulary from documents as they are added, then uses edit distance and a
//! small built-in synonym table to expand low-signal queries.

use std::collections::{BTreeSet, HashMap};

const MAX_DICTIONARY_TERMS: usize = 50_000;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct QueryTerm {
    pub term: String,
    pub weight: f64,
    pub source: QueryTermSource,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum QueryTermSource {
    Original,
    Correction,
    Synonym,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct QueryPlan {
    pub original_query: String,
    pub corrected_query: Option<String>,
    pub terms: Vec<QueryTerm>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct QueryIntelligence {
    dictionary: HashMap<String, u32>,
    synonyms: HashMap<String, Vec<String>>,
    max_edit_distance: usize,
}

impl QueryIntelligence {
    pub fn new() -> Self {
        let mut qi = QueryIntelligence {
            dictionary: HashMap::new(),
            synonyms: HashMap::new(),
            max_edit_distance: 2,
        };
        qi.seed_common_terms();
        qi.seed_synonyms();
        qi
    }

    pub fn observe_document(&mut self, title: &str, content: &str) {
        let sample = content.chars().take(40_000).collect::<String>();
        for token in super::tokenize(&format!("{title} {sample}")) {
            if !should_learn_token(&token) {
                continue;
            }
            self.add_word(&token, 1);
        }
    }

    pub fn add_word(&mut self, word: &str, frequency: u32) {
        if word.len() < 2 {
            return;
        }
        let normalized = word.to_lowercase();
        if !self.dictionary.contains_key(&normalized)
            && self.dictionary.len() >= MAX_DICTIONARY_TERMS
            && frequency < 100
        {
            return;
        }
        *self.dictionary.entry(normalized).or_insert(0) += frequency.max(1);
    }

    pub fn add_synonym_group<I, S>(&mut self, words: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let group: Vec<String> = words
            .into_iter()
            .map(|w| w.into().to_lowercase())
            .filter(|w| w.len() >= 2)
            .collect();
        for word in &group {
            let mut peers = group.clone();
            peers.retain(|p| p != word);
            self.synonyms.entry(word.clone()).or_default().extend(peers);
            self.synonyms.get_mut(word).unwrap().sort();
            self.synonyms.get_mut(word).unwrap().dedup();
        }
    }

    pub fn plan(&self, query: &str) -> QueryPlan {
        let original_terms = super::tokenize(query);
        let corrected_terms: Vec<String> = original_terms
            .iter()
            .map(|term| self.correct_word(term).unwrap_or_else(|| term.clone()))
            .collect();

        let corrected_query = if corrected_terms != original_terms && !corrected_terms.is_empty() {
            Some(corrected_terms.join(" "))
        } else {
            None
        };

        let mut seen = BTreeSet::new();
        let mut terms = Vec::new();

        for term in &original_terms {
            push_term(&mut terms, &mut seen, term, 1.0, QueryTermSource::Original);
        }

        for term in &corrected_terms {
            if !original_terms.iter().any(|t| t == term) {
                push_term(&mut terms, &mut seen, term, 0.88, QueryTermSource::Correction);
            }
        }

        for term in original_terms.iter().chain(corrected_terms.iter()) {
            if let Some(synonyms) = self.synonyms.get(term) {
                for synonym in synonyms.iter().take(4) {
                    push_term(&mut terms, &mut seen, synonym, 0.62, QueryTermSource::Synonym);
                }
            }
        }

        QueryPlan {
            original_query: query.to_string(),
            corrected_query,
            terms,
        }
    }

    pub fn correct_word(&self, word: &str) -> Option<String> {
        let word = word.to_lowercase();
        if self.dictionary.contains_key(&word) {
            return Some(word);
        }
        if let Some(common) = common_typo(&word) {
            return Some(common.to_string());
        }

        let mut best: Vec<(&str, usize, u32)> = Vec::new();
        for (candidate, freq) in &self.dictionary {
            if candidate.chars().next() != word.chars().next() {
                continue;
            }
            if candidate.len().abs_diff(word.len()) > self.max_edit_distance {
                continue;
            }
            let distance = edit_distance(&word, candidate);
            if distance <= self.max_edit_distance {
                best.push((candidate.as_str(), distance, *freq));
            }
        }

        best.sort_by(|a, b| {
            common_rank(b.2).cmp(&common_rank(a.2))
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| b.2.cmp(&a.2))
        });
        best.first().map(|(candidate, _, _)| (*candidate).to_string())
    }

    pub fn dictionary_size(&self) -> usize {
        self.dictionary.len()
    }

    pub fn synonym_groups(&self) -> usize {
        self.synonyms.len()
    }

    fn seed_common_terms(&mut self) {
        let terms = [
            ("rust", 900), ("flux", 900), ("search", 900), ("mcp", 850),
            ("query", 700), ("index", 700), ("pagerank", 650), ("ranking", 650),
            ("crawler", 600), ("crawl", 600), ("distributed", 600), ("p2p", 600),
            ("gossipsub", 550), ("libp2p", 550), ("semantic", 500), ("vector", 500),
            ("embedding", 500), ("spell", 350), ("synonym", 350), ("bm25", 500),
            ("tantivy", 450), ("postgres", 350), ("redis", 350), ("simd", 500),
            ("quality", 500), ("freshness", 450), ("snippet", 450), ("agent", 450),
            ("swarm", 450), ("wallet", 300), ("sigil", 350), ("quillon", 350),
        ];
        for (word, freq) in terms {
            self.add_word(word, freq);
        }
    }

    fn seed_synonyms(&mut self) {
        let groups = [
            &["p2p", "peer", "peers", "libp2p", "gossipsub"][..],
            &["search", "query", "find", "lookup", "retrieve"][..],
            &["index", "catalog", "corpus", "documents"][..],
            &["crawl", "crawler", "spider", "fetch"][..],
            &["semantic", "vector", "embedding", "similarity"][..],
            &["ranking", "rank", "score", "relevance"][..],
            &["mcp", "tool", "combo", "agent"][..],
            &["fresh", "freshness", "recent", "recency"][..],
        ];
        for group in groups {
            self.add_synonym_group(group.iter().copied());
        }
    }
}

impl Default for QueryIntelligence {
    fn default() -> Self {
        Self::new()
    }
}

fn push_term(
    terms: &mut Vec<QueryTerm>,
    seen: &mut BTreeSet<String>,
    term: &str,
    weight: f64,
    source: QueryTermSource,
) {
    let term = term.to_lowercase();
    if seen.insert(term.clone()) {
        terms.push(QueryTerm { term, weight, source });
    }
}

fn should_learn_token(token: &str) -> bool {
    if token.len() < 2 || token.len() > 32 {
        return false;
    }
    if !token.chars().any(|c| c.is_alphabetic()) {
        return false;
    }
    let hexish = token.chars().filter(|c| c.is_ascii_hexdigit()).count();
    if token.len() >= 12 && hexish * 100 / token.len() > 80 {
        return false;
    }
    true
}

fn common_rank(freq: u32) -> u8 {
    if freq >= 300 { 2 } else if freq >= 25 { 1 } else { 0 }
}

fn common_typo(word: &str) -> Option<&'static str> {
    match word {
        "serach" | "seach" | "searc" => Some("search"),
        "querry" | "qury" => Some("query"),
        "cralwer" | "crawlr" => Some("crawler"),
        "distirbuted" | "distribued" => Some("distributed"),
        "libpp" | "lip2p" => Some("libp2p"),
        "gossipsup" => Some("gossipsub"),
        _ => None,
    }
}

fn edit_distance(a: &str, b: &str) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0; b.len() + 1];

    for (i, ca) in a.bytes().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.bytes().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrects_common_flux_typo() {
        let qi = QueryIntelligence::new();
        assert_eq!(qi.correct_word("serach").as_deref(), Some("search"));
    }

    #[test]
    fn expands_p2p_terms() {
        let qi = QueryIntelligence::new();
        let plan = qi.plan("p2p search");
        assert!(plan.terms.iter().any(|t| t.term == "libp2p"));
        assert!(plan.terms.iter().any(|t| t.term == "query"));
    }
}
