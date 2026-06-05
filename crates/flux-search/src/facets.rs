//! flux-search v2 — facet aggregation.
//!
//! Computes top-N facet values across a result set so the UI can show
//! filter chips ("agent: rocky-sigil · 27", "today · 412", etc).

use crate::Document;
use std::collections::HashMap;

/// One facet value + its count.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FacetEntry {
    pub value: String,
    pub count: u64,
}

/// Aggregated facets for a result set.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct Facets {
    pub by_kind: Vec<FacetEntry>,
    pub by_agent: Vec<FacetEntry>,
    pub by_day: Vec<FacetEntry>,
    pub total: u64,
}

const TOP_N: usize = 8;

/// Aggregate facets from a slice of documents.
pub fn aggregate(docs: &[Document]) -> Facets {
    let mut by_kind: HashMap<String, u64> = HashMap::new();
    let mut by_agent: HashMap<String, u64> = HashMap::new();
    let mut by_day: HashMap<String, u64> = HashMap::new();

    for d in docs {
        if let Some(c) = &d.category {
            *by_kind.entry(c.clone()).or_default() += 1;
        }
        if let Some(a) = extract_agent_id(&d.content) {
            *by_agent.entry(a).or_default() += 1;
        }
        if let Some(ts) = d.last_crawled {
            *by_day.entry(day_bucket(ts)).or_default() += 1;
        }
    }

    Facets {
        by_kind: top_n(by_kind, TOP_N),
        by_agent: top_n(by_agent, TOP_N),
        by_day: top_n(by_day, TOP_N),
        total: docs.len() as u64,
    }
}

fn top_n(m: HashMap<String, u64>, n: usize) -> Vec<FacetEntry> {
    let mut v: Vec<FacetEntry> = m
        .into_iter()
        .map(|(value, count)| FacetEntry { value, count })
        .collect();
    v.sort_by(|a, b| b.count.cmp(&a.count));
    v.truncate(n);
    v
}

/// Best-effort: look for an "agent:" line in the Document content.
fn extract_agent_id(content: &str) -> Option<String> {
    content
        .lines()
        .find_map(|l| l.strip_prefix("agent: ").map(|s| s.trim().to_string()))
}

/// Bucket a unix-ms timestamp into a "YYYY-MM-DD" string (UTC).
fn day_bucket(ts_ms: u64) -> String {
    // Plain math (no chrono dep) — days since 1970-01-01.
    let secs = ts_ms / 1000;
    let days = secs / 86_400;
    let (y, m, d) = civil_from_days(days as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Howard Hinnant's "civil_from_days" algorithm — converts days since
/// 1970-01-01 to (year, month, day) without external deps.
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = y + if m <= 2 { 1 } else { 0 };
    (y as i32, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(category: &str, agent: &str, ts: u64) -> Document {
        Document {
            id: "x".into(),
            url: "".into(),
            title: "".into(),
            content: format!("agent: {agent}\n"),
            meta_description: None,
            language: None,
            category: Some(category.into()),
            page_rank: 0.0,
            readability_score: 0.0,
            word_count: 0,
            last_crawled: Some(ts),
            content_hash: "".into(),
        }
    }

    #[test]
    fn aggregate_counts_kinds_and_agents() {
        let docs = vec![
            doc("tool", "rocky-sigil", 1_780_125_000_000),
            doc("tool", "rocky-sigil", 1_780_125_500_000),
            doc("broadcast", "rocky", 1_780_125_000_000),
            doc("doc", "rocky-sigil", 1_780_125_000_000),
        ];
        let f = aggregate(&docs);
        assert_eq!(f.total, 4);
        let kind: HashMap<_, _> = f.by_kind.into_iter().map(|e| (e.value, e.count)).collect();
        assert_eq!(kind.get("tool").copied(), Some(2));
        assert_eq!(kind.get("doc").copied(), Some(1));
        let agent: HashMap<_, _> = f.by_agent.into_iter().map(|e| (e.value, e.count)).collect();
        assert_eq!(agent.get("rocky-sigil").copied(), Some(3));
    }

    #[test]
    fn day_bucket_is_iso_date() {
        // 2026-05-30 00:00 UTC ≈ unix 1780099200
        let s = day_bucket(1_780_099_200 * 1000);
        assert_eq!(s, "2026-05-30");
    }

    #[test]
    fn top_n_caps() {
        let mut m = HashMap::new();
        for i in 0..30 {
            m.insert(format!("k{i}"), i as u64);
        }
        assert_eq!(top_n(m, 5).len(), 5);
    }
}
