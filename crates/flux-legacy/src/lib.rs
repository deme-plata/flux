//! flux-legacy — bring flux's refactor/combo powers to an arbitrary LEGACY cargo workspace.
//!
//! The flux toolchain analyzes its own dogfood tree; `flux-legacy` points the same lens at a
//! brownfield repo it didn't build — e.g. the 101-crate Quillon Graph node. It walks
//! `<root>/crates/*`, measures each crate with REAL metrics (no stubs), finds god-files and
//! coupling, and (via the [`plan`] lane) ranks what to refactor first.
//!
//! Shared shape lives here so every lane (LEGACY-1..6) builds against the same types.

pub mod analyze;
/// BETA 2: multi-language — detect language(s) from markers/extensions + language-agnostic survey
/// (files/LOC/god-files) so flux-legacy works on ANY repo, not just Rust/cargo.
pub mod lang;
/// BETA 2: pull any project off GitHub — normalize a repo ref + shallow blob-filtered clone.
pub mod import;
/// BETA 2: analyze ANY repo into the same LegacyReport/bundle the hospital + 1M bridge consume.
/// analyze_auto / bundle_auto route Rust-workspace → precise, everything else → generic.
pub mod project;
/// repo-wide LLM analysis bundle: pack the highest-value code into a 1M-token window
/// (integrates flux-context + context::outline). For feeding DeepSeek / Claude.
pub mod corpus;
/// send the corpus to DeepSeek v4's 1M window for a whole-node analysis (legacy × context).
pub mod ask;
/// PROTOTYPE 10: live-node STABILITY audit — disk/RAM/stall/peers/serving/authoritative-DB vs the
/// runbook thresholds → fatal-vs-cosmetic verdict. Tailored for the node-maintenance job.
pub mod stability;
/// PROTOTYPE 12: SAFE REMEDIATION — map a stability verdict → risk-classed reversible fixes; run only
/// the Auto subset (fail-closed), prepare NeedHuman, refuse Forbidden (DB/balance/consensus).
pub mod remediate;
/// LEGACY HEALTH (hospital): ER triage — each crate a patient, vitals + acuity, sickest first.
pub mod triage;
/// PSYCHIATRY clinic: behavioral pathology in "wicked" code (unsafe/swallowed-errors/todo!/panic-spam)
/// → a DSM-ish diagnosis + medication + the real refactor that heals it back to normal.
pub mod psych;
/// CONSULT: DeepSeek as the INDEPENDENT consulting physician — examines one patient cold, then
/// second_opinion() reconciles its read against the in-house triage/psych.
pub mod consult;
/// ADMIT: the full clinical pathway for ONE patient — triage→psych→consult→surgery→recovery→
/// discharge, dry-run, human-readable. The hospital's single-patient capstone.
pub mod admit;
/// PROTOTYPE 3: topo-sort the dep graph into parallel build layers + cycle blockers.
pub mod buildplan;
pub mod context;
pub mod execute;
/// PULSE: mine the live node's journald stream → per-crate runtime-pain → fuse into the ranking.
pub mod pulse;
pub mod stabilize;
/// MEGA-CONTEXT FEED: materialize a corpus manifest + feed deepseek-v4-flash's 1M window.
pub mod bundle;
/// PROTOTYPE 4: in-crate verify — apply a patch in an isolated sandbox, build+test the real crate.
pub mod verify;
/// PROTOTYPE 6: shadow verify — candidate vs canonical state-roots over N real blocks (T3 gate).
pub mod shadow;
/// PROTOTYPE 7: consensus-critical gate — height-gate codegen + activation margin + canary + 2/3 quorum.
pub mod consensus;
/// PROTOTYPE 8: swarm orchestrator — assign lanes + drive each target precheck→…→sync, fail-closed.
pub mod drive;
/// PROTOTYPE 9 (capstone): autonomous loop — measure→DeepSeek master-plan→drive→re-measure→loop;
/// human-gated only at T4/real-money via the tier cap.
pub mod autopilot;
/// BETA-1 RELEASE: package P1-P12 into one consultancy-grade node-modernization assessment + manifest.
pub mod release;
/// PROTOTYPE 6 (land half): git-aware apply + SYNC — branch a verified refactor, commit, push to
/// the bare hub (Epsilon→hub→Beta). shadow/verify gate; pipeline lands. Reversible, --confirm-gated.
pub mod pipeline;
/// PROTOTYPE 5: cheap structural pre-check of a split patch (runs in front of `verify`).
pub mod precheck;
pub mod plan;
pub mod cycles;
pub mod ai_refactor;
/// corpus ⇄ DeepSeek 1M-context bridge: reason over a whole packed subsystem, get a cited diff.
pub mod ai_analyze;
/// TOTAL CONTROL: operator-driven, persisted plan (approve/veto/reorder), survives re-analysis.
pub mod control;
pub mod render;
/// PROTOTYPE 2 actuator: split a god-file into cohesive modules (dry-run patch + staging).
pub mod split;

use serde::{Deserialize, Serialize};

/// A single `.rs` file large enough to be a refactor target on its own.
pub const GOD_FILE_LOC: usize = 800;

/// The whole-workspace analysis result.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LegacyReport {
    pub root: String,
    pub workspace_name: String,
    pub crate_count: usize,
    pub total_loc: usize,
    pub crates: Vec<LegacyCrate>,
    /// Single files over [`GOD_FILE_LOC`], worst-first.
    pub god_files: Vec<GodFile>,
    pub analyze_ms: u128,
}

/// Per-crate measured metrics. All fields are observed, none predicted.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LegacyCrate {
    pub name: String,
    pub path: String,
    pub loc: usize,
    pub file_count: usize,
    pub biggest_file: String,
    pub biggest_file_loc: usize,
    pub pub_fns: usize,
    /// pub struct + pub enum + pub trait
    pub pub_types: usize,
    pub has_tests: bool,
    /// intra-workspace path-dependencies (crate names), parsed from this crate's Cargo.toml
    pub deps: Vec<String>,
    /// crates in this workspace that depend on this one (fan-in), filled by inverting `deps`
    pub dependents: Vec<String>,
}

/// A `.rs` file over the god-file threshold.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GodFile {
    pub crate_name: String,
    pub file: String,
    pub loc: usize,
}

/// One prioritized refactor action (produced by the [`plan`] lane).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RefactorTask {
    pub rank: usize,
    pub crate_name: String,
    /// "split-god-file" | "add-tests" | "decouple" | …
    pub kind: String,
    /// file or crate the action targets
    pub target: String,
    pub detail: String,
    /// 0–1, how much this improves the architecture
    pub impact: f64,
    /// "low" | "medium" | "high"
    pub effort: String,
    pub est_minutes: u64,
}

pub use analyze::analyze_workspace_legacy;
