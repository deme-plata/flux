//! dataset.rs — turn **chronos** sim runs into a fine-tune corpus.
//!
//! `flux_chronos_run` emits a deterministic star-flood report per config. We
//! parse those lines into [`ChronosRecord`]s and emit instruction/completion
//! pairs ([`Example`]) as JSONL — the corpus an expert is fine-tuned on so it
//! learns to *reason about gossip/consensus delivery under scale, latency, and
//! redundancy*. The data is reproducible (seed-pinned), so the corpus is too.
//!
//! HONEST LIMIT (measured 2026-05-31): the chronos MCP wrapper currently reports
//! 0.0% loss / 100% delivery regardless of the `drop` arg, and caps node counts.
//! So this corpus teaches the scale×latency×redundancy surface, NOT loss-
//! resilience — until the drop param is wired through, examples stay clean-network.

use serde::Serialize;

/// One parsed chronos run.
#[derive(Debug, Clone, PartialEq)]
pub struct ChronosRecord {
    pub nodes: u32,
    pub sinks: u32,
    pub msgs_each: u32,
    pub latency_ms: u32,
    pub loss_pct: f64,
    pub redundancy: u32,
    pub delivered: u32,
    pub total: u32,
    pub sim_wall_ms: u32,
    pub seed: u64,
}

impl ChronosRecord {
    pub fn delivery_pct(&self) -> f64 {
        if self.total == 0 { 0.0 } else { self.delivered as f64 / self.total as f64 * 100.0 }
    }
}

/// An instruction-tuning example (Alpaca-style: instruction + input → output).
#[derive(Debug, Clone, Serialize)]
pub struct Example {
    pub instruction: String,
    pub input: String,
    pub output: String,
}

/// Parse a `flux_chronos_run` report (the multi-line text block) into a record.
/// Tolerant of field order; returns None if the delivery line is absent.
pub fn parse_run(report: &str) -> Option<ChronosRecord> {
    let num = |s: &str, suffix: &str| -> Option<f64> {
        // grab the number immediately preceding `suffix` (e.g. "40ms", "x3")
        let pos = s.find(suffix)?;
        let head = &s[..pos];
        let n: String = head.chars().rev().take_while(|c| c.is_ascii_digit() || *c == '.').collect::<String>().chars().rev().collect();
        n.parse().ok()
    };
    let mut nodes = 0u32; let mut sinks = 0u32; let mut msgs_each = 0u32;
    let mut latency_ms = 0u32; let mut loss_pct = 0.0f64; let mut redundancy = 1u32;
    let mut delivered = 0u32; let mut total = 0u32; let mut sim_wall_ms = 0u32; let mut seed = 0u64;

    for line in report.lines() {
        let l = line.trim();
        // "8 nodes (7 sinks) · 50 msgs each · 40ms latency · 0.0% loss · redundancy x1"
        if l.contains("nodes (") {
            nodes = l.split_whitespace().next().and_then(|x| x.parse().ok()).unwrap_or(0);
            if let Some(s) = num(l, " sinks)") { sinks = s as u32; }
            if let Some(m) = num(l, " msgs each") { msgs_each = m as u32; }
            if let Some(ms) = num(l, "ms latency") { latency_ms = ms as u32; }
            if let Some(p) = num(l, "% loss") { loss_pct = p; }
            if let Some(r) = num(l, "redundancy x").or_else(|| num(l, "x")) { redundancy = r.max(1.0) as u32; }
        }
        // "Unique delivered: 350/350 (100.0%)"
        if l.starts_with("Unique delivered:") {
            if let Some(frac) = l.split(':').nth(1).map(str::trim) {
                let nums: Vec<&str> = frac.split('/').collect();
                if nums.len() >= 2 {
                    delivered = nums[0].trim().parse().unwrap_or(0);
                    total = nums[1].split_whitespace().next().and_then(|x| x.parse().ok()).unwrap_or(0);
                }
            }
        }
        // "Seed: 42 (reproducible) · sim wall: 6ms"
        if l.contains("Seed:") {
            if let Some(s) = num(l, " (reproducible)") { seed = s as u64; }
            if let Some(w) = num(l, "ms").filter(|_| l.contains("sim wall")) {
                // sim wall is the last "Nms" on the line
                if let Some(pos) = l.rfind("sim wall:") {
                    if let Some(w2) = num(&l[pos..], "ms") { sim_wall_ms = w2 as u32; } else { sim_wall_ms = w as u32; }
                }
            }
        }
    }
    if total == 0 { return None; }
    Some(ChronosRecord { nodes, sinks, msgs_each, latency_ms, loss_pct, redundancy, delivered, total, sim_wall_ms, seed })
}

/// Turn a record into instruction-tuning examples. Each record yields a few
/// framings (predict delivery, explain redundancy, reason about wall time) so the
/// expert learns the relationship, not a lookup.
pub fn to_examples(r: &ChronosRecord) -> Vec<Example> {
    let cfg = format!(
        "star-flood gossip: {} nodes ({} sinks), {} msgs each, {}ms link latency, {:.1}% loss, redundancy x{}",
        r.nodes, r.sinks, r.msgs_each, r.latency_ms, r.loss_pct, r.redundancy
    );
    vec![
        Example {
            instruction: "Given a chronos gossip-flood config, predict the unique-message delivery rate and justify it.".into(),
            input: cfg.clone(),
            output: format!(
                "Delivery: {}/{} = {:.1}%. With {:.1}% link loss and redundancy x{}, every sink still receives each unique message (a star flood gives each sink {} independent paths from the producer at x{}), so delivery saturates at 100% on a lossless link. Reproducible at seed {}.",
                r.delivered, r.total, r.delivery_pct(), r.loss_pct, r.redundancy, r.redundancy, r.redundancy, r.seed
            ),
        },
        Example {
            instruction: "Estimate the simulated wall time for this chronos run and explain what dominates it.".into(),
            input: cfg.clone(),
            output: format!(
                "Sim wall ≈ {}ms. Chronos runs in virtual time, so wall time tracks event count ({} deliveries) not real latency — the {}ms link delay is modeled, not waited on. Scale (nodes×msgs) drives wall time; latency does not.",
                r.sim_wall_ms, r.total, r.latency_ms
            ),
        },
    ]
}

/// Build the JSONL corpus (one Example per line) from a set of records.
pub fn to_jsonl(records: &[ChronosRecord]) -> String {
    let mut out = String::new();
    for r in records {
        for ex in to_examples(r) {
            if let Ok(line) = serde_json::to_string(&ex) {
                out.push_str(&line);
                out.push('\n');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "🕰 flux_chronos_run — star-flood\n  8 nodes (7 sinks) · 50 msgs each · 40ms latency · 0.0% loss · redundancy x1\n  Unique delivered: 350/350 (100.0%)\n  Seed: 42 (reproducible) · sim wall: 6ms";

    #[test]
    fn parses_a_real_chronos_report() {
        let r = parse_run(SAMPLE).expect("should parse");
        assert_eq!(r.nodes, 8);
        assert_eq!(r.sinks, 7);
        assert_eq!(r.msgs_each, 50);
        assert_eq!(r.latency_ms, 40);
        assert_eq!(r.delivered, 350);
        assert_eq!(r.total, 350);
        assert_eq!(r.redundancy, 1);
        assert_eq!(r.seed, 42);
        assert!((r.delivery_pct() - 100.0).abs() < 1e-6);
    }

    #[test]
    fn emits_jsonl_examples() {
        let r = parse_run(SAMPLE).unwrap();
        let jsonl = to_jsonl(&[r]);
        let lines: Vec<&str> = jsonl.lines().collect();
        assert_eq!(lines.len(), 2, "two framings per record");
        // each line is valid JSON with the three fields
        for l in lines {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            assert!(v.get("instruction").is_some());
            assert!(v.get("input").is_some());
            assert!(v.get("output").is_some());
        }
    }

    #[test]
    fn skips_reports_without_delivery_line() {
        assert!(parse_run("🕰 flux_chronos_run — star-flood\n  garbage").is_none());
    }
}
