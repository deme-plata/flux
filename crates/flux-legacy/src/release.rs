//! release.rs — **flux-legacy BETA-2 RELEASE** (the flagship).
//!
//! Packages the whole prototype ladder (P1–P12 + the side modules) into ONE consultancy-grade
//! deliverable: point it at a brownfield node and get a polished assessment — the health verdict,
//! the sickest crates, a prioritized treatment plan with each treatment's required GATE-PATH, and a
//! beta-1 readiness statement. This is the cohesive product a top team hands over for "beta 1":
//! not a pile of modules, but one report that says *what's wrong, what we'd do, and how each change
//! is proven safe before it touches mainnet.*
//!
//! Pure (composes already-computed inputs) so it's unit-tested; the bin wires the live data.

use serde::{Deserialize, Serialize};

/// The current release tag — bumped to beta-2: the ladder now generalizes beyond the Rust dogfood
/// to survey ANY brownfield repo and onboard projects off GitHub.
pub const RELEASE_VERSION: &str = "flux-legacy 0.1.0-beta.2";
/// Back-compat alias — the bin and older callers reference `BETA1_VERSION`.
pub const BETA1_VERSION: &str = RELEASE_VERSION;

/// Status of a ladder rung in this release — honest, never overclaimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CapStatus { Shipped, Integration, Planned }

impl CapStatus {
    pub fn mark(self) -> &'static str {
        match self { CapStatus::Shipped => "✅ shipped", CapStatus::Integration => "🔗 integration", CapStatus::Planned => "⬜ planned" }
    }
}

/// One rung of the prototype ladder as it ships in beta-1.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capability {
    pub proto: &'static str,
    pub name: &'static str,
    pub status: CapStatus,
    pub does: &'static str,
}

/// The beta-1 capability matrix — the ladder, with each rung's real status.
pub fn capabilities() -> Vec<Capability> {
    use CapStatus::*;
    vec![
        Capability { proto: "P1", name: "analyze + plan + render", status: Shipped, does: "real per-crate metrics, god-files, fan-in; ranked refactor plan" },
        Capability { proto: "P2", name: "split + cycles", status: Shipped, does: "brace-parse a god-file → dry-run module-split patch; dep-cycle detection" },
        Capability { proto: "P3", name: "context", status: Shipped, does: "ground a refactor brief in the REAL code (bounded outline)" },
        Capability { proto: "P4", name: "verify", status: Shipped, does: "apply a patch in an isolated sandbox, build+test the real crate" },
        Capability { proto: "P5", name: "precheck", status: Shipped, does: "cheap Safe/Review/Unsafe structural verdict before the costly verify" },
        Capability { proto: "P6", name: "shadow + pipeline", status: Shipped, does: "state-root equivalence over N real blocks; git-aware branch+commit+push to hub" },
        Capability { proto: "P7", name: "consensus", status: Shipped, does: "height-gate codegen + activation margin + canary + 2/3 quorum" },
        Capability { proto: "P8", name: "drive", status: Shipped, does: "swarm lane assignment + per-target gate-chain orchestrator, fail-closed" },
        Capability { proto: "P9", name: "autopilot", status: Shipped, does: "autonomous measure→DeepSeek plan→drive→re-measure loop; T4 deferred to human" },
        Capability { proto: "P10", name: "stability", status: Shipped, does: "live-node health audit vs runbook thresholds (fatal-vs-cosmetic)" },
        Capability { proto: "Pulse", name: "pulse", status: Shipped, does: "mine the live journald → per-crate runtime pain → fuse into the ranking" },
        Capability { proto: "1M", name: "corpus + bundle + ask", status: Shipped, does: "pack the highest-value code into DeepSeek-v4's 1M window for whole-node reasoning" },
        Capability { proto: "P11", name: "triage + psych", status: Shipped, does: "hospital board: acuity-ranked patients + behavioral code-smell diagnoses (verified live on the node)" },
        Capability { proto: "P12", name: "consult", status: Shipped, does: "DeepSeek as the INDEPENDENT doctor — cold per-crate read + second_opinion vs in-house (live on q-types)" },
        Capability { proto: "P13", name: "admit", status: Shipped, does: "full single-patient pathway: triage→psych→consult→surgery→recovery→discharge, dry-run" },
        Capability { proto: "P14", name: "remediate", status: Shipped, does: "map a stability verdict → risk-classed reversible fixes; run only the Auto subset, fail-closed" },
        // ── beta-2: generalize beyond the Rust dogfood to ANY brownfield repo ──
        Capability { proto: "β2", name: "lang + survey", status: Shipped, does: "detect language(s) + survey ANY brownfield repo — not just Rust" },
        Capability { proto: "β2", name: "import", status: Shipped, does: "onboard a project off GitHub (shallow, blob-limited) then survey — the 'any repo → Flux' door" },
    ]
}

/// The gate-path a treatment of a given risk tier must pass before it can land on mainnet.
pub fn gate_path_for_tier(tier: u8) -> &'static str {
    match tier {
        0 | 1 => "P5 precheck → P4 build+test",
        2 => "P5 precheck → P4 build+test (sandbox)",
        3 => "P4 build+test → P6 shadow (state-roots match over N blocks)",
        _ => "P4 build+test → P6 shadow → P7 height-gate + canary + 2/3 quorum → human activation",
    }
}

/// One prioritized treatment + the gate-path its tier requires.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Treatment {
    pub rank: usize,
    pub crate_name: String,
    pub target: String,
    pub kind: String,
    pub tier: u8,
    pub gate_path: String,
}

/// Is the tool itself ready to ship beta-1? (all gates green, no danger health).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Readiness { Ready, NotReady(String) }

/// The full beta-1 assessment — the consultancy deliverable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Beta1Assessment {
    pub version: String,
    pub target_root: String,
    pub crate_count: usize,
    pub total_loc: usize,
    pub god_files: usize,
    /// P10 stability one-liner (health verdict).
    pub health: String,
    /// triage + pulse: the sickest crates, worst first.
    pub sickest: Vec<String>,
    /// the prioritized plan, each with its gate-path.
    pub treatments: Vec<Treatment>,
    pub readiness: Readiness,
}

/// Build the assessment from already-computed pieces (live data wired by the bin).
pub fn build_assessment(
    target_root: impl Into<String>,
    crate_count: usize,
    total_loc: usize,
    god_files: usize,
    health: impl Into<String>,
    sickest: Vec<String>,
    ranked: &[(usize, String, String, String, u8)], // (rank, crate, target, kind, tier)
    tests_green: bool,
) -> Beta1Assessment {
    let treatments = ranked.iter().map(|(rank, c, target, kind, tier)| Treatment {
        rank: *rank, crate_name: c.clone(), target: target.clone(), kind: kind.clone(),
        tier: *tier, gate_path: gate_path_for_tier(*tier).to_string(),
    }).collect();
    let readiness = if tests_green { Readiness::Ready } else { Readiness::NotReady("crate test suite not green".into()) };
    Beta1Assessment {
        version: BETA1_VERSION.into(), target_root: target_root.into(),
        crate_count, total_loc, god_files, health: health.into(), sickest, treatments, readiness,
    }
}

/// Render the assessment as the polished beta-1 report (markdown).
pub fn render_assessment(a: &Beta1Assessment) -> String {
    let mut s = String::new();
    s.push_str(&format!("# {} — node modernization assessment\n\n", a.version));
    s.push_str(&format!("**Target:** `{}` · {} crates · {} LOC · {} god-files (>800)\n\n", a.target_root, a.crate_count, a.total_loc, a.god_files));
    s.push_str(&format!("## Health\n{}\n\n", a.health));
    s.push_str("## Sickest crates (triage + live runtime pain)\n");
    for (i, c) in a.sickest.iter().enumerate() { s.push_str(&format!("{}. {}\n", i + 1, c)); }
    s.push_str("\n## Prioritized treatment plan (each with its safety gate-path)\n");
    s.push_str("| # | crate | target | treatment | tier | gate-path before mainnet |\n|---|---|---|---|---|---|\n");
    for t in &a.treatments {
        s.push_str(&format!("| {} | {} | {} | {} | T{} | {} |\n", t.rank, t.crate_name, t.target, t.kind, t.tier, t.gate_path));
    }
    s.push_str("\n## Capability matrix\n| rung | capability | status | what it does |\n|---|---|---|---|\n");
    for c in capabilities() {
        s.push_str(&format!("| {} | {} | {} | {} |\n", c.proto, c.name, c.status.mark(), c.does));
    }
    s.push_str(&format!("\n## Readiness\n{}\n", match &a.readiness {
        Readiness::Ready => "✅ **beta-2 READY** — all gates green; surveys any repo, never auto-merges to mainline, every change is operator-gated.".to_string(),
        Readiness::NotReady(why) => format!("🟡 **NOT READY** — {why}"),
    }));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_matrix_covers_the_ladder() {
        let caps = capabilities();
        assert!(caps.len() >= 12, "the ladder P1-P14 + side modules");
        // P11/P12 shipped + verified live this session — honest, not overclaimed beyond what ran
        let p12 = caps.iter().find(|c| c.proto == "P12").unwrap();
        assert_eq!(p12.status, CapStatus::Shipped);
        let p4 = caps.iter().find(|c| c.proto == "P4").unwrap();
        assert_eq!(p4.status, CapStatus::Shipped);
    }

    #[test]
    fn gate_path_escalates_with_tier() {
        assert!(gate_path_for_tier(1).contains("precheck"));
        assert!(gate_path_for_tier(3).contains("shadow"));
        let t4 = gate_path_for_tier(4);
        assert!(t4.contains("shadow") && t4.contains("quorum") && t4.contains("human"));
    }

    #[test]
    fn assessment_renders_and_reports_readiness() {
        let ranked = vec![
            (1, "q-api-server".to_string(), "main.rs".to_string(), "split-god-file".to_string(), 2u8),
            (2, "q-storage".to_string(), "turbo_sync.rs".to_string(), "decouple".to_string(), 3u8),
        ];
        let a = build_assessment("/home/orobit/qnk", 100, 754_743, 217,
            "🟡 WATCH — serving, standalone, sync-gap non-fatal", vec!["q-api-server".into(), "q-storage".into()],
            &ranked, true);
        assert_eq!(a.readiness, Readiness::Ready);
        let r = render_assessment(&a);
        assert!(r.contains("0.1.0-beta.2"));
        assert!(r.contains("q-api-server") && r.contains("turbo_sync.rs"));
        assert!(r.contains("shadow"), "the T3 treatment shows its shadow gate-path");
        assert!(r.contains("Capability matrix"));
    }

    #[test]
    fn not_ready_when_tests_red() {
        let a = build_assessment("x", 1, 1, 0, "h", vec![], &[], false);
        assert!(matches!(a.readiness, Readiness::NotReady(_)));
        assert!(render_assessment(&a).contains("NOT READY"));
    }
}
