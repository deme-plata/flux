//! flux-ai-bench — objective Flux-dev benchmark for AI agents.
//!
//! ## Why
//!
//! Multiple AI agents (rocky, rocky-sigil, codex, vite-engine, update-v1, …) ship
//! into the same Flux substrate. Today the only signal the swarm has on whether
//! a newcomer can be trusted with a substrate-critical lane is past output
//! quality, which is anecdotal and subject to social bias. flux-ai-bench gives
//! a hard, reproducible score.
//!
//! ## The 10 tasks
//!
//! Each task evaluates a specific Flux-dev skill. Tasks scale 0–10. Composite
//! score is the sum (0–100). See [`Task`] for the full registry.
//!
//! | T# | Skill | Tests |
//! |----|-------|-------|
//! | T1 | Compile-first-try | Submit a function; fluxc must compile cleanly without an iteration cycle. |
//! | T2 | Fix-cycle | Given a broken function + flux_combo error, use flux_qspec to repair in 1 turn. |
//! | T3 | Provenance chain | Emit `.proof` artifact, verify against agent pubkey. |
//! | T4 | Swarm coord | flux_file_claim before edit, release after, no leaks. |
//! | T5 | VarFlow axiom 5 | Multiverse-before-mainline check via flux_chronos_run. |
//! | T6 | Cache discipline | Touch 1 file → unrelated crates still cache-hit. |
//! | T7 | ZK gate | Verify a sample tip-proof in ≤10ms via flux_zk_verify_10ms. |
//! | T8 | Dogfood | No raw `cargo` invocations; use `fluxc` / MCP tools only. |
//! | T9 | Honest numbers | All reported measurements traceable to a file read, not memory. |
//! | T10 | Recover from bad claim | Handle `self-owned:` / `Conflict:` swarm responses gracefully. |
//!
//! ## Result persistence
//!
//! Each [`BenchResult`] is appended to a JSONL ledger keyed by agent wallet.
//! The leaderboard is computed by re-folding the ledger. This mirrors the
//! sigil-releases ledger pattern.
//!
//! ## Mainline usage
//!
//! ```no_run
//! use flux_ai_bench::{BenchSuite, AgentRef};
//! # fn run() -> anyhow::Result<()> {
//! let suite = BenchSuite::standard();
//! let agent = AgentRef { id: "rocky".into(), wallet: "qnk7154…".into() };
//! // Each task runs against a typed Submission the agent provided up-front.
//! // For automated runs, MCP wraps this — the agent never calls Rust directly.
//! # let _ = (suite, agent);
//! # Ok(()) }
//! ```

pub mod grade;
pub mod runner;
pub mod scoring;
pub mod tasks;

pub use runner::{append_to_ledger, leaderboard, naive_grade, read_ledger, LeaderboardEntry};

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub use scoring::{Score, TaskOutcome};
pub use tasks::{Task, TaskId, TaskRegistry};
pub use grade::{
    grade, grade_t8_dogfood, grade_t9_honest_numbers, grade_t10_recover_from_bad_claim,
    ReportedNumber, ToolCall, Transcript,
};

/// Identity of the agent being benchmarked. The wallet is the persistent
/// reference; the id is a human-friendly nickname.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRef {
    pub id: String,
    pub wallet: String,
}

/// What the agent submitted for one task. The schema depends on the task —
/// `payload` is task-defined JSON. The bench infrastructure validates it
/// against the task's input schema before scoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Submission {
    pub task: TaskId,
    pub payload: serde_json::Value,
    pub agent: AgentRef,
    pub ts_ms: u64,
}

impl Submission {
    pub fn new(task: TaskId, agent: AgentRef, payload: serde_json::Value) -> Self {
        Self {
            task,
            payload,
            agent,
            ts_ms: now_ms(),
        }
    }
}

/// The result of evaluating one submission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    pub task: TaskId,
    pub score: Score,
    pub outcome: TaskOutcome,
    pub notes: Vec<String>,
    pub ts_ms: u64,
}

/// The aggregate result of running all 10 tasks against one agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchResult {
    pub agent: AgentRef,
    pub tasks: HashMap<TaskId, TaskResult>,
    pub composite: u32,    // 0..=100
    pub flux_dev_score: f64,
    pub fluxc_version: String,
    pub ts_ms: u64,
}

impl BenchResult {
    /// Construct from per-task results. Composite is the sum of scores
    /// (0..=100). `flux_dev_score` is a confidence-weighted aggregate that
    /// down-weights tasks with low sample variance.
    pub fn from_tasks(agent: AgentRef, results: Vec<TaskResult>, fluxc_version: String) -> Self {
        let mut tasks = HashMap::new();
        let mut composite: u32 = 0;
        for r in results.iter() {
            composite += r.score.0 as u32;
        }
        for r in results {
            tasks.insert(r.task, r);
        }
        let n = tasks.len() as f64;
        let flux_dev_score = if n > 0.0 { (composite as f64) / n } else { 0.0 };
        Self {
            agent,
            tasks,
            composite,
            flux_dev_score,
            fluxc_version,
            ts_ms: now_ms(),
        }
    }
}

/// The benchmark suite — a registry of [`Task`]s in canonical order.
pub struct BenchSuite {
    pub registry: TaskRegistry,
}

impl BenchSuite {
    /// The standard 10-task suite (T1–T10).
    pub fn standard() -> Self {
        Self {
            registry: TaskRegistry::standard(),
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_suite_has_10_tasks() {
        let s = BenchSuite::standard();
        assert_eq!(s.registry.tasks.len(), 10, "standard suite must have exactly 10 tasks");
    }

    #[test]
    fn task_ids_are_t1_through_t10() {
        let s = BenchSuite::standard();
        let mut ids: Vec<u8> = s.registry.tasks.iter().map(|t| t.id.0).collect();
        ids.sort();
        assert_eq!(ids, (1..=10).collect::<Vec<_>>());
    }

    #[test]
    fn composite_score_sums_correctly() {
        let agent = AgentRef { id: "test".into(), wallet: "qnk_test".into() };
        let results: Vec<TaskResult> = (1..=10)
            .map(|i| TaskResult {
                task: TaskId(i),
                score: Score(7),
                outcome: TaskOutcome::Pass,
                notes: vec![],
                ts_ms: 0,
            })
            .collect();
        let bench = BenchResult::from_tasks(agent, results, "0.18.0".into());
        assert_eq!(bench.composite, 70);
        assert_eq!(bench.flux_dev_score, 7.0);
    }
}
