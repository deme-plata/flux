// Flux Search - BLAKE3-native search engine with Vizily-derived ranking,
// query intelligence, semantic fallback, PageRank, SAP scoring, and persistence.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_INDEXED_CONTENT_CHARS: usize = 200_000;

pub mod benches;
pub mod facets;
pub mod mcp_tap;
pub mod ml_ranker;
pub mod pagerank;
pub mod query_intel;
pub mod ranking;
pub mod secret_scrape;
pub mod semantic;

pub use facets::{aggregate as aggregate_facets, FacetEntry, Facets};
pub use mcp_tap::{doc_from_broadcast, doc_from_settled, doc_from_tool_call, McpToolCall, SettledTask, SwarmBroadcast};
pub use ml_ranker::{LearningRanker, SearchSignals};
pub use pagerank::PageRankCalculator;
pub use query_intel::{QueryIntelligence, QueryPlan, QueryTerm, QueryTermSource};
pub use ranking::{RankingEngine, RankingFactors, RankingWeights};
pub use secret_scrape::{classify_pattern, redact_args, redact_string, SecretPattern};
pub use semantic::{DocumentEmbedding, SemanticIndex};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Document {
    pub id: String,
    pub url: String,
    pub title: String,
    pub content: String,
    pub meta_description: Option<String>,
    pub language: Option<String>,
    pub category: Option<String>,
    pub page_rank: f64,
    pub readability_score: f64,
    pub word_count: usize,
    pub last_crawled: Option<u64>,
    pub content_hash: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SearchResult {
    pub doc_id: String,
    pub url: String,
    pub title: String,
    pub snippet: String,
    pub score: f64,
    pub ranking_factors: Option<RankingFactors>,
    pub meta_description: Option<String>,
    pub last_crawled: Option<u64>,
    /// 1-based line number of the best-matching line in the document content
    /// (grep-style). `None` for non-line-oriented or empty content.
    #[serde(default)]
    pub line: Option<u32>,
    /// The trimmed text of that line — so callers get the actual code, not just
    /// a filename + rank. This is the single biggest gap vs ripgrep.
    #[serde(default)]
    pub line_text: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SearchQuery {
    pub q: String,
    pub page: usize,
    pub per_page: usize,
    pub category: Option<String>,
    pub language: Option<String>,
}

impl Default for SearchQuery {
    fn default() -> Self {
        SearchQuery {
            q: String::new(),
            page: 1,
            per_page: 10,
            category: None,
            language: None,
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
    pub total_results: usize,
    pub page: usize,
    pub total_pages: usize,
    pub query_time_ms: u64,
    pub corrected_query: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct LinkSnapshot {
    pub from_url: String,
    pub to_url: String,
    pub weight: f64,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SearchSnapshot {
    pub version: u32,
    pub generated_ms: u64,
    pub documents: Vec<Document>,
    pub links: Vec<LinkSnapshot>,
    pub sap_domain_scores: HashMap<String, f64>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SearchIndexStats {
    pub documents: usize,
    pub terms: usize,
    pub links: usize,
    pub semantic_embeddings: usize,
    pub dictionary_terms: usize,
    pub synonym_terms: usize,
}

pub struct SearchEngine {
    documents: HashMap<String, Document>,
    inverted_index: HashMap<String, HashMap<String, u32>>,
    doc_count: usize,
    pagerank: PageRankCalculator,
    ranker: RankingEngine,
    link_graph: HashMap<String, Vec<(String, f64)>>,
    cache: HashMap<String, (SearchResponse, u64)>,
    sap_domain_scores: HashMap<String, f64>,
    query_intel: QueryIntelligence,
    semantic: SemanticIndex,
    learning_ranker: LearningRanker,
}

impl SearchEngine {
    pub fn new() -> Self {
        SearchEngine {
            documents: HashMap::new(),
            inverted_index: HashMap::new(),
            doc_count: 0,
            pagerank: PageRankCalculator::new(),
            ranker: RankingEngine::new(),
            link_graph: HashMap::new(),
            cache: HashMap::new(),
            sap_domain_scores: HashMap::new(),
            query_intel: QueryIntelligence::new(),
            semantic: SemanticIndex::new(),
            learning_ranker: LearningRanker::new(),
        }
    }

    pub fn load_or_new<P: AsRef<Path>>(path: P) -> Self {
        Self::load_from_path(path).unwrap_or_else(|_| Self::new())
    }

    pub fn load_from_path<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let data = fs::read_to_string(path.as_ref()).map_err(|e| format!("read index: {e}"))?;
        let snapshot: SearchSnapshot = serde_json::from_str(&data).map_err(|e| format!("parse index: {e}"))?;
        let mut engine = SearchEngine::new();
        engine.sap_domain_scores = snapshot.sap_domain_scores;
        for doc in snapshot.documents {
            engine.index_document(doc);
        }
        for link in snapshot.links {
            engine.add_link(&link.from_url, &link.to_url, link.weight);
        }
        engine.recalculate_pagerank();
        Ok(engine)
    }

    pub fn save_to_path<P: AsRef<Path>>(&self, path: P) -> Result<(), String> {
        if let Some(parent) = path.as_ref().parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create index dir: {e}"))?;
        }
        let json = serde_json::to_string_pretty(&self.snapshot()).map_err(|e| format!("serialize index: {e}"))?;
        fs::write(path.as_ref(), json).map_err(|e| format!("write index: {e}"))
    }

    pub fn snapshot(&self) -> SearchSnapshot {
        let mut documents: Vec<Document> = self.documents.values().cloned().collect();
        documents.sort_by(|a, b| a.url.cmp(&b.url));

        let mut links = Vec::new();
        for (from_url, outs) in &self.link_graph {
            for (to_url, weight) in outs {
                links.push(LinkSnapshot {
                    from_url: from_url.clone(),
                    to_url: to_url.clone(),
                    weight: *weight,
                });
            }
        }
        links.sort_by(|a, b| a.from_url.cmp(&b.from_url).then_with(|| a.to_url.cmp(&b.to_url)));

        SearchSnapshot {
            version: 1,
            generated_ms: now_ms(),
            documents,
            links,
            sap_domain_scores: self.sap_domain_scores.clone(),
        }
    }

    pub fn index_path<P: AsRef<Path>>(&mut self, path: P, recursive: bool) -> Result<usize, String> {
        let root = path.as_ref().to_path_buf();
        // Respect .gitignore directory entries (ripgrep's headline behavior) so the
        // index isn't polluted with build artifacts / vendored trees, on top of the
        // hardcoded skips. Dependency-free: simple bare-name / `name/` patterns only.
        let gitignored_dirs = collect_gitignored_dirs(&root);
        let mut count = 0usize;
        let mut stack = vec![root];

        while let Some(path) = stack.pop() {
            if path.is_dir() {
                let entries = fs::read_dir(&path).map_err(|e| format!("read_dir {}: {e}", path.display()))?;
                for entry in entries.flatten() {
                    let child = entry.path();
                    if child.is_dir()
                        && recursive
                        && !is_ignored_dir(&child)
                        && !dir_is_gitignored(&child, &gitignored_dirs)
                    {
                        stack.push(child);
                    } else if child.is_file() && is_indexable_file(&child) {
                        if self.index_file(&child).is_ok() {
                            count += 1;
                        }
                    }
                }
            } else if path.is_file() && is_indexable_file(&path) {
                self.index_file(&path)?;
                count += 1;
            }
        }

        Ok(count)
    }

    pub fn index_file<P: AsRef<Path>>(&mut self, path: P) -> Result<(), String> {
        let path = path.as_ref();
        let raw_content = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let content_hash = blake3::hash(raw_content.as_bytes())
            .to_hex()
            .as_str()
            .chars()
            .take(32)
            .collect();
        let content = searchable_content(&raw_content);
        let title = path.file_name().and_then(|s| s.to_str()).unwrap_or("untitled").to_string();
        let id = path.to_string_lossy().to_string();
        let url = format!("file://{}", path.display());
        let word_count = tokenize(&content).len();

        self.index_document(Document {
            id,
            url,
            title,
            content,
            meta_description: None,
            language: Some("en".into()),
            category: category_from_path(path),
            page_rank: 0.5,
            readability_score: 0.8,
            word_count,
            last_crawled: Some(now_ms()),
            content_hash,
        });
        Ok(())
    }

    pub fn index_document(&mut self, mut doc: Document) {
        if doc.word_count == 0 {
            doc.word_count = tokenize(&doc.content).len();
        }
        if doc.content_hash.is_empty() {
            doc.content_hash = blake3::hash(doc.content.as_bytes())
                .to_hex()
                .as_str()
                .chars()
                .take(32)
                .collect();
        }

        let replacing = self.documents.contains_key(&doc.url);
        self.documents.insert(doc.url.clone(), doc.clone());
        if replacing {
            self.rebuild_runtime_indexes();
        } else {
            self.add_doc_to_runtime(&doc);
            self.doc_count = self.documents.len();
        }
        self.cache.clear();
    }

    /// Bulk-load many documents, then build the runtime indexes ONCE — O(n) total.
    ///
    /// `index_document` does a full O(n) `rebuild_runtime_indexes()` every time it
    /// sees a url it already holds (`replacing == true`). When the source stream
    /// has many duplicate urls (e.g. millions of mining events keyed by the same
    /// wallet/tag in the explorer history), feeding them one-at-a-time degrades to
    /// O(n²) and a restart never finishes. This collapses that to O(n): dedup into
    /// the document map (last write wins, same as `insert`), then rebuild once.
    pub fn bulk_load(&mut self, docs: impl IntoIterator<Item = Document>) {
        for mut doc in docs {
            if doc.word_count == 0 {
                doc.word_count = tokenize(&doc.content).len();
            }
            if doc.content_hash.is_empty() {
                doc.content_hash = blake3::hash(doc.content.as_bytes())
                    .to_hex()
                    .as_str()
                    .chars()
                    .take(32)
                    .collect();
            }
            self.documents.insert(doc.url.clone(), doc);
        }
        self.rebuild_runtime_indexes();
        self.cache.clear();
    }

    pub fn add_link(&mut self, from_url: &str, to_url: &str, weight: f64) {
        self.pagerank.add_link(from_url, to_url, weight);
        self.link_graph.entry(from_url.to_string()).or_default().push((to_url.to_string(), weight));
        self.cache.clear();
    }

    pub fn recalculate_pagerank(&mut self) {
        if let Ok(ranks) = self.pagerank.calculate_pagerank() {
            for (url, rank) in &ranks {
                if let Some(doc) = self.documents.get_mut(url) {
                    doc.page_rank = *rank;
                }
            }
        }
        self.semantic.clear();
        let docs: Vec<Document> = self.documents.values().cloned().collect();
        for doc in &docs {
            self.semantic.index_document(doc);
        }
        self.cache.clear();
    }

    pub fn set_sap_scores(&mut self, scores: HashMap<String, f64>) {
        self.sap_domain_scores = scores;
        self.cache.clear();
    }

    pub fn search(&mut self, query: SearchQuery) -> SearchResponse {
        let start = now_ms();
        let page = query.page.max(1);
        let per_page = query.per_page.max(1).min(1_000);
        let cache_key = format!("{}|{}|{}|{:?}|{:?}", query.q, page, per_page, query.category, query.language);

        if let Some((cached, ts)) = self.cache.get(&cache_key) {
            if now_ms().saturating_sub(*ts) < 60_000 {
                let mut resp = cached.clone();
                resp.query_time_ms = now_ms().saturating_sub(start);
                return resp;
            }
        }

        let plan = self.query_intel.plan(&query.q);
        let original_terms: Vec<String> = tokenize(&query.q).into_iter().map(|t| t.to_lowercase()).collect();
        let mut doc_scores: HashMap<String, f64> = HashMap::new();

        for term in &plan.terms {
            if let Some(postings) = self.inverted_index.get(&term.term) {
                for (doc_url, &tf) in postings {
                    // smooth-idf (+1): a term present in *every* doc otherwise yields
                    // idf=ln(1)=0, making a real match score 0 and get filtered out.
                    let idf = ((self.doc_count as f64 + 1.0) / (postings.len() as f64 + 1.0)).ln() + 1.0;
                    *doc_scores.entry(doc_url.clone()).or_insert(0.0) += tf as f64 * idf * term.weight;
                }
            }
        }

        // Spell-correction was cosmetic until now: the corrected query was surfaced
        // in the response but its tokens never reached the index, so a typo-only
        // query ("serach") matched nothing (the semantic pass even seeds a 0.0 entry,
        // so an `is_empty` guard would never fire). Feed the corrected tokens into the
        // index too — additively — so the correction actually contributes tf·idf.
        if let Some(ref corrected) = plan.corrected_query {
            for tok in tokenize(corrected) {
                if let Some(postings) = self.inverted_index.get(&tok) {
                    for (doc_url, &tf) in postings {
                        // smooth-idf (+1): a term present in *every* doc otherwise yields
                    // idf=ln(1)=0, making a real match score 0 and get filtered out.
                    let idf = ((self.doc_count as f64 + 1.0) / (postings.len() as f64 + 1.0)).ln() + 1.0;
                        *doc_scores.entry(doc_url.clone()).or_insert(0.0) += tf as f64 * idf;
                    }
                }
            }
        }

        for (url, semantic_score) in self.semantic.top_k(&query.q, per_page.max(25) * 4) {
            if semantic_score >= 0.55 {
                doc_scores.entry(url).or_insert(0.0);
            }
        }

        let mut ranked: Vec<(f64, RankingFactors, &Document)> = Vec::with_capacity(doc_scores.len());
        for (url, tfidf) in doc_scores {
            let Some(doc) = self.documents.get(&url) else { continue; };
            if !query.category.as_ref().map_or(true, |c| doc.category.as_ref().map_or(true, |dc| dc == c)) {
                continue;
            }
            if !query.language.as_ref().map_or(true, |l| doc.language.as_ref().map_or(true, |dl| dl == l)) {
                continue;
            }

            let freshness = self.calculate_freshness(doc);
            let quality = self.calculate_quality(doc);
            let sap = self.sap_boost(doc);
            let semantic = self.semantic.similarity(&query.q, &doc.url);
            if tfidf <= 0.0 && semantic < 0.55 {
                continue;
            }

            let factors = self.ranker.compute_factors(tfidf, doc.page_rank, freshness, quality, sap);
            let signals = SearchSignals {
                tf_idf: tfidf,
                semantic,
                title_match: ml_ranker::title_match_score(&query.q, &doc.title),
                exact_phrase: ml_ranker::exact_phrase_score(&query.q, &format!("{} {}", doc.title, doc.content)),
                term_coverage: ml_ranker::term_coverage(&original_terms, &doc.title, &doc.content),
                page_rank: doc.page_rank,
                domain_authority: ml_ranker::domain_authority(&doc.url),
                quality,
                freshness,
                sap,
            };
            let score = (factors.final_score * 0.45 + self.learning_ranker.score(&signals) * 0.55).clamp(0.0, 1.0);
            ranked.push((score, factors, doc));
        }

        ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        let total_results = ranked.len();
        let total_pages = if total_results == 0 { 0 } else { (total_results + per_page - 1) / per_page };
        let start_idx = (page.saturating_sub(1)) * per_page;
        let end_idx = (start_idx + per_page).min(total_results);
        let page_rows: &[(f64, RankingFactors, &Document)] = if start_idx >= total_results {
            &[]
        } else {
            &ranked[start_idx..end_idx]
        };

        let formatted: Vec<SearchResult> = page_rows
            .iter()
            .map(|(score, factors, doc)| {
                let (line, line_text) = best_match_line(&doc.content, &query.q);
                SearchResult {
                    doc_id: doc.id.clone(),
                    url: doc.url.clone(),
                    title: doc.title.clone(),
                    snippet: generate_snippet(&doc.content, &query.q, 200),
                    score: *score,
                    ranking_factors: Some(factors.clone()),
                    meta_description: doc.meta_description.clone(),
                    last_crawled: doc.last_crawled,
                    line,
                    line_text,
                }
            })
            .collect();

        let response = SearchResponse {
            results: formatted,
            total_results,
            page,
            total_pages,
            query_time_ms: now_ms().saturating_sub(start),
            corrected_query: plan.corrected_query,
        };
        self.cache.insert(cache_key, (response.clone(), now_ms()));
        response
    }

    /// Literal substring search — ripgrep parity. No spell-correction, no synonym
    /// expansion, no semantic fallback: returns exactly the documents whose content
    /// or title contains `needle`, each with the first matching line number + text.
    ///
    /// This is the mode to use for code/symbol lookup: `tokenize` splits on every
    /// non-alphanumeric char, so the normal `search()` would shatter `refill_slots`
    /// into `["refill", "slots"]` and drag in semantically-related noise. A literal
    /// scan keeps the symbol intact and stays deterministic (ordered by match count
    /// desc, then url asc). `case_sensitive` toggles case folding.
    pub fn literal_search(
        &self,
        needle: &str,
        page: usize,
        per_page: usize,
        case_sensitive: bool,
    ) -> SearchResponse {
        let start = now_ms();
        let page = page.max(1);
        let per_page = per_page.max(1).min(1_000);
        let needle_cmp = if case_sensitive { needle.to_string() } else { needle.to_lowercase() };

        let mut hits: Vec<(u32, SearchResult)> = Vec::new();
        if !needle.is_empty() {
            for doc in self.documents.values() {
                let body = if case_sensitive { doc.content.clone() } else { doc.content.to_lowercase() };
                let title = if case_sensitive { doc.title.clone() } else { doc.title.to_lowercase() };
                let count = (body.matches(&needle_cmp).count() + title.matches(&needle_cmp).count()) as u32;
                if count == 0 {
                    continue;
                }
                let (line, line_text) = first_literal_line(&doc.content, needle, case_sensitive);
                hits.push((
                    count,
                    SearchResult {
                        doc_id: doc.id.clone(),
                        url: doc.url.clone(),
                        title: doc.title.clone(),
                        snippet: line_text
                            .clone()
                            .unwrap_or_else(|| generate_snippet(&doc.content, needle, 200)),
                        score: count as f64,
                        ranking_factors: None,
                        meta_description: doc.meta_description.clone(),
                        last_crawled: doc.last_crawled,
                        line,
                        line_text,
                    },
                ));
            }
        }
        // Deterministic: most matches first, ties broken by url (stable, grep-like).
        hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.url.cmp(&b.1.url)));

        let total_results = hits.len();
        let total_pages = if total_results == 0 { 0 } else { (total_results + per_page - 1) / per_page };
        let start_idx = page.saturating_sub(1) * per_page;
        let end_idx = (start_idx + per_page).min(total_results);
        let results: Vec<SearchResult> = if start_idx >= total_results {
            Vec::new()
        } else {
            hits[start_idx..end_idx].iter().map(|(_, r)| r.clone()).collect()
        };

        SearchResponse {
            results,
            total_results,
            page,
            total_pages,
            query_time_ms: now_ms().saturating_sub(start),
            corrected_query: None,
        }
    }

    pub fn doc_count(&self) -> usize {
        self.doc_count
    }

    pub fn get_pagerank(&self, url: &str) -> Option<f64> {
        self.documents.get(url).map(|d| d.page_rank)
    }

    pub fn stats(&self) -> SearchIndexStats {
        SearchIndexStats {
            documents: self.documents.len(),
            terms: self.inverted_index.len(),
            links: self.link_graph.values().map(|v| v.len()).sum(),
            semantic_embeddings: self.semantic.len(),
            dictionary_terms: self.query_intel.dictionary_size(),
            synonym_terms: self.query_intel.synonym_groups(),
        }
    }

    fn add_doc_to_runtime(&mut self, doc: &Document) {
        let combined = format!("{} {}", doc.title, doc.content);
        for term in tokenize(&combined) {
            self.inverted_index
                .entry(term)
                .or_default()
                .entry(doc.url.clone())
                .and_modify(|c| *c += 1)
                .or_insert(1);
        }
        self.query_intel.observe_document(&doc.title, &doc.content);
        self.semantic.index_document(doc);
        self.pagerank.add_page(&doc.url);
    }

    fn rebuild_runtime_indexes(&mut self) {
        self.inverted_index.clear();
        self.doc_count = self.documents.len();
        self.pagerank = PageRankCalculator::new();
        self.query_intel = QueryIntelligence::new();
        self.semantic = SemanticIndex::new();

        let docs: Vec<Document> = self.documents.values().cloned().collect();
        for doc in &docs {
            self.add_doc_to_runtime(doc);
        }
        for (from, outs) in self.link_graph.clone() {
            for (to, weight) in outs {
                self.pagerank.add_link(&from, &to, weight);
            }
        }
    }

    fn calculate_freshness(&self, doc: &Document) -> f64 {
        match doc.last_crawled {
            Some(last) => {
                let age_days = (now_ms().saturating_sub(last)) as f64 / 86_400_000.0;
                (-age_days / 30.0).exp().clamp(0.0, 1.0)
            }
            None => 0.5,
        }
    }

    fn calculate_quality(&self, doc: &Document) -> f64 {
        let mut score: f64 = 0.0;
        if !doc.title.is_empty() {
            score += 0.1;
            let len = doc.title.len();
            if (20..=80).contains(&len) {
                score += 0.1;
            }
            if doc.title.split_whitespace().count() >= 3 {
                score += 0.1;
            }
        }
        if !doc.content.is_empty() {
            score += 0.1;
            if doc.content.len() >= 100 {
                score += 0.1;
            }
            if doc.word_count >= 50 {
                score += 0.1;
            }
        }
        if let Some(ref meta) = doc.meta_description {
            score += 0.1;
            if (50..=160).contains(&meta.len()) {
                score += 0.1;
            }
        }
        let url = &doc.url;
        if url.starts_with("https://") {
            score += 0.05;
        }
        if !url.contains('?') || url.matches('&').count() <= 2 {
            score += 0.05;
        }
        if url.len() <= 100 {
            score += 0.05;
        }
        if (2..=5).contains(&url.matches('/').count()) {
            score += 0.05;
        }
        score.min(1.0)
    }

    fn sap_boost(&self, doc: &Document) -> f64 {
        let domain = doc
            .url
            .split("://")
            .nth(1)
            .unwrap_or(&doc.url)
            .split('/')
            .next()
            .unwrap_or(&doc.url)
            .split(':')
            .next()
            .unwrap_or(&doc.url);
        self.sap_domain_scores.get(domain).copied().unwrap_or(0.5)
    }
}

impl Default for SearchEngine {
    fn default() -> Self {
        Self::new()
    }
}

pub fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| is_search_token(s))
        .map(|s| s.to_string())
        .collect()
}

fn searchable_content(content: &str) -> String {
    let mut chars = content.chars();
    let preview: String = chars.by_ref().take(MAX_INDEXED_CONTENT_CHARS).collect();
    if chars.next().is_some() {
        format!("{preview}\n\n[flux-search: truncated preview for indexing]")
    } else {
        preview
    }
}

fn is_search_token(token: &str) -> bool {
    if token.len() < 2 || token.len() > 64 {
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

pub fn generate_snippet(content: &str, query: &str, max_length: usize) -> String {
    if content.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = content.chars().collect();
    if chars.len() <= max_length {
        return content.to_string();
    }

    let query_terms: Vec<String> = tokenize(query);
    let lower_chars: Vec<char> = content.to_lowercase().chars().collect();
    let mut best_score = 0u32;
    let mut best_start = 0usize;
    let limit = chars.len().saturating_sub(max_length);

    let mut i = 0usize;
    while i <= limit {
        let window: String = lower_chars[i..(i + max_length).min(lower_chars.len())].iter().collect();
        let score: u32 = query_terms.iter().map(|t| window.matches(t).count() as u32).sum();
        if score > best_score {
            best_score = score;
            best_start = i;
        }
        i += 50;
    }

    let end = (best_start + max_length).min(chars.len());
    let mut snippet: String = chars[best_start..end].iter().collect();
    if best_start > 0 {
        snippet = format!("...{}", snippet);
    }
    if end < chars.len() {
        snippet = format!("{}...", snippet);
    }
    snippet
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

/// 1-based line number + trimmed text of the line in `content` with the most
/// query-term occurrences (ties → earliest line). Returns `(None, None)` when the
/// query has no tokens or nothing matches. Used to give ranked results a concrete
/// grep-style line instead of just a filename.
pub fn best_match_line(content: &str, query: &str) -> (Option<u32>, Option<String>) {
    let terms = tokenize(query);
    if terms.is_empty() {
        return (None, None);
    }
    let mut best_score = 0u32;
    let mut best_line = 0u32;
    let mut best_text: Option<String> = None;
    for (i, raw) in content.lines().enumerate() {
        let lower = raw.to_lowercase();
        let score: u32 = terms.iter().map(|t| lower.matches(t.as_str()).count() as u32).sum();
        if score > best_score {
            best_score = score;
            best_line = (i as u32) + 1;
            best_text = Some(raw.trim().chars().take(240).collect());
        }
    }
    if best_score == 0 {
        (None, None)
    } else {
        (Some(best_line), best_text)
    }
}

/// First 1-based line containing `needle` as a literal substring, plus its trimmed
/// text. Symbol-friendly (does not tokenize), so `refill_slots` / `Foo::bar` match.
fn first_literal_line(content: &str, needle: &str, case_sensitive: bool) -> (Option<u32>, Option<String>) {
    if needle.is_empty() {
        return (None, None);
    }
    let needle_cmp = if case_sensitive { needle.to_string() } else { needle.to_lowercase() };
    for (i, raw) in content.lines().enumerate() {
        let hay = if case_sensitive { raw.to_string() } else { raw.to_lowercase() };
        if hay.contains(&needle_cmp) {
            return (Some((i as u32) + 1), Some(raw.trim().chars().take(240).collect()));
        }
    }
    (None, None)
}

fn is_ignored_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|s| s.to_str())
        .map(|name| matches!(name, "target" | "node_modules" | ".git" | "dist" | "build"))
        .unwrap_or(false)
}

/// Collect directory-ignore names from `.gitignore` files at `root` and up to a
/// few parent levels (to catch the repo-root file when indexing a subdir). Only
/// simple directory patterns are honored — a bare name (`vendor`) or a trailing
/// slash (`coverage/`). Globs, negations and path-anchored rules are deferred to a
/// future `ignore`-crate upgrade; this covers the common index-pollution cases.
fn collect_gitignored_dirs(root: &Path) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    let mut dir = Some(root.to_path_buf());
    let mut levels = 0;
    while let Some(d) = dir {
        if let Ok(text) = fs::read_to_string(d.join(".gitignore")) {
            for raw in text.lines() {
                let line = raw.trim();
                if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
                    continue;
                }
                let trimmed = line.trim_start_matches('/').trim_end_matches('/');
                // Only simple directory names (no globs, no nested path).
                if trimmed.is_empty() || trimmed.contains('/') || trimmed.contains('*') {
                    continue;
                }
                set.insert(trimmed.to_string());
            }
        }
        levels += 1;
        if levels >= 4 {
            break;
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
    set
}

fn dir_is_gitignored(path: &Path, ignored: &std::collections::HashSet<String>) -> bool {
    path.file_name()
        .and_then(|s| s.to_str())
        .map(|name| ignored.contains(name))
        .unwrap_or(false)
}

fn is_indexable_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|s| s.to_str()).unwrap_or(""),
        "rs" | "md" | "toml" | "json" | "ts" | "tsx" | "js" | "jsx" | "css" | "html" | "txt"
    )
}

fn category_from_path(path: &Path) -> Option<String> {
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let category = match ext {
        "rs" => "rust",
        "md" => "docs",
        "toml" | "json" => "config",
        "ts" | "tsx" | "js" | "jsx" => "frontend",
        "css" | "html" => "web",
        "txt" => "text",
        _ => return None,
    };
    Some(category.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(id: &str, url: &str, title: &str, content: &str) -> Document {
        Document {
            id: id.into(),
            url: url.into(),
            title: title.into(),
            content: content.into(),
            meta_description: Some(title.into()),
            language: Some("en".into()),
            category: Some("tech".into()),
            page_rank: 0.5,
            readability_score: 0.8,
            word_count: 0,
            last_crawled: Some(now_ms()),
            content_hash: String::new(),
        }
    }

    #[test]
    fn test_tokenize() {
        let tokens = tokenize("Hello World! Rust programming");
        assert!(tokens.contains(&"hello".to_string()));
        assert!(tokens.contains(&"world".to_string()));
    }

    #[test]
    fn test_index_and_search() {
        let mut engine = SearchEngine::new();
        engine.index_document(doc(
            "1",
            "https://example.com/rust",
            "Rust Programming Language",
            "Rust is a systems programming language focused on safety and performance.",
        ));
        engine.index_document(doc(
            "2",
            "https://example.com/python",
            "Python Programming",
            "Python is a high-level programming language great for beginners.",
        ));
        let response = engine.search(SearchQuery { q: "rust programming".into(), ..Default::default() });
        assert!(!response.results.is_empty());
        assert!(response.results[0].title.contains("Rust"));
    }

    #[test]
    fn test_spell_correction_and_synonym_expansion() {
        let mut engine = SearchEngine::new();
        engine.index_document(doc(
            "1",
            "https://flux.local/search",
            "Flux Search MCP",
            "The Flux search index supports query retrieval through MCP combos.",
        ));
        let response = engine.search(SearchQuery { q: "serach lookup".into(), ..Default::default() });
        assert_eq!(response.corrected_query.as_deref(), Some("search lookup"));
        assert!(!response.results.is_empty());
    }

    #[test]
    fn test_snapshot_roundtrip() {
        let mut engine = SearchEngine::new();
        engine.index_document(doc(
            "1",
            "https://example.com/p2p",
            "Distributed P2P Search",
            "A libp2p crawler can feed a distributed search index.",
        ));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.json");
        engine.save_to_path(&path).unwrap();

        let mut loaded = SearchEngine::load_from_path(&path).unwrap();
        let response = loaded.search(SearchQuery { q: "libp2p crawler".into(), ..Default::default() });
        assert_eq!(loaded.doc_count(), 1);
        assert!(!response.results.is_empty());
    }

    #[test]
    fn test_snippet_generation() {
        let content = "Rust is a systems programming language focused on safety, speed, and concurrency. It prevents segfaults and guarantees thread safety.";
        let snippet = generate_snippet(content, "rust safety", 80);
        assert!(snippet.contains("Rust"));
    }

    // ── ripgrep-lesson regressions ──

    #[test]
    fn test_ranked_results_carry_line_and_text() {
        // Lesson 1: a hit must come back with the concrete line, not just a filename.
        let mut engine = SearchEngine::new();
        engine.index_document(doc(
            "1",
            "file://crates/sigil-top/src/block_sync.rs",
            "block_sync.rs",
            "fn main() {}\nlet refill_slots = if full_archive { 1 } else { max };\nreturn ok;",
        ));
        let resp = engine.search(SearchQuery { q: "refill slots".into(), ..Default::default() });
        assert!(!resp.results.is_empty());
        let top = &resp.results[0];
        assert_eq!(top.line, Some(2), "best-matching line is line 2");
        assert!(top.line_text.as_deref().unwrap().contains("refill_slots"));
    }

    #[test]
    fn test_literal_search_keeps_symbols_and_is_noise_free() {
        // Lesson 2: literal mode must match the underscore symbol intact (tokenizing
        // would shatter it into refill/slots) AND return nothing for an absent term
        // — no semantic/synonym fallback dragging in unrelated docs.
        let mut engine = SearchEngine::new();
        engine.index_document(doc(
            "1",
            "file://a.rs",
            "a.rs",
            "fn x() {}\n    let refill_slots = compute();\n    ok\n",
        ));
        engine.index_document(doc(
            "2",
            "file://b.rs",
            "b.rs",
            "totally unrelated prose about scheduling and frontiers",
        ));

        let hit = engine.literal_search("refill_slots", 1, 10, false);
        assert_eq!(hit.total_results, 1, "exactly the one doc containing the symbol");
        assert_eq!(hit.results[0].line, Some(2));
        assert!(hit.results[0].line_text.as_deref().unwrap().contains("refill_slots"));
        assert!(hit.corrected_query.is_none(), "literal mode does not spell-correct");

        let miss = engine.literal_search("nonexistent_symbol_xyz", 1, 10, false);
        assert_eq!(miss.total_results, 0, "no semantic noise for an absent symbol");
    }

    #[test]
    fn test_index_path_respects_gitignore_dirs() {
        // Lesson 4: a directory listed in .gitignore must not be indexed.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join(".gitignore"), "skipme/\n").unwrap();
        fs::create_dir(root.join("skipme")).unwrap();
        fs::create_dir(root.join("keep")).unwrap();
        fs::write(root.join("skipme").join("a.rs"), "fn ignored_fn() {}").unwrap();
        fs::write(root.join("keep").join("b.rs"), "fn kept_fn() {}").unwrap();

        let mut engine = SearchEngine::new();
        let n = engine.index_path(root, true).unwrap();
        assert_eq!(n, 1, "only keep/b.rs is indexable+unignored");
        assert_eq!(engine.literal_search("ignored_fn", 1, 10, false).total_results, 0);
        assert_eq!(engine.literal_search("kept_fn", 1, 10, false).total_results, 1);
    }
}
