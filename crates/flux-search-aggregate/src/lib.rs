//! flux-search-aggregate — instant network-wide crawl rollup for agents.
//!
//! The flux-search crawler runs on many nodes over flux-p2p. Each node only
//! knows what *it* crawled. An agent that wants "the whole network's crawl
//! state + numbers" would otherwise have to poll every node and merge by hand.
//!
//! This crate does the merge once and pushes it out two ways, so an agent knows
//! the aggregated result **instantly**:
//!
//!  1. **Webhook push** — [`AggregatorState::refresh`] returns a [`WebhookPayload`]
//!     on every recompute; the MCP webhook layer (`flux_webhook_*`) fans it out
//!     to every subscribed agent. No polling.
//!  2. **MCP combo pull** — [`AggregatorState::combo`] returns a single
//!     [`McpComboResponse`] bundling the whole network summary + per-node health
//!     + top domains/terms in ONE call (the "combo" pattern: one call, everything).
//!
//! ```text
//!  node A ┐
//!  node B ┼─ NodeCrawlReport ──► aggregate() ──► NetworkCrawlSummary
//!  node C ┘                                       │        │
//!                                    WebhookPayload   McpComboResponse
//!                                    (pushed)         (pulled, one call)
//! ```

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// What one node reports about its local crawl. Sent over flux-p2p (or posted to
/// the aggregator) on each node's refresh interval.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeCrawlReport {
    /// Node / peer id (hex pubkey or short name).
    pub node_id: String,
    /// Unix ms this report was produced.
    pub ts_ms: u64,
    /// Documents currently indexed on this node.
    pub docs_indexed: u64,
    /// Distinct terms in this node's inverted index.
    pub unique_terms: u64,
    /// Total bytes of content crawled.
    pub bytes_crawled: u64,
    /// URLs in this node's crawl frontier (pending).
    pub frontier_pending: u64,
    /// Per-domain document counts on this node (domain → docs).
    pub domain_docs: BTreeMap<String, u64>,
    /// Top query terms by document frequency on this node (term → df).
    pub term_freq: BTreeMap<String, u64>,
}

impl NodeCrawlReport {
    pub fn new(node_id: impl Into<String>, ts_ms: u64) -> Self {
        NodeCrawlReport {
            node_id: node_id.into(),
            ts_ms,
            docs_indexed: 0,
            unique_terms: 0,
            bytes_crawled: 0,
            frontier_pending: 0,
            domain_docs: BTreeMap::new(),
            term_freq: BTreeMap::new(),
        }
    }
}

/// One (label, count) row in a ranked top-N list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ranked {
    pub label: String,
    pub count: u64,
}

/// The merged, network-wide picture. This is the number an agent wants.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NetworkCrawlSummary {
    /// Unix ms this summary was computed.
    pub computed_ms: u64,
    /// How many nodes contributed.
    pub node_count: usize,
    /// How many of those nodes are "fresh" (reported within the staleness window).
    pub fresh_nodes: usize,
    /// Sum of docs across nodes (raw — see `unique_domains` for dedup-aware reach).
    pub total_docs: u64,
    /// Sum of crawled bytes across nodes.
    pub total_bytes: u64,
    /// Sum of pending frontier URLs (network crawl backlog).
    pub total_frontier: u64,
    /// Distinct domains seen anywhere on the network (dedup across nodes).
    pub unique_domains: usize,
    /// Top domains network-wide, by summed doc count, descending.
    pub top_domains: Vec<Ranked>,
    /// Top terms network-wide, by summed document-frequency, descending.
    pub top_terms: Vec<Ranked>,
}

/// Merge node reports into one network summary. `now_ms` + `staleness_ms` decide
/// which nodes count as "fresh"; ALL reports contribute to totals (an agent wants
/// the full picture), but `fresh_nodes` flags how much of it is current.
/// `top_n` caps the ranked lists.
pub fn aggregate(
    reports: &[NodeCrawlReport],
    now_ms: u64,
    staleness_ms: u64,
    top_n: usize,
) -> NetworkCrawlSummary {
    let mut total_docs = 0u64;
    let mut total_bytes = 0u64;
    let mut total_frontier = 0u64;
    let mut fresh_nodes = 0usize;
    let mut domain_acc: BTreeMap<String, u64> = BTreeMap::new();
    let mut term_acc: BTreeMap<String, u64> = BTreeMap::new();

    for r in reports {
        total_docs = total_docs.saturating_add(r.docs_indexed);
        total_bytes = total_bytes.saturating_add(r.bytes_crawled);
        total_frontier = total_frontier.saturating_add(r.frontier_pending);
        if now_ms.saturating_sub(r.ts_ms) <= staleness_ms {
            fresh_nodes += 1;
        }
        for (dom, n) in &r.domain_docs {
            *domain_acc.entry(dom.clone()).or_insert(0) += *n;
        }
        for (term, n) in &r.term_freq {
            *term_acc.entry(term.clone()).or_insert(0) += *n;
        }
    }

    NetworkCrawlSummary {
        computed_ms: now_ms,
        node_count: reports.len(),
        fresh_nodes,
        total_docs,
        total_bytes,
        total_frontier,
        unique_domains: domain_acc.len(),
        top_domains: top_ranked(&domain_acc, top_n),
        top_terms: top_ranked(&term_acc, top_n),
    }
}

/// Rank a label→count map descending by count, then label (stable tie-break),
/// taking the top `n`.
fn top_ranked(map: &BTreeMap<String, u64>, n: usize) -> Vec<Ranked> {
    let mut v: Vec<Ranked> = map
        .iter()
        .map(|(label, &count)| Ranked { label: label.clone(), count })
        .collect();
    v.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.label.cmp(&b.label)));
    v.truncate(n);
    v
}

/// The payload pushed to subscribed agents on each refresh. `kind` lets the
/// webhook layer route it (`"crawl.network.summary"`). It carries the full
/// summary plus a tiny delta vs the previous push so agents can react to change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WebhookPayload {
    pub kind: String,
    pub summary: NetworkCrawlSummary,
    /// docs added network-wide since the previous push (0 on first push).
    pub docs_delta: i64,
    /// nodes that went fresh→stale or appeared since last push.
    pub node_count_delta: i64,
}

/// One-call MCP combo response: everything an agent needs about the network
/// crawl in a single pull, no per-node round-trips.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpComboResponse {
    pub summary: NetworkCrawlSummary,
    /// Per-node health: node_id → (docs, fresh?). So an agent can see WHO has what.
    pub nodes: Vec<NodeHealth>,
    /// Human one-liner for an agent's log / decision.
    pub headline: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeHealth {
    pub node_id: String,
    pub docs_indexed: u64,
    pub fresh: bool,
    pub age_ms: u64,
}

/// Holds the latest report from each node + the previous summary, so it can
/// compute deltas for webhook pushes and serve the combo pull. One per
/// aggregator process (e.g. a bootstrap node, or the MCP server).
#[derive(Debug, Default)]
pub struct AggregatorState {
    latest: BTreeMap<String, NodeCrawlReport>,
    prev_summary: Option<NetworkCrawlSummary>,
    staleness_ms: u64,
    top_n: usize,
}

impl AggregatorState {
    /// `staleness_ms`: how recent a node report must be to count as fresh.
    /// `top_n`: size of the ranked domain/term lists.
    pub fn new(staleness_ms: u64, top_n: usize) -> Self {
        AggregatorState {
            latest: BTreeMap::new(),
            prev_summary: None,
            staleness_ms,
            top_n,
        }
    }

    /// Ingest a node's report (overwrites that node's previous report). Call this
    /// from the flux-p2p receive path or an HTTP POST from each node.
    pub fn ingest(&mut self, report: NodeCrawlReport) {
        self.latest.insert(report.node_id.clone(), report);
    }

    /// Recompute the summary and return a webhook payload (with deltas vs the
    /// last refresh). Call on a timer or on each ingest; push the result to
    /// subscribed agents via `flux_webhook_*`.
    pub fn refresh(&mut self, now_ms: u64) -> WebhookPayload {
        let reports: Vec<NodeCrawlReport> = self.latest.values().cloned().collect();
        let summary = aggregate(&reports, now_ms, self.staleness_ms, self.top_n);

        let (docs_delta, node_count_delta) = match &self.prev_summary {
            Some(p) => (
                summary.total_docs as i64 - p.total_docs as i64,
                summary.node_count as i64 - p.node_count as i64,
            ),
            None => (0, 0),
        };
        self.prev_summary = Some(summary.clone());

        WebhookPayload {
            kind: "crawl.network.summary".to_string(),
            summary,
            docs_delta,
            node_count_delta,
        }
    }

    /// Serve the one-shot MCP combo: full summary + per-node health + headline.
    /// Read-only; does NOT advance the delta baseline (so a combo pull doesn't
    /// swallow a delta a subsequent webhook push should report).
    pub fn combo(&self, now_ms: u64) -> McpComboResponse {
        let reports: Vec<NodeCrawlReport> = self.latest.values().cloned().collect();
        let summary = aggregate(&reports, now_ms, self.staleness_ms, self.top_n);

        let mut nodes: Vec<NodeHealth> = self
            .latest
            .values()
            .map(|r| {
                let age = now_ms.saturating_sub(r.ts_ms);
                NodeHealth {
                    node_id: r.node_id.clone(),
                    docs_indexed: r.docs_indexed,
                    fresh: age <= self.staleness_ms,
                    age_ms: age,
                }
            })
            .collect();
        nodes.sort_by(|a, b| b.docs_indexed.cmp(&a.docs_indexed).then_with(|| a.node_id.cmp(&b.node_id)));

        let headline = format!(
            "{} nodes ({} fresh) · {} docs · {} domains · {} URLs pending",
            summary.node_count,
            summary.fresh_nodes,
            summary.total_docs,
            summary.unique_domains,
            summary.total_frontier,
        );

        McpComboResponse { summary, nodes, headline }
    }

    /// JSON for the MCP tool result.
    pub fn combo_json(&self, now_ms: u64) -> String {
        serde_json::to_string(&self.combo(now_ms)).expect("combo serializes")
    }

    pub fn node_count(&self) -> usize {
        self.latest.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(node: &str, ts: u64, docs: u64, domains: &[(&str, u64)], terms: &[(&str, u64)]) -> NodeCrawlReport {
        let mut r = NodeCrawlReport::new(node, ts);
        r.docs_indexed = docs;
        r.bytes_crawled = docs.saturating_mul(1000);
        r.frontier_pending = 10;
        for (d, n) in domains {
            r.domain_docs.insert((*d).into(), *n);
        }
        for (t, n) in terms {
            r.term_freq.insert((*t).into(), *n);
        }
        r
    }

    #[test]
    fn aggregate_sums_across_nodes() {
        let reports = vec![
            report("A", 1000, 100, &[("rust.org", 50), ("a.com", 50)], &[("rust", 30)]),
            report("B", 1000, 200, &[("rust.org", 70), ("b.com", 90)], &[("rust", 40), ("zk", 10)]),
        ];
        let s = aggregate(&reports, 1000, 60_000, 10);
        assert_eq!(s.node_count, 2);
        assert_eq!(s.total_docs, 300);
        assert_eq!(s.unique_domains, 3, "rust.org dedup'd across nodes");
        // rust.org tops with the cross-node sum 50+70=120 (> b.com's 90)
        assert_eq!(s.top_domains[0].label, "rust.org");
        assert_eq!(s.top_domains[0].count, 120);
        // rust term summed across nodes: 30+40=70
        assert_eq!(s.top_terms[0].label, "rust");
        assert_eq!(s.top_terms[0].count, 70);
    }

    #[test]
    fn staleness_marks_fresh_nodes() {
        let reports = vec![
            report("fresh", 9_000, 10, &[], &[]),
            report("stale", 1_000, 10, &[], &[]),
        ];
        // now=10_000, window=5_000 → "fresh" is 1s old (ok), "stale" is 9s old (stale)
        let s = aggregate(&reports, 10_000, 5_000, 10);
        assert_eq!(s.node_count, 2, "both count toward totals");
        assert_eq!(s.fresh_nodes, 1, "only one within the window");
        assert_eq!(s.total_docs, 20);
    }

    #[test]
    fn top_n_caps_lists() {
        let mut r = NodeCrawlReport::new("A", 1000);
        for i in 0..20 {
            r.domain_docs.insert(format!("d{i}.com"), i as u64);
        }
        let s = aggregate(&[r], 1000, 60_000, 5);
        assert_eq!(s.top_domains.len(), 5, "capped at top_n");
        // highest count first
        assert_eq!(s.top_domains[0].label, "d19.com");
    }

    #[test]
    fn aggregator_ingest_overwrites_per_node() {
        let mut agg = AggregatorState::new(60_000, 10);
        agg.ingest(report("A", 1000, 100, &[], &[]));
        agg.ingest(report("A", 2000, 150, &[], &[])); // newer report from A
        assert_eq!(agg.node_count(), 1, "same node overwrites, not duplicates");
        let combo = agg.combo(2000);
        assert_eq!(combo.summary.total_docs, 150, "latest A report wins");
    }

    #[test]
    fn refresh_computes_docs_delta() {
        let mut agg = AggregatorState::new(60_000, 10);
        agg.ingest(report("A", 1000, 100, &[], &[]));
        let p1 = agg.refresh(1000);
        assert_eq!(p1.docs_delta, 0, "first push has no baseline");

        agg.ingest(report("B", 2000, 50, &[], &[]));
        let p2 = agg.refresh(2000);
        assert_eq!(p2.docs_delta, 50, "B added 50 docs network-wide");
        assert_eq!(p2.node_count_delta, 1, "one new node");
        assert_eq!(p2.summary.total_docs, 150);
        assert_eq!(p2.kind, "crawl.network.summary");
    }

    #[test]
    fn combo_gives_per_node_health_and_headline() {
        let mut agg = AggregatorState::new(5_000, 10);
        agg.ingest(report("big", 10_000, 500, &[("x.com", 500)], &[("term", 5)]));
        agg.ingest(report("small-stale", 1_000, 10, &[], &[]));
        let c = agg.combo(10_000);
        assert_eq!(c.nodes.len(), 2);
        // sorted by docs desc → "big" first
        assert_eq!(c.nodes[0].node_id, "big");
        assert!(c.nodes[0].fresh, "big reported just now");
        assert!(!c.nodes[1].fresh, "small-stale is 9s old past 5s window");
        assert!(c.headline.contains("2 nodes"));
        assert!(c.headline.contains("510 docs"));
    }

    #[test]
    fn combo_does_not_advance_delta_baseline() {
        let mut agg = AggregatorState::new(60_000, 10);
        agg.ingest(report("A", 1000, 100, &[], &[]));
        agg.refresh(1000); // baseline = 100
        agg.ingest(report("B", 2000, 50, &[], &[]));
        let _ = agg.combo(2000); // a pull — must NOT eat the delta
        let push = agg.refresh(2000);
        assert_eq!(push.docs_delta, 50, "combo pull didn't swallow the delta");
    }

    #[test]
    fn empty_network_is_safe() {
        let agg = AggregatorState::new(60_000, 10);
        let c = agg.combo(1000);
        assert_eq!(c.summary.node_count, 0);
        assert_eq!(c.summary.total_docs, 0);
        assert!(c.nodes.is_empty());
        assert!(c.headline.contains("0 nodes"));
    }

    #[test]
    fn combo_json_is_valid() {
        let mut agg = AggregatorState::new(60_000, 10);
        agg.ingest(report("A", 1000, 100, &[("x.com", 100)], &[("t", 1)]));
        let json = agg.combo_json(1000);
        let back: McpComboResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.summary.total_docs, 100);
    }

    #[test]
    fn saturating_totals_dont_overflow() {
        let reports = vec![
            report("A", 1, u64::MAX, &[], &[]),
            report("B", 1, 100, &[], &[]),
        ];
        let s = aggregate(&reports, 1, 60_000, 10);
        assert_eq!(s.total_docs, u64::MAX, "saturates, no panic");
    }
}
