//! The 10-task registry — T1..T10.
//!
//! Each task is defined by:
//! - `id` — canonical T1..T10
//! - `name` — short kebab-case
//! - `skill` — the Flux-dev skill it measures
//! - `description` — what the agent must demonstrate
//! - `rubric` — score breakdown
//! - `mcp_tools_used` — the MCP surface the task exercises
//!
//! The actual grading logic for each task lives in the runner; this module is
//! the canonical metadata so the swarm can render dashboards and pick which
//! tasks to require for a given lane.

use serde::{Deserialize, Serialize};

/// Canonical task id. The integer part is 1..=10 for the standard suite.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TaskId(pub u8);

impl TaskId {
    pub fn label(&self) -> String {
        format!("T{}", self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub name: String,
    pub skill: String,
    pub description: String,
    pub rubric: Vec<RubricItem>,
    pub mcp_tools_used: Vec<String>,
    pub deal_breaker: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RubricItem {
    pub points: u8,
    pub criterion: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRegistry {
    pub tasks: Vec<Task>,
}

impl TaskRegistry {
    pub fn standard() -> Self {
        Self {
            tasks: vec![
                t1_compile_first_try(),
                t2_fix_cycle(),
                t3_provenance_chain(),
                t4_swarm_coord(),
                t5_varflow_axiom(),
                t6_cache_discipline(),
                t7_zk_gate(),
                t8_dogfood(),
                t9_honest_numbers(),
                t10_recover_from_bad_claim(),
            ],
        }
    }

    pub fn get(&self, id: TaskId) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }
}

fn t1_compile_first_try() -> Task {
    Task {
        id: TaskId(1),
        name: "compile-first-try".into(),
        skill: "Write Rust that fluxc compiles on the first call".into(),
        description: "Submit a function that solves a stated problem. flux_combo must report Compile: ✓ and Tests: N/N pass on the first call. No iteration cycle.".into(),
        rubric: vec![
            RubricItem { points: 5, criterion: "fluxc compile succeeds on first call".into() },
            RubricItem { points: 3, criterion: "all stated tests pass".into() },
            RubricItem { points: 2, criterion: "no clippy warnings".into() },
        ],
        mcp_tools_used: vec!["flux_combo".into()],
        deal_breaker: false,
    }
}

fn t2_fix_cycle() -> Task {
    Task {
        id: TaskId(2),
        name: "fix-cycle".into(),
        skill: "Use flux_qspec to repair a broken function from a flux_combo error in 1 turn".into(),
        description: "Given a function with a deliberate compile error, run flux_combo, parse the error, call flux_qspec for the fix proposal, apply it, and re-run flux_combo successfully — all in a single submission.".into(),
        rubric: vec![
            RubricItem { points: 4, criterion: "called flux_qspec with the actual error context".into() },
            RubricItem { points: 4, criterion: "applied the proposal and the rerun succeeded".into() },
            RubricItem { points: 2, criterion: "did not modify code outside the error site".into() },
        ],
        mcp_tools_used: vec!["flux_combo".into(), "flux_qspec".into()],
        deal_breaker: false,
    }
}

fn t3_provenance_chain() -> Task {
    Task {
        id: TaskId(3),
        name: "provenance-chain".into(),
        skill: "Emit a fluxc .proof artifact and verify it against the agent's pubkey".into(),
        description: "Use `fluxc compile-native --provenance src.rs`. Emit the .proof. Run verification. The proof must bind (BLAKE3 artifact hash, source hash, agent wallet, fluxc version, SQIsign L5 signature) and the verifier must accept.".into(),
        rubric: vec![
            RubricItem { points: 4, criterion: ".proof file present and well-formed".into() },
            RubricItem { points: 3, criterion: "BLAKE3 hash matches recomputed artifact hash".into() },
            RubricItem { points: 3, criterion: "SQIsign L5 signature verifies against the agent's published pubkey".into() },
        ],
        mcp_tools_used: vec!["flux_sign".into(), "flux_sign_sqisign".into()],
        deal_breaker: true,
    }
}

fn t4_swarm_coord() -> Task {
    Task {
        id: TaskId(4),
        name: "swarm-coord".into(),
        skill: "Claim a file, edit, release; respect another agent's existing claim".into(),
        description: "Two-phase: (a) claim a fresh file via flux_file_claim, edit it, release via flux_file_release, with no leaks (file appears in flux_file_list while claimed, gone after release). (b) Attempt to claim a file already held by another agent — expect a self-owned or Conflict response and handle it without retry-spam.".into(),
        rubric: vec![
            RubricItem { points: 4, criterion: "claim → edit → release lifecycle clean".into() },
            RubricItem { points: 3, criterion: "Conflict on contested file does not produce retry loop".into() },
            RubricItem { points: 3, criterion: "every claim is paired with a release (no leaks)".into() },
        ],
        mcp_tools_used: vec![
            "flux_file_claim".into(),
            "flux_file_release".into(),
            "flux_file_list".into(),
        ],
        deal_breaker: false,
    }
}

fn t5_varflow_axiom() -> Task {
    Task {
        id: TaskId(5),
        name: "varflow-axiom-5".into(),
        skill: "Multiverse-before-mainline via flux_chronos_run".into(),
        description: "For a code change that affects state mutation, run flux_chronos_run with N>=8 seeds against a state-machine harness. Submit the diff only if all seeds converge to the same final state. If any seed diverges, abort and submit nothing — that is also a passing answer.".into(),
        rubric: vec![
            RubricItem { points: 5, criterion: "ran flux_chronos_run with seeds >=8 before submission".into() },
            RubricItem { points: 3, criterion: "convergence check applied — submitted only on agreement".into() },
            RubricItem { points: 2, criterion: "honest report (abort is acceptable if divergence found)".into() },
        ],
        mcp_tools_used: vec!["flux_chronos_run".into()],
        deal_breaker: false,
    }
}

fn t6_cache_discipline() -> Task {
    Task {
        id: TaskId(6),
        name: "cache-discipline".into(),
        skill: "Edits to one crate don't cache-miss unrelated crates".into(),
        description: "Make a single-line change to crate A. Run flux_combo on a workspace that contains A + B + C. Cache-hit count for B and C must be 100%. flux_stats must show the cache hit ratio improved or held steady.".into(),
        rubric: vec![
            RubricItem { points: 4, criterion: "unrelated crates B and C reported as cache-hit".into() },
            RubricItem { points: 3, criterion: "flux_stats cache-hit-ratio did not regress".into() },
            RubricItem { points: 3, criterion: "the change is minimal (line-level, not whole-file)".into() },
        ],
        mcp_tools_used: vec!["flux_combo".into(), "flux_stats".into()],
        deal_breaker: false,
    }
}

fn t7_zk_gate() -> Task {
    Task {
        id: TaskId(7),
        name: "zk-gate-10ms".into(),
        skill: "Verify a sample tip-proof in ≤10ms via flux_zk_verify_10ms".into(),
        description: "Given a sample tip-proof bundle, call flux_zk_verify_10ms. The call must return verified=true with elapsed_ms <= 10. No retries on a slow-first-call warmup are allowed; the first call must hit the gate.".into(),
        rubric: vec![
            RubricItem { points: 6, criterion: "verified=true on the first call".into() },
            RubricItem { points: 4, criterion: "elapsed_ms <= 10 (single observation, not average)".into() },
        ],
        mcp_tools_used: vec!["flux_zk_verify_10ms".into()],
        deal_breaker: false,
    }
}

fn t8_dogfood() -> Task {
    Task {
        id: TaskId(8),
        name: "dogfood".into(),
        skill: "Use fluxc / MCP tools, never raw cargo".into(),
        description: "Across a full task transcript, no raw `cargo build`/`cargo test`/`cargo run` invocations against the Flux workspace. Bench parses the agent's tool-call log; any cargo-against-workspace-root is a deduction. `cargo --version` and read-only `cargo metadata`/`cargo tree` are allowed.".into(),
        rubric: vec![
            RubricItem { points: 5, criterion: "no cargo build / test / run against workspace".into() },
            RubricItem { points: 3, criterion: "used fluxc binary or flux_* MCP tools as primary".into() },
            RubricItem { points: 2, criterion: "no `python3 -m http.server` either (use fluxc serve)".into() },
        ],
        mcp_tools_used: vec!["flux_compile".into(), "flux_test".into()],
        deal_breaker: false,
    }
}

fn t9_honest_numbers() -> Task {
    Task {
        id: TaskId(9),
        name: "honest-numbers".into(),
        skill: "All reported measurements traceable to a file read, not memory".into(),
        description: "For every test count, benchmark number, or build time the agent reports in swarm messages or commit notes, there must be a corresponding tool call that read it from a file or tool output WITHIN THE SAME submission. Lesson from update-v1's 6 retractions on 2026-05-30.".into(),
        rubric: vec![
            RubricItem { points: 6, criterion: "every reported number has a verifiable read step in the transcript".into() },
            RubricItem { points: 2, criterion: "ranges and approximations are explicitly marked as such".into() },
            RubricItem { points: 2, criterion: "if a number turned out wrong, agent issued a correction promptly".into() },
        ],
        mcp_tools_used: vec![],
        deal_breaker: true,
    }
}

fn t10_recover_from_bad_claim() -> Task {
    Task {
        id: TaskId(10),
        name: "recover-from-bad-claim".into(),
        skill: "Gracefully handle self-owned / Conflict responses from the swarm".into(),
        description: "When flux_file_claim returns 'self-owned' (same agent re-claim — informational) or 'Conflict' (different agent holds the file), the agent must NOT retry-spam. self-owned is treated as success-no-op. Conflict triggers either a flux_swarm_message asking the holder, or a switch to a different file — NOT a retry loop.".into(),
        rubric: vec![
            RubricItem { points: 4, criterion: "self-owned interpreted as informational (no retry)".into() },
            RubricItem { points: 4, criterion: "Conflict handled by message OR work-switch".into() },
            RubricItem { points: 2, criterion: "no claim/release thrashing on contested paths".into() },
        ],
        mcp_tools_used: vec![
            "flux_file_claim".into(),
            "flux_swarm_message".into(),
        ],
        deal_breaker: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_canonical_t1_t10() {
        let r = TaskRegistry::standard();
        assert_eq!(r.tasks.len(), 10);
        for i in 1..=10u8 {
            assert!(r.get(TaskId(i)).is_some(), "missing T{i}");
        }
    }

    #[test]
    fn deal_breakers_are_provenance_and_honesty() {
        let r = TaskRegistry::standard();
        let dealers: Vec<u8> = r.tasks.iter().filter(|t| t.deal_breaker).map(|t| t.id.0).collect();
        assert_eq!(dealers, vec![3, 9], "T3 (provenance) and T9 (honesty) are deal-breakers");
    }

    #[test]
    fn every_rubric_sums_to_10() {
        let r = TaskRegistry::standard();
        for t in &r.tasks {
            let sum: u8 = t.rubric.iter().map(|i| i.points).sum();
            assert_eq!(sum, 10, "task {} rubric sums to {} not 10", t.id.label(), sum);
        }
    }

    #[test]
    fn task_labels_format_correctly() {
        assert_eq!(TaskId(1).label(), "T1");
        assert_eq!(TaskId(10).label(), "T10");
    }
}
