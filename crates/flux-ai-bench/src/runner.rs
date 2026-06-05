//! Runner — load submissions, evaluate against tasks, persist results.
//!
//! Persistence is JSONL at the path returned by [`ledger_path`]. Each line is
//! a [`BenchResult`]. Re-folding the ledger produces a leaderboard.

use crate::{AgentRef, BenchResult, Submission, TaskId, TaskResult};
use crate::scoring::{Score, TaskOutcome};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Default location for the bench ledger.
pub fn ledger_path() -> PathBuf {
    if let Ok(p) = std::env::var("FLUX_AI_BENCH_LEDGER") {
        return PathBuf::from(p);
    }
    PathBuf::from("/home/orobit/flux-ai-bench-ledger.jsonl")
}

/// Append one bench result to the JSONL ledger.
pub fn append_to_ledger(result: &BenchResult) -> Result<()> {
    use std::io::Write;
    let path = ledger_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening ledger {}", path.display()))?;
    let line = serde_json::to_string(result)?;
    writeln!(f, "{}", line)?;
    Ok(())
}

/// Read every line of the ledger into [`BenchResult`]s.
pub fn read_ledger() -> Result<Vec<BenchResult>> {
    use std::io::{BufRead, BufReader};
    let path = ledger_path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let f = std::fs::File::open(&path)?;
    let mut out = Vec::new();
    for line in BufReader::new(f).lines() {
        let line = line?;
        if line.trim().is_empty() { continue; }
        if let Ok(r) = serde_json::from_str::<BenchResult>(&line) {
            out.push(r);
        }
    }
    Ok(out)
}

/// One row of the leaderboard — best run per agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaderboardEntry {
    pub agent: AgentRef,
    pub best_composite: u32,
    pub best_run_ts_ms: u64,
    pub runs: u32,
    pub per_task_best: HashMap<TaskId, u8>,
}

/// Fold the ledger into a leaderboard. Each agent keeps their best composite.
pub fn leaderboard() -> Result<Vec<LeaderboardEntry>> {
    let results = read_ledger()?;
    let mut by_wallet: HashMap<String, LeaderboardEntry> = HashMap::new();
    for r in results {
        let key = r.agent.wallet.clone();
        let entry = by_wallet.entry(key).or_insert_with(|| LeaderboardEntry {
            agent: r.agent.clone(),
            best_composite: 0,
            best_run_ts_ms: 0,
            runs: 0,
            per_task_best: HashMap::new(),
        });
        entry.runs += 1;
        if r.composite > entry.best_composite {
            entry.best_composite = r.composite;
            entry.best_run_ts_ms = r.ts_ms;
            entry.agent = r.agent.clone();
        }
        for (tid, tr) in &r.tasks {
            let cur = entry.per_task_best.entry(*tid).or_insert(0);
            if tr.score.0 > *cur { *cur = tr.score.0 }
        }
    }
    let mut sorted: Vec<LeaderboardEntry> = by_wallet.into_values().collect();
    sorted.sort_by_key(|e| std::cmp::Reverse(e.best_composite));
    Ok(sorted)
}

/// Trivial scorer that gives partial credit based on a self-reported
/// `passed` boolean and `confidence` 0–1 in the submission payload. Real
/// graders are task-specific and live in the upcoming `runner-tasks` module —
/// this gives the surface a working default while T1–T10 grading code lands.
pub fn naive_grade(sub: &Submission) -> TaskResult {
    let passed = sub.payload.get("passed").and_then(|v| v.as_bool()).unwrap_or(false);
    let conf = sub
        .payload
        .get("confidence")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    let raw = if passed { 7.0 + 3.0 * conf } else { 3.0 * conf };
    let score = Score(raw.round().min(10.0) as u8);
    TaskResult {
        task: sub.task,
        score,
        outcome: TaskOutcome::from_score(score),
        notes: vec![format!("naive grader; payload self-reported passed={passed} confidence={conf:.2}")],
        ts_ms: crate::now_ms(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TaskId;

    #[test]
    fn naive_grade_fail_when_not_passed() {
        let agent = AgentRef { id: "test".into(), wallet: "qnk_t".into() };
        let sub = Submission::new(TaskId(1), agent, serde_json::json!({ "passed": false, "confidence": 0.5 }));
        let r = naive_grade(&sub);
        assert!(r.score.0 <= 2, "fail-not-passed yielded {} (expected ≤2)", r.score.0);
    }

    #[test]
    fn naive_grade_pass_with_high_confidence_perfect_10() {
        let agent = AgentRef { id: "test".into(), wallet: "qnk_t".into() };
        let sub = Submission::new(TaskId(1), agent, serde_json::json!({ "passed": true, "confidence": 1.0 }));
        let r = naive_grade(&sub);
        assert_eq!(r.score.0, 10);
    }

    #[test]
    fn naive_grade_pass_low_confidence_is_partial() {
        let agent = AgentRef { id: "test".into(), wallet: "qnk_t".into() };
        let sub = Submission::new(TaskId(1), agent, serde_json::json!({ "passed": true, "confidence": 0.1 }));
        let r = naive_grade(&sub);
        assert!(r.score.0 >= 7 && r.score.0 <= 8);
    }

    #[test]
    fn ledger_path_respects_env() {
        std::env::set_var("FLUX_AI_BENCH_LEDGER", "/tmp/fab-test.jsonl");
        assert_eq!(ledger_path(), PathBuf::from("/tmp/fab-test.jsonl"));
        std::env::remove_var("FLUX_AI_BENCH_LEDGER");
    }
}
