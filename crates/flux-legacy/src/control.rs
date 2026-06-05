//! control.rs — the operator-driven plan engine. TOTAL CONTROL over the legacy refactor plan.
//!
//! P2 [`plan`](crate::plan) produces a ranked list; on its own that's a *static report*. This module
//! turns it into a plan the OPERATOR drives across sessions: approve / veto / reorder / parameterize
//! each task, persisted to disk, and — critically — re-analysis MERGES new metrics WITHOUT losing a
//! single operator decision (matched by stable task identity). Nothing here touches source code:
//! it records *intent* and computes the working order. A task only becomes eligible for execution
//! when the operator sets it [`TaskStatus::Approved`]; the actuator (split / ai_refactor → P4
//! sandbox-verify → write to canonical source) consumes [`PlanState::approved`] and stays gated.
//!
//! Pure + serde-persisted (serde_json, no new deps). Atomic save via tmp-file + rename.

use crate::RefactorTask;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::Path;

/// Where a task sits in the operator's workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    /// proposed by the planner, no operator decision yet
    #[default]
    Pending,
    /// operator green-lit it → eligible for sandbox-verify + write
    Approved,
    /// operator removed it from the active plan (kept for audit)
    Vetoed,
    /// applied + verified
    Done,
}

/// Stable identity for a refactor across re-analysis. (crate, kind, target) names a refactor
/// uniquely, so operator decisions survive a metrics refresh even as impact/effort change.
pub fn task_id(crate_name: &str, kind: &str, target: &str) -> String {
    format!("{crate_name}::{kind}::{target}")
}

/// One task plus the operator's overlay on it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanEntry {
    pub id: String,
    pub crate_name: String,
    pub kind: String,
    pub target: String,
    pub detail: String,
    /// latest measured impact (refreshed on merge)
    pub impact: f64,
    pub effort: String,
    pub est_minutes: u64,
    /// planner's rank from impact (1 = highest); refreshed on merge
    pub auto_rank: usize,
    /// operator's pinned position; `None` = follow `auto_rank`
    pub operator_rank: Option<usize>,
    pub status: TaskStatus,
    /// per-task parameters the operator sets (e.g. `chunk_size=300`)
    #[serde(default)]
    pub params: BTreeMap<String, String>,
}

/// An audit record of one operator action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionLog {
    pub seq: u64,
    pub action: String,
    pub task_id: String,
}

/// The persisted, operator-driven plan.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlanState {
    pub repo_root: String,
    pub entries: Vec<PlanEntry>,
    pub history: Vec<ActionLog>,
}

/// The operator overlay carried across a re-analysis (everything the planner does NOT own).
type Overlay = (TaskStatus, Option<usize>, BTreeMap<String, String>);

impl PlanState {
    /// Build a fresh plan from a planner output.
    pub fn from_tasks(repo_root: impl Into<String>, tasks: &[RefactorTask]) -> Self {
        let mut s = PlanState { repo_root: repo_root.into(), ..Default::default() };
        s.merge_tasks(tasks);
        s
    }

    /// Refresh metrics from a NEW analysis while preserving every operator decision (status,
    /// pinned rank, params) by task identity. New tasks arrive Pending; tasks no longer produced
    /// by the planner drop out (their history stays). This is what makes the plan survive the repo
    /// changing under it.
    pub fn merge_tasks(&mut self, tasks: &[RefactorTask]) {
        let overlay: HashMap<String, Overlay> = self
            .entries
            .iter()
            .map(|e| (e.id.clone(), (e.status, e.operator_rank, e.params.clone())))
            .collect();

        self.entries = tasks
            .iter()
            .map(|t| {
                let id = task_id(&t.crate_name, &t.kind, &t.target);
                let (status, operator_rank, params) = overlay.get(&id).cloned().unwrap_or_default();
                PlanEntry {
                    id,
                    crate_name: t.crate_name.clone(),
                    kind: t.kind.clone(),
                    target: t.target.clone(),
                    detail: t.detail.clone(),
                    impact: t.impact,
                    effort: t.effort.clone(),
                    est_minutes: t.est_minutes,
                    auto_rank: t.rank,
                    operator_rank,
                    status,
                    params,
                }
            })
            .collect();
    }

    fn idx(&self, id: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.id == id)
    }

    fn log(&mut self, action: &str, id: &str) {
        let seq = self.history.len() as u64 + 1;
        self.history.push(ActionLog { seq, action: action.into(), task_id: id.into() });
    }

    /// Green-light a task for execution. Returns false if the id is unknown.
    pub fn approve(&mut self, id: &str) -> bool {
        self.set_status(id, TaskStatus::Approved, "approve")
    }

    /// Remove a task from the active plan (kept for audit; excluded from [`effective_plan`]).
    pub fn veto(&mut self, id: &str) -> bool {
        self.set_status(id, TaskStatus::Vetoed, "veto")
    }

    /// Mark a task applied + verified.
    pub fn mark_done(&mut self, id: &str) -> bool {
        self.set_status(id, TaskStatus::Done, "done")
    }

    /// Clear any operator decision back to Pending and unpin its rank.
    pub fn reset(&mut self, id: &str) -> bool {
        match self.idx(id) {
            Some(i) => {
                self.entries[i].status = TaskStatus::Pending;
                self.entries[i].operator_rank = None;
                self.log("reset", id);
                true
            }
            None => false,
        }
    }

    fn set_status(&mut self, id: &str, status: TaskStatus, action: &str) -> bool {
        match self.idx(id) {
            Some(i) => {
                self.entries[i].status = status;
                self.log(action, id);
                true
            }
            None => false,
        }
    }

    /// Pin a task to a position; [`effective_plan`] honours it over the planner's auto rank.
    pub fn reorder(&mut self, id: &str, position: usize) -> bool {
        match self.idx(id) {
            Some(i) => {
                self.entries[i].operator_rank = Some(position);
                self.log(&format!("reorder->{position}"), id);
                true
            }
            None => false,
        }
    }

    /// Set a per-task parameter the actuator will read (e.g. `chunk_size` for a split).
    pub fn set_param(&mut self, id: &str, key: &str, value: &str) -> bool {
        match self.idx(id) {
            Some(i) => {
                self.entries[i].params.insert(key.into(), value.into());
                self.log(&format!("param:{key}={value}"), id);
                true
            }
            None => false,
        }
    }

    /// The working plan the operator sees: non-vetoed tasks, ordered by operator pin where set,
    /// else by the planner's impact rank. Stable, deterministic.
    pub fn effective_plan(&self) -> Vec<&PlanEntry> {
        let mut v: Vec<&PlanEntry> =
            self.entries.iter().filter(|e| e.status != TaskStatus::Vetoed).collect();
        v.sort_by(|a, b| {
            let ka = a.operator_rank.unwrap_or(a.auto_rank);
            let kb = b.operator_rank.unwrap_or(b.auto_rank);
            ka.cmp(&kb).then(a.auto_rank.cmp(&b.auto_rank)).then(a.id.cmp(&b.id))
        });
        v
    }

    /// The execution queue: only tasks the operator approved, in effective order.
    pub fn approved(&self) -> Vec<&PlanEntry> {
        self.effective_plan().into_iter().filter(|e| e.status == TaskStatus::Approved).collect()
    }

    /// Persist atomically (tmp-file + rename) so a crash never leaves a half-written plan.
    pub fn save(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let path = path.as_ref();
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)
    }

    /// Load a previously saved plan.
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        let json = std::fs::read_to_string(path)?;
        serde_json::from_str(&json).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(crate_name: &str, kind: &str, target: &str, impact: f64, rank: usize) -> RefactorTask {
        RefactorTask {
            rank,
            crate_name: crate_name.into(),
            kind: kind.into(),
            target: target.into(),
            detail: "d".into(),
            impact,
            effort: "high".into(),
            est_minutes: 180,
        }
    }

    fn sample() -> Vec<RefactorTask> {
        vec![
            task("q-api-server", "split-god-file", "src/main.rs", 0.70, 1),
            task("q-types", "decouple", "q-types", 0.65, 2),
            task("q-storage", "split-god-file", "src/lib.rs", 0.30, 3),
        ]
    }

    #[test]
    fn from_tasks_starts_all_pending_in_rank_order() {
        let s = PlanState::from_tasks("/repo", &sample());
        assert_eq!(s.entries.len(), 3);
        assert!(s.entries.iter().all(|e| e.status == TaskStatus::Pending));
        let order: Vec<_> = s.effective_plan().iter().map(|e| e.target.clone()).collect();
        assert_eq!(order, vec!["src/main.rs", "q-types", "src/lib.rs"]);
    }

    #[test]
    fn approve_and_veto_change_status_and_queue() {
        let mut s = PlanState::from_tasks("/repo", &sample());
        let main_id = task_id("q-api-server", "split-god-file", "src/main.rs");
        let stor_id = task_id("q-storage", "split-god-file", "src/lib.rs");
        assert!(s.approve(&main_id));
        assert!(s.veto(&stor_id));
        // vetoed task drops out of the working plan
        assert!(s.effective_plan().iter().all(|e| e.id != stor_id));
        // approved queue holds exactly the approved one
        let q: Vec<_> = s.approved().iter().map(|e| e.id.clone()).collect();
        assert_eq!(q, vec![main_id]);
        // unknown id is a no-op false, not a panic
        assert!(!s.approve("nope::x::y"));
    }

    #[test]
    fn reorder_pins_over_auto_rank() {
        let mut s = PlanState::from_tasks("/repo", &sample());
        let stor_id = task_id("q-storage", "split-god-file", "src/lib.rs");
        s.reorder(&stor_id, 0); // pin the lowest-impact task to the very top
        let order: Vec<_> = s.effective_plan().iter().map(|e| e.target.clone()).collect();
        assert_eq!(order[0], "src/lib.rs");
    }

    #[test]
    fn reanalysis_preserves_operator_decisions() {
        let mut s = PlanState::from_tasks("/repo", &sample());
        let main_id = task_id("q-api-server", "split-god-file", "src/main.rs");
        s.approve(&main_id);
        s.set_param(&main_id, "chunk_size", "300");
        // repo changed: main.rs shrank (impact down) + a brand-new god-file appeared
        let mut next = sample();
        next[0].impact = 0.40; // refreshed metric
        next.push(task("q-vm", "split-god-file", "src/vm.rs", 0.55, 2));
        s.merge_tasks(&next);
        let main = s.entries.iter().find(|e| e.id == main_id).unwrap();
        assert_eq!(main.status, TaskStatus::Approved, "approval survives re-analysis");
        assert_eq!(main.impact, 0.40, "metric refreshed");
        assert_eq!(main.params.get("chunk_size").map(String::as_str), Some("300"));
        assert!(s.entries.iter().any(|e| e.target == "src/vm.rs"), "new task picked up Pending");
    }

    #[test]
    fn history_logs_every_action() {
        let mut s = PlanState::from_tasks("/repo", &sample());
        let id = task_id("q-types", "decouple", "q-types");
        s.veto(&id);
        s.reset(&id);
        s.approve(&id);
        assert_eq!(s.history.len(), 3);
        assert_eq!(s.history[0].action, "veto");
        assert_eq!(s.history[2].action, "approve");
    }

    #[test]
    fn save_load_roundtrip() {
        let mut s = PlanState::from_tasks("/repo", &sample());
        let id = task_id("q-api-server", "split-god-file", "src/main.rs");
        s.approve(&id);
        let p = std::env::temp_dir().join(format!("flux-legacy-plan-{}.json", std::process::id()));
        s.save(&p).unwrap();
        let back = PlanState::load(&p).unwrap();
        assert_eq!(back.repo_root, "/repo");
        assert_eq!(back.approved().len(), 1);
        let _ = std::fs::remove_file(&p);
    }
}
