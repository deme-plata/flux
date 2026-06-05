//! autopilot.rs — flux-legacy **P9: the AUTONOMOUS LEGACY-MODERNIZATION LOOP** (the capstone).
//!
//! Composes the whole ladder into a self-driving loop:
//! ```text
//!   measure (P1 analyze + Pulse rank)
//!        │
//!   master_plan (corpus 1M → DeepSeek-v4 re-rank → prioritized plan)
//!        │
//!   drive (P8: per target → precheck→split→verify→[shadow→consensus]→land)
//!        │
//!   re-measure → loop  until { modernized · budget · stalled · max-iters }
//! ```
//! **Human-gated ONLY at the T4 consensus-activation boundary and any real-money step** — enforced by
//! capping the autonomous tier (`max_tier`); T4+ targets are deferred to the operator, never driven
//! autonomously. Everything T1–T3 runs hands-off, fail-closed, reversible (branch-only).
//!
//! The loop is injected via [`Autopilot`] (measure / master_plan / drive) so it's unit-tested
//! deterministically; the real impl wires P1+Pulse, the corpus→DeepSeek brain, and the P8 driver.

use crate::drive::CampaignReport;
use crate::RefactorTask;

/// Operator-set bounds on the autonomous loop.
#[derive(Debug, Clone)]
pub struct AutopilotConfig {
    pub max_iterations: usize,
    /// autonomous tier ceiling — T4+ is deferred to the human (full-auto should cap at 3).
    pub max_tier: u8,
    /// hard USD cap across all DeepSeek planning calls.
    pub usd_budget_total: f64,
    /// stop when the node has ≤ this many god-files (the "modernized" bar).
    pub target_god_files: usize,
    /// dry-run (stage+verify, never sync) — the safe default.
    pub dry_run: bool,
}

impl Default for AutopilotConfig {
    fn default() -> Self {
        Self { max_iterations: 20, max_tier: 3, usd_budget_total: 5.0, target_god_files: 50, dry_run: true }
    }
}

/// Why the loop stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// god-file count reached the target — the node is modernized enough.
    Modernized,
    /// the DeepSeek planning budget was exhausted.
    BudgetExhausted,
    /// an iteration made no progress (god-files didn't drop) — avoid spinning.
    Stalled,
    /// hit the iteration ceiling.
    MaxIterations,
    /// a planning call failed.
    PlanError(String),
}

/// One pass of the loop.
#[derive(Debug, Clone)]
pub struct IterationReport {
    pub iter: usize,
    pub god_files_before: usize,
    pub god_files_after: usize,
    pub deferred_to_human: usize, // T4+ targets the loop refused to drive
    pub spent_usd: f64,
    pub campaign: CampaignReport,
}

/// The full autopilot run.
#[derive(Debug, Clone, Default)]
pub struct AutopilotReport {
    pub iterations: Vec<IterationReport>,
    pub total_spent_usd: f64,
    pub final_god_files: usize,
    pub stopped: Option<StopReasonWire>,
}

/// Serializable mirror of [`StopReason`] (the report is rendered/persisted).
pub type StopReasonWire = String;

/// The injected "engine": measure, plan (the DeepSeek brain), and drive (P8).
pub trait Autopilot {
    /// P1 analyze + Pulse-fused ranking → `(god_file_count, ranked tasks)`.
    fn measure(&self) -> (usize, Vec<RefactorTask>);
    /// corpus(1M) → DeepSeek re-rank → `(prioritized plan, usd_cost)`. The whole-node brain.
    fn master_plan(&self, tasks: &[RefactorTask]) -> Result<(Vec<RefactorTask>, f64), String>;
    /// P8 drive the (already tier-filtered) plan.
    fn drive(&self, tasks: &[RefactorTask]) -> CampaignReport;
}

/// Run the autonomous loop under `cfg`. Human-gated only via the tier cap (T4+ deferred).
pub fn run_autopilot<A: Autopilot>(engine: &A, cfg: &AutopilotConfig) -> AutopilotReport {
    let mut report = AutopilotReport::default();
    let stop = loop {
        let iter = report.iterations.len();
        if iter >= cfg.max_iterations {
            break StopReason::MaxIterations;
        }
        let (god_before, tasks) = engine.measure();
        if god_before <= cfg.target_god_files {
            report.final_god_files = god_before;
            break StopReason::Modernized;
        }
        let (plan, cost) = match engine.master_plan(&tasks) {
            Ok(p) => p,
            Err(e) => break StopReason::PlanError(e),
        };
        if report.total_spent_usd + cost > cfg.usd_budget_total {
            break StopReason::BudgetExhausted;
        }
        report.total_spent_usd += cost;

        // human-gate: T4+ targets are NOT driven autonomously — deferred to the operator.
        let deferred = plan.iter().filter(|t| tier_of(t) > cfg.max_tier).count();
        let autonomous: Vec<RefactorTask> = plan.into_iter().filter(|t| tier_of(t) <= cfg.max_tier).collect();

        let campaign = engine.drive(&autonomous);
        let (god_after, _) = engine.measure();
        report.iterations.push(IterationReport {
            iter, god_files_before: god_before, god_files_after: god_after,
            deferred_to_human: deferred, spent_usd: cost, campaign,
        });
        report.final_god_files = god_after;

        if god_after >= god_before {
            break StopReason::Stalled; // no progress → don't spin (and don't keep paying DeepSeek)
        }
    };
    report.stopped = Some(stop_label(&stop));
    report
}

/// Heuristic tier from a task's kind (mirrors the master-plan risk tiers). Consensus-touching kinds
/// are T4 (human-gated); structural are T1/T2; logic T3.
pub fn tier_of(t: &RefactorTask) -> u8 {
    let k = t.kind.to_lowercase();
    let consensus_crate = matches!(t.crate_name.as_str(),
        "q-types" | "q-vm" | "q-storage") && (k.contains("vdf") || k.contains("emission") || k.contains("consensus"));
    if k.contains("vdf") || k.contains("emission") || k.contains("consensus") || k.contains("validation") || consensus_crate {
        4
    } else if k.contains("decouple") || k.contains("libp2p") || k.contains("rpc") || k.contains("encoding") {
        3
    } else if k.contains("split") || k.contains("extract") {
        2
    } else {
        1 // rename / add-tests / dead-code / docs
    }
}

fn stop_label(s: &StopReason) -> String {
    match s {
        StopReason::Modernized => "modernized (god-files ≤ target)".into(),
        StopReason::BudgetExhausted => "DeepSeek budget exhausted".into(),
        StopReason::Stalled => "stalled (no progress this iteration)".into(),
        StopReason::MaxIterations => "max iterations".into(),
        StopReason::PlanError(e) => format!("plan error: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drive::CampaignReport;

    fn task(kind: &str, krate: &str) -> RefactorTask {
        RefactorTask { rank: 1, crate_name: krate.into(), kind: kind.into(), target: krate.into(), detail: String::new(), impact: 0.9, effort: "high".into(), est_minutes: 60 }
    }

    /// Engine whose god-file count drops by `step` each iteration (until it hits 0), at `cost`/plan.
    struct Converging { remaining: std::cell::Cell<usize>, step: usize, cost: f64, plan_kinds: Vec<&'static str> }
    impl Autopilot for Converging {
        fn measure(&self) -> (usize, Vec<RefactorTask>) {
            (self.remaining.get(), self.plan_kinds.iter().map(|k| task(k, "q-net")).collect())
        }
        fn master_plan(&self, tasks: &[RefactorTask]) -> Result<(Vec<RefactorTask>, f64), String> {
            Ok((tasks.to_vec(), self.cost))
        }
        fn drive(&self, _tasks: &[RefactorTask]) -> CampaignReport {
            // simulate progress: each drive removes `step` god-files
            self.remaining.set(self.remaining.get().saturating_sub(self.step));
            CampaignReport { attempted: 1, staged: 1, ..Default::default() }
        }
    }

    #[test]
    fn loop_converges_to_modernized() {
        let eng = Converging { remaining: std::cell::Cell::new(60), step: 5, cost: 0.01, plan_kinds: vec!["split-god-file"] };
        let cfg = AutopilotConfig { target_god_files: 50, max_iterations: 20, usd_budget_total: 5.0, max_tier: 3, dry_run: true };
        let r = run_autopilot(&eng, &cfg);
        assert_eq!(r.stopped.as_deref(), Some("modernized (god-files ≤ target)"));
        assert!(r.final_god_files <= 50);
        assert!(!r.iterations.is_empty());
    }

    #[test]
    fn loop_stops_on_budget() {
        let eng = Converging { remaining: std::cell::Cell::new(1000), step: 1, cost: 2.0, plan_kinds: vec!["split-god-file"] };
        let cfg = AutopilotConfig { target_god_files: 0, usd_budget_total: 3.0, max_iterations: 100, max_tier: 3, dry_run: true };
        let r = run_autopilot(&eng, &cfg);
        assert_eq!(r.stopped.as_deref(), Some("DeepSeek budget exhausted"));
        assert!(r.total_spent_usd <= 3.0);
    }

    #[test]
    fn loop_stops_when_stalled() {
        // step 0 → drive makes no progress → must stop, not spin forever
        let eng = Converging { remaining: std::cell::Cell::new(100), step: 0, cost: 0.01, plan_kinds: vec!["split-god-file"] };
        let cfg = AutopilotConfig { target_god_files: 10, max_iterations: 100, usd_budget_total: 99.0, max_tier: 3, dry_run: true };
        let r = run_autopilot(&eng, &cfg);
        assert_eq!(r.stopped.as_deref(), Some("stalled (no progress this iteration)"));
        assert_eq!(r.iterations.len(), 1);
    }

    #[test]
    fn t4_is_deferred_to_human() {
        assert_eq!(tier_of(&task("vdf-change", "q-api-server")), 4);
        assert_eq!(tier_of(&task("emission-curve", "q-storage")), 4);
        assert_eq!(tier_of(&task("decouple", "q-types")), 3);
        assert_eq!(tier_of(&task("split-god-file", "q-api-server")), 2);
        assert_eq!(tier_of(&task("add-tests", "q-net")), 1);

        // a plan full of T4 work + max_tier 3 → nothing driven, all deferred, no progress → stalled
        let eng = Converging { remaining: std::cell::Cell::new(100), step: 5, cost: 0.01, plan_kinds: vec!["vdf-change"] };
        let cfg = AutopilotConfig { target_god_files: 10, max_iterations: 5, usd_budget_total: 9.0, max_tier: 3, dry_run: true };
        let r = run_autopilot(&eng, &cfg);
        assert!(r.iterations[0].deferred_to_human >= 1, "T4 deferred to the operator, not auto-driven");
    }
}
