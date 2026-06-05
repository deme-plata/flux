//! Vizily-style semantic retrieval without heavyweight model dependencies.
//!
//! The original project sketched BERT/FastText-style embeddings. This Flux port
//! keeps the same interface idea but uses deterministic hash embeddings so it
//! stays small, reproducible, and MCP-friendly.

use std::collections::HashMap;

use crate::Document;

const DEFAULT_DIM: usize = 128;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DocumentEmbedding {
    pub id: String,
    pub url: String,
    pub title_vector: Vec<f32>,
    pub content_vector: Vec<f32>,
    pub combined_vector: Vec<f32>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SemanticIndex {
    dim: usize,
    embeddings: HashMap<String, DocumentEmbedding>,
}

impl SemanticIndex {
    pub fn new() -> Self {
        Self { dim: DEFAULT_DIM, embeddings: HashMap::new() }
    }

    pub fn clear(&mut self) {
        self.embeddings.clear();
    }

    pub fn index_document(&mut self, doc: &Document) {
        let title_vector = embed_text(&doc.title, self.dim);
        let content_prefix = doc.content.chars().take(1_200).collect::<String>();
        let content_vector = embed_text(&content_prefix, self.dim);
        let combined_vector = blend(&title_vector, &content_vector, 0.70, 0.30);
        self.embeddings.insert(doc.url.clone(), DocumentEmbedding {
            id: doc.id.clone(),
            url: doc.url.clone(),
            title_vector,
            content_vector,
            combined_vector,
        });
    }

    pub fn similarity(&self, query: &str, url: &str) -> f64 {
        let Some(embedding) = self.embeddings.get(url) else {
            return 0.0;
        };
        let query_vector = embed_text(query, self.dim);
        cosine_similarity(&query_vector, &embedding.combined_vector) as f64
    }

    pub fn top_k(&self, query: &str, k: usize) -> Vec<(String, f64)> {
        let query_vector = embed_text(query, self.dim);
        let mut scored: Vec<(String, f64)> = self.embeddings
            .values()
            .map(|embedding| {
                (
                    embedding.url.clone(),
                    cosine_similarity(&query_vector, &embedding.combined_vector) as f64,
                )
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);
        scored
    }

    pub fn len(&self) -> usize {
        self.embeddings.len()
    }
}

impl Default for SemanticIndex {
    fn default() -> Self {
        Self::new()
    }
}

pub fn embed_text(text: &str, dim: usize) -> Vec<f32> {
    let mut vector = vec![0.0f32; dim.max(1)];
    for token in crate::tokenize(text) {
        let h = stable_hash(&token);
        let idx = (h as usize) % vector.len();
        let sign = if h & 1 == 0 { 1.0 } else { -1.0 };
        let weight = 1.0 + (token.len().min(12) as f32 / 12.0);
        vector[idx] += sign * weight;
    }
    normalize(&mut vector);
    vector
}

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut aa = 0.0f32;
    let mut bb = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        aa += x * x;
        bb += y * y;
    }
    if aa == 0.0 || bb == 0.0 {
        0.0
    } else {
        ((dot / (aa.sqrt() * bb.sqrt())) + 1.0) / 2.0
    }
}

fn blend(a: &[f32], b: &[f32], aw: f32, bw: f32) -> Vec<f32> {
    let mut out = a.iter().zip(b.iter()).map(|(x, y)| x * aw + y * bw).collect::<Vec<_>>();
    normalize(&mut out);
    out
}

fn normalize(v: &mut [f32]) {
    let mag = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if mag > 0.0 {
        for x in v {
            *x /= mag;
        }
    }
}

fn stable_hash(s: &str) -> u64 {
    let mut hash = 14_695_981_039_346_656_037u64;
    for b in s.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similar_text_scores_above_unrelated_text() {
        let q = embed_text("distributed p2p search", 64);
        let near = embed_text("p2p distributed crawler search", 64);
        let far = embed_text("wallet balance transfer", 64);
        assert!(cosine_similarity(&q, &near) > cosine_similarity(&q, &far));
    }
}
