//! drive.rs — flux-legacy **P8: SWARM ORCHESTRATOR**.
//!
//! P1–P7 are the primitives; P8 is the DRIVER that composes them: assign god-files to swarm agents
//! (lanes), then run each target through the full gate chain —
//! `precheck (P5) → split (P2) → verify (P4) → [shadow (P6) → consensus (P7) for risky tiers] →
//! sync (P6 pipeline)` — **fail-closed** (stop at the first failed gate), tier-bounded, and
//! dry-run by default (only `--confirm` syncs). This is the driver the upcoming `flux_legacy_*` MCP
//! tools wrap so the swarm can run the whole pipeline.
//!
//! Side effects are injected via [`LegacyOps`] (precheck/split/verify/shadow/consensus/sync), so the
//! orchestration is unit-tested deterministically; the real impl wires the actual lanes.

use crate::RefactorTask;

/// One lane: an agent owns one target task (the swarm claims these over the bus).
#[derive(Debug, Clone)]
pub struct Lane {
    pub agent_id: String,
    pub task: RefactorTask,
}

/// Round-robin the ranked tasks across agents — the lane breakdown a swarm broadcast claims from.
pub fn assign_lanes(tasks: &[RefactorTask], agents: &[String]) -> Vec<Lane> {
    if agents.is_empty() {
        return Vec::new();
    }
    tasks.iter().enumerate()
        .map(|(i, t)| Lane { agent_id: agents[i % agents.len()].clone(), task: t.clone() })
        .collect()
}

/// Where a target ended in the gate chain. Each later stage implies all earlier gates passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriveStage {
    /// tier above the configured ceiling — not attempted.
    Skipped,
    /// staged + verified green, but not synced (dry-run, or operator hasn't confirmed).
    StagedVerified,
    /// passed every required gate AND landed on a refactor branch.
    Synced(String),
    /// a gate failed — `String` says which (fail-closed).
    Rejected(String),
}

impl DriveStage {
    pub fn landed(&self) -> bool { matches!(self, DriveStage::Synced(_)) }
}

/// Drive config — the operator's control knobs.
#[derive(Debug, Clone)]
pub struct DriveConfig {
    /// refuse anything above this risk tier.
    pub max_tier: u8,
    /// at/above this tier, also require shadow (P6) + consensus (P7).
    pub require_consensus_at_tier: u8,
    /// dry-run: stage + verify, but never sync (the default; `--confirm` flips it).
    pub dry_run: bool,
}

impl Default for DriveConfig {
    fn default() -> Self { Self { max_tier: 2, require_consensus_at_tier: 4, dry_run: true } }
}

/// The injected side effects — real impls call the P2/P4/P5/P6/P7 lanes; tests mock them.
pub trait LegacyOps {
    fn tier_of(&self, t: &RefactorTask) -> u8;
    fn precheck_ok(&self, t: &RefactorTask) -> bool;   // P5: not Unsafe
    fn split_ok(&self, t: &RefactorTask) -> bool;      // P2: produced a patch
    fn verify_green(&self, t: &RefactorTask) -> bool;  // P4: sandbox build + tests
    fn shadow_match(&self, t: &RefactorTask) -> bool;  // P6: state-roots match over N blocks
    fn consensus_ok(&self, t: &RefactorTask) -> bool;  // P7: height-gate validated + 2/3 quorum
    fn sync(&self, t: &RefactorTask) -> Result<String, String>; // P6 pipeline: branch + commit + push
}

/// The outcome of driving one target.
#[derive(Debug, Clone)]
pub struct DriveOutcome {
    pub task: RefactorTask,
    pub stage: DriveStage,
}

/// Drive ONE target through the gate chain, stopping at the first failed gate (fail-closed).
pub fn drive_target<O: LegacyOps>(t: &RefactorTask, ops: &O, cfg: &DriveConfig) -> DriveOutcome {
    let stage = drive_stage(t, ops, cfg);
    DriveOutcome { task: t.clone(), stage }
}

fn drive_stage<O: LegacyOps>(t: &RefactorTask, ops: &O, cfg: &DriveConfig) -> DriveStage {
    let tier = ops.tier_of(t);
    if tier > cfg.max_tier {
        return DriveStage::Skipped;
    }
    if !ops.precheck_ok(t) {
        return DriveStage::Rejected("P5 precheck: Unsafe".into());
    }
    if !ops.split_ok(t) {
        return DriveStage::Rejected("P2 split: no patch produced".into());
    }
    if !ops.verify_green(t) {
        return DriveStage::Rejected("P4 verify: sandbox not green".into());
    }
    if tier >= cfg.require_consensus_at_tier {
        if !ops.shadow_match(t) {
            return DriveStage::Rejected("P6 shadow: state-roots diverged".into());
        }
        if !ops.consensus_ok(t) {
            return DriveStage::Rejected("P7 consensus: height-gate/quorum not met".into());
        }
    }
    if cfg.dry_run {
        return DriveStage::StagedVerified; // verified, awaiting operator --confirm
    }
    match ops.sync(t) {
        Ok(branch) => DriveStage::Synced(branch),
        Err(e) => DriveStage::Rejected(format!("P6 sync: {e}")),
    }
}

/// The whole campaign: assign lanes, drive every target, tally.
#[derive(Debug, Clone, Default)]
pub struct CampaignReport {
    pub attempted: usize,
    pub synced: usize,
    pub staged: usize,
    pub rejected: usize,
    pub skipped: usize,
    pub outcomes: Vec<DriveOutcome>,
}

/// Drive all ranked tasks (assigned across `agents`) through the chain under `cfg`.
pub fn drive_campaign<O: LegacyOps>(tasks: &[RefactorTask], agents: &[String], ops: &O, cfg: &DriveConfig) -> CampaignReport {
    let mut r = CampaignReport::default();
    for lane in assign_lanes(tasks, agents) {
        let out = drive_target(&lane.task, ops, cfg);
        r.attempted += 1;
        match &out.stage {
            DriveStage::Synced(_) => r.synced += 1,
            DriveStage::StagedVerified => r.staged += 1,
            DriveStage::Rejected(_) => r.rejected += 1,
            DriveStage::Skipped => r.skipped += 1,
        }
        r.outcomes.push(out);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(kind: &str, krate: &str) -> RefactorTask {
        RefactorTask { rank: 1, crate_name: krate.into(), kind: kind.into(), target: krate.into(), detail: String::new(), impact: 0.9, effort: "high".into(), est_minutes: 60 }
    }

    /// Mock that passes everything, with a configurable tier; flags let tests fail one gate.
    struct Ops { tier: u8, precheck: bool, split: bool, verify: bool, shadow: bool, consensus: bool, sync_ok: bool }
    impl Default for Ops { fn default() -> Self { Self { tier: 2, precheck: true, split: true, verify: true, shadow: true, consensus: true, sync_ok: true } } }
    impl LegacyOps for Ops {
        fn tier_of(&self, _t: &RefactorTask) -> u8 { self.tier }
        fn precheck_ok(&self, _t: &RefactorTask) -> bool { self.precheck }
        fn split_ok(&self, _t: &RefactorTask) -> bool { self.split }
        fn verify_green(&self, _t: &RefactorTask) -> bool { self.verify }
        fn shadow_match(&self, _t: &RefactorTask) -> bool { self.shadow }
        fn consensus_ok(&self, _t: &RefactorTask) -> bool { self.consensus }
        fn sync(&self, t: &RefactorTask) -> Result<String, String> {
            if self.sync_ok { Ok(format!("refactor/{}-split", t.crate_name)) } else { Err("push rejected".into()) }
        }
    }

    #[test]
    fn lanes_round_robin() {
        let tasks = vec![task("add-tests", "a"), task("split-god-file", "b"), task("decouple", "c")];
        let agents = vec!["rocky".to_string(), "codex".to_string()];
        let lanes = assign_lanes(&tasks, &agents);
        assert_eq!(lanes[0].agent_id, "rocky");
        assert_eq!(lanes[1].agent_id, "codex");
        assert_eq!(lanes[2].agent_id, "rocky"); // wraps
        assert!(assign_lanes(&tasks, &[]).is_empty());
    }

    #[test]
    fn green_t2_dry_run_stages_not_syncs() {
        let cfg = DriveConfig::default(); // dry_run, max_tier 2
        let out = drive_target(&task("split-god-file", "q-storage"), &Ops::default(), &cfg);
        assert_eq!(out.stage, DriveStage::StagedVerified);
        assert!(!out.stage.landed());
    }

    #[test]
    fn green_t2_confirm_syncs() {
        let cfg = DriveConfig { dry_run: false, ..DriveConfig::default() };
        let out = drive_target(&task("split-god-file", "q-storage"), &Ops::default(), &cfg);
        assert!(matches!(out.stage, DriveStage::Synced(b) if b == "refactor/q-storage-split"));
    }

    #[test]
    fn fails_closed_at_each_gate() {
        let cfg = DriveConfig { dry_run: false, ..DriveConfig::default() };
        let t = task("split-god-file", "x");
        assert!(matches!(drive_target(&t, &Ops { precheck: false, ..Default::default() }, &cfg).stage, DriveStage::Rejected(s) if s.contains("precheck")));
        assert!(matches!(drive_target(&t, &Ops { split: false, ..Default::default() }, &cfg).stage, DriveStage::Rejected(s) if s.contains("split")));
        assert!(matches!(drive_target(&t, &Ops { verify: false, ..Default::default() }, &cfg).stage, DriveStage::Rejected(s) if s.contains("verify")));
        assert!(matches!(drive_target(&t, &Ops { sync_ok: false, ..Default::default() }, &cfg).stage, DriveStage::Rejected(s) if s.contains("sync")));
    }

    #[test]
    fn tier_above_ceiling_skipped() {
        let cfg = DriveConfig::default(); // max_tier 2
        let out = drive_target(&task("decouple", "x"), &Ops { tier: 4, ..Default::default() }, &cfg);
        assert_eq!(out.stage, DriveStage::Skipped);
    }

    #[test]
    fn t4_requires_shadow_and_consensus() {
        // raise the ceiling so a T4 is attempted; it must clear shadow + consensus
        let cfg = DriveConfig { max_tier: 4, require_consensus_at_tier: 4, dry_run: false };
        let t = task("vdf-change", "q-api-server");
        assert!(matches!(drive_target(&t, &Ops { tier: 4, shadow: false, ..Default::default() }, &cfg).stage, DriveStage::Rejected(s) if s.contains("shadow")));
        assert!(matches!(drive_target(&t, &Ops { tier: 4, consensus: false, ..Default::default() }, &cfg).stage, DriveStage::Rejected(s) if s.contains("consensus")));
        assert!(drive_target(&t, &Ops { tier: 4, ..Default::default() }, &cfg).stage.landed());
    }

    #[test]
    fn campaign_tallies() {
        let tasks = vec![task("split-god-file", "a"), task("split-god-file", "b")];
        let r = drive_campaign(&tasks, &["rocky".into()], &Ops::default(), &DriveConfig::default());
        assert_eq!(r.attempted, 2);
        assert_eq!(r.staged, 2); // dry-run → both staged-verified
        assert_eq!(r.synced + r.rejected + r.skipped, 0);
    }
}
