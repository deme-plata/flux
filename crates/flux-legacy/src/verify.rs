//! verify.rs — flux-legacy **P4: IN-CRATE VERIFY**.
//!
//! P3 grounds a refactor in real code; but flux-moe's pipeline gate is a *standalone*
//! `rustc --crate-type=lib`, which can't compile a change that references the crate's own
//! types/deps. P4 is the REAL gate: apply the proposed [`CratePatch`] to an **isolated sandbox** of
//! the legacy repo (a `git worktree` on a scratch branch, or a recursive copy), run that crate's
//! OWN build (+ tests), and classify. The real workspace is never mutated; rollback = discard the
//! sandbox. Fail-closed: anything but [`VerifyOutcome::Green`] rejects.
//!
//! Side-effecting steps shell out (`git`, the build cmd); the pure parts (path-safety, output
//! parsing, command templating) are unit-tested, and the orchestrator is tested end-to-end with
//! trivial `true`/`false` commands — no real cargo build needed to prove the gate logic.

use std::path::{Path, PathBuf};
use std::process::Command;

/// An edit set a proposer produced for one crate: `(repo-relative path → new file content)`.
#[derive(Debug, Clone)]
pub struct CratePatch {
    pub crate_name: String,
    pub edits: Vec<(String, String)>,
}

/// An isolated checkout where a patch is applied + verified without touching the real tree.
#[derive(Debug, Clone)]
pub struct CrateSandbox {
    pub repo_root: PathBuf,
    pub work_dir: PathBuf,
    pub is_worktree: bool,
    pub branch: String,
}

/// The verdict of an in-crate verification. Only [`Green`](VerifyOutcome::Green) may land.
#[derive(Debug, Clone, serde::Serialize)]
pub enum VerifyOutcome {
    Green { build_ms: u128, tests_passed: u32 },
    BuildFailed { log: String },
    TestsFailed { log: String, passed: u32, failed: u32 },
    Timeout,
    Error(String),
}

impl VerifyOutcome {
    pub fn green(&self) -> bool { matches!(self, VerifyOutcome::Green { .. }) }
    pub fn label(&self) -> &'static str {
        match self {
            VerifyOutcome::Green { .. } => "GREEN (build + tests)",
            VerifyOutcome::BuildFailed { .. } => "build failed",
            VerifyOutcome::TestsFailed { .. } => "tests failed",
            VerifyOutcome::Timeout => "timed out",
            VerifyOutcome::Error(_) => "error",
        }
    }
}

/// How to build + test the patched crate. `{crate}` in the commands is replaced with the crate name.
#[derive(Debug, Clone)]
pub struct VerifyConfig {
    pub run_tests: bool,
    pub timeout_s: u64,
    pub build_cmd: String,
    pub test_cmd: Option<String>,
}

impl Default for VerifyConfig {
    fn default() -> Self {
        // NOTE: the legacy node is a cargo workspace; the flux dogfood tree uses fluxc. Caller picks.
        Self {
            run_tests: true,
            timeout_s: 1800, // q-api-server is huge — a real build is minutes
            build_cmd: "cargo build -p {crate}".into(),
            test_cmd: Some("cargo test -p {crate}".into()),
        }
    }
}

/// Replace `{crate}` in a command template.
pub fn cmd_for(template: &str, crate_name: &str) -> String {
    template.replace("{crate}", crate_name)
}

/// Apply a patch's edits into `work_dir`. Rejects any path that escapes the sandbox (absolute or
/// containing `..`) — a proposed patch must never write outside its isolated checkout.
pub fn apply_patch(work_dir: &Path, patch: &CratePatch) -> Result<(), String> {
    for (rel, content) in &patch.edits {
        if rel.starts_with('/') || rel.split(['/', '\\']).any(|c| c == "..") {
            return Err(format!("unsafe patch path (escapes sandbox): {rel}"));
        }
        let dest = work_dir.join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {parent:?}: {e}"))?;
        }
        std::fs::write(&dest, content).map_err(|e| format!("write {dest:?}: {e}"))?;
    }
    Ok(())
}

/// Parse cargo's `test result: ok. N passed; M failed; …` lines (summed across test binaries).
pub fn parse_test_counts(log: &str) -> (u32, u32) {
    let num_before = |line: &str, marker: &str| -> u32 {
        line.find(marker)
            .and_then(|i| line[..i].rsplit(|c: char| !c.is_ascii_digit()).find(|s| !s.is_empty()))
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0)
    };
    let (mut p, mut f) = (0u32, 0u32);
    for line in log.lines() {
        if line.contains(" passed") { p += num_before(line, " passed"); }
        if line.contains(" failed") { f += num_before(line, " failed"); }
    }
    (p, f)
}

fn tail(s: &str, n: usize) -> String {
    let c: Vec<char> = s.chars().collect();
    if c.len() <= n { s.to_string() } else { c[c.len() - n..].iter().collect() }
}

/// Run `cmd` (via bash, wrapped in `timeout`) in `cwd`. Returns `(success, combined_log, timed_out)`.
fn run(cmd: &str, cwd: &Path, timeout_s: u64) -> (bool, String, bool) {
    let full = format!("timeout {timeout_s} {cmd}");
    match Command::new("bash").arg("-c").arg(&full).current_dir(cwd).output() {
        Ok(o) => {
            let mut log = String::from_utf8_lossy(&o.stdout).into_owned();
            log.push_str(&String::from_utf8_lossy(&o.stderr));
            let timed_out = o.status.code() == Some(124); // GNU timeout exit code
            (o.status.success(), log, timed_out)
        }
        Err(e) => (false, format!("spawn failed: {e}"), false),
    }
}

/// THE GATE: apply `patch` to the sandbox, build (+ optionally test) the crate, classify the result.
/// The real workspace is untouched — all writes land in `sb.work_dir`.
pub fn verify_in_crate(sb: &CrateSandbox, patch: &CratePatch, cfg: &VerifyConfig) -> VerifyOutcome {
    if let Err(e) = apply_patch(&sb.work_dir, patch) {
        return VerifyOutcome::Error(e);
    }
    let t0 = std::time::Instant::now();
    let (bok, blog, bto) = run(&cmd_for(&cfg.build_cmd, &patch.crate_name), &sb.work_dir, cfg.timeout_s);
    if bto { return VerifyOutcome::Timeout; }
    let build_ms = t0.elapsed().as_millis();
    if !bok { return VerifyOutcome::BuildFailed { log: tail(&blog, 1500) }; }

    if cfg.run_tests {
        if let Some(tc) = &cfg.test_cmd {
            let (tok, tlog, tto) = run(&cmd_for(tc, &patch.crate_name), &sb.work_dir, cfg.timeout_s);
            if tto { return VerifyOutcome::Timeout; }
            let (passed, failed) = parse_test_counts(&tlog);
            if !tok {
                return VerifyOutcome::TestsFailed { log: tail(&tlog, 1500), passed, failed };
            }
            return VerifyOutcome::Green { build_ms, tests_passed: passed };
        }
    }
    VerifyOutcome::Green { build_ms, tests_passed: 0 }
}

/// Open an isolated sandbox: a `git worktree` on a scratch branch if `repo_root` is a git repo,
/// else a recursive copy. The caller must [`sandbox_close`] it.
pub fn sandbox_open(repo_root: &Path) -> Result<CrateSandbox, String> {
    let pid = std::process::id();
    let work_dir = std::env::temp_dir().join(format!("flux-legacy-verify-{pid}"));
    let _ = std::fs::remove_dir_all(&work_dir);
    let wd = work_dir.to_str().ok_or("bad work dir path")?;
    if repo_root.join(".git").exists() {
        let branch = format!("flux-legacy/verify-{pid}");
        let out = Command::new("git").current_dir(repo_root)
            .args(["worktree", "add", "-b", &branch, wd])
            .output().map_err(|e| format!("git worktree: {e}"))?;
        if !out.status.success() {
            return Err(format!("git worktree add failed: {}", String::from_utf8_lossy(&out.stderr)));
        }
        Ok(CrateSandbox { repo_root: repo_root.into(), work_dir, is_worktree: true, branch })
    } else {
        let out = Command::new("cp").args(["-a", repo_root.to_str().unwrap_or(""), wd])
            .output().map_err(|e| format!("cp: {e}"))?;
        if !out.status.success() {
            return Err(format!("copy failed: {}", String::from_utf8_lossy(&out.stderr)));
        }
        Ok(CrateSandbox { repo_root: repo_root.into(), work_dir, is_worktree: false, branch: String::new() })
    }
}

/// Discard a sandbox: remove the worktree + scratch branch, or delete the copy. Idempotent.
pub fn sandbox_close(sb: CrateSandbox) {
    if sb.is_worktree {
        let _ = Command::new("git").current_dir(&sb.repo_root)
            .args(["worktree", "remove", "--force", sb.work_dir.to_str().unwrap_or("")]).output();
        let _ = Command::new("git").current_dir(&sb.repo_root)
            .args(["branch", "-D", &sb.branch]).output();
    } else {
        let _ = std::fs::remove_dir_all(&sb.work_dir);
    }
}

/// A ConsensusGate verdict - the third gate in the GateRunner chain
/// (CompileGate -> TestGate -> ConsensusGate). It turns a Chronos deterministic
/// network-sim result into pass/fail: a consensus/network-affecting DiffUnit lands
/// only if the sim stays DETERMINISTIC (converged: same tx-arrival order -> byte-
/// identical node snapshots, the chain invariant in TourbillonReport) AND delivery
/// holds at/above the runbook floor. The sim is flux-chronos (seed 42, virtual time);
/// the GateRunner runs it on the patched node and feeds the result here.
/// Proven baseline: 16 nodes, 20% loss, redundancy 3 -> 99.6% delivery, converged.
#[derive(Debug, Clone, PartialEq)]
pub struct ChronosVerdict {
    pub delivery_pct: f64,
    pub converged: bool,
    pub threshold_pct: f64,
    pub passed: bool,
}

/// Decide the ConsensusGate from a Chronos sim result (delivery + convergence).
/// FAIL-CLOSED: non-converged (the sim diverged under tx-order permutation) OR
/// delivery below the floor => the gate FAILS. Pure, no I/O.
pub fn chronos_verdict(delivery_pct: f64, converged: bool, threshold_pct: f64) -> ChronosVerdict {
    let passed = converged && delivery_pct >= threshold_pct;
    ChronosVerdict { delivery_pct, converged, threshold_pct, passed }
}

/// The full GateRunner chain for one DiffUnit: CompileGate + TestGate
/// (`verify_in_crate` GREEN) must pass, and IF a ConsensusGate ran (a
/// consensus/network-affecting change), it must pass too. T1/T2 changes pass `None`.
pub fn gate_chain_passes(verify: &VerifyOutcome, consensus: Option<&ChronosVerdict>) -> bool {
    verify.green() && consensus.map_or(true, |c| c.passed)
}

// ===== P7 - consensus-critical safety layer (master-plan): height-gate + canary + 2/3 =====

/// Height-gating: a consensus change ships COMPILED-BUT-DISABLED behind a block-height
/// activation flag. Validators upgrade async before `activation_height`; the new code path
/// runs only at/after it. (The runbook's q-consensus-guard rule.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeightGate {
    pub upgrade: String,
    pub activation_height: u64,
}
impl HeightGate {
    /// Active at `current_height`? Before activation the old path runs (the change is dark).
    pub fn is_active(&self, current_height: u64) -> bool { current_height >= self.activation_height }
    /// Safety margin (blocks) validators still have before activation to upgrade.
    pub fn blocks_until_active(&self, current_height: u64) -> u64 {
        self.activation_height.saturating_sub(current_height)
    }
}

/// Canary rollout stage: a T4 change rolls out staged (one non-validator -> 3 validators ->
/// full), watching Pulse between stages; any stage can roll back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanaryStage { Pending, OneNonValidator, ThreeValidators, FullRollout, RolledBack }

/// 2/3 validator supermajority approval (the T4 OperatorGate). Strict BFT 2/3:
/// `approvals * 3 >= total * 2`, and `total` must be > 0 (fail-closed on no validators).
pub fn validator_approval_passes(approvals: u32, total: u32) -> bool {
    total > 0 && approvals.saturating_mul(3) >= total.saturating_mul(2)
}

/// The full T4 (consensus-critical: VDF / emission / block-validation) gate. A DiffUnit lands
/// ONLY if: the ConsensusGate ran AND passed (chronos shadow-verify), it ships height-gated
/// (compiled-but-disabled behind an activation flag), the canary reached FullRollout, and 2/3
/// validators approved. Fail-closed: any missing => FAIL. The operator's typed confirm + the
/// staging-branch-only rule (never auto-main) sit on top of this.
pub fn t4_gate_passes(
    verify: &VerifyOutcome,
    consensus: Option<&ChronosVerdict>,
    height_gated: bool,
    canary: CanaryStage,
    approvals: u32,
    total_validators: u32,
) -> bool {
    gate_chain_passes(verify, consensus)
        && consensus.is_some()
        && height_gated
        && canary == CanaryStage::FullRollout
        && validator_approval_passes(approvals, total_validators)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn height_gate_activates_at_the_right_block() {
        let g = HeightGate { upgrade: "vdf_v2".into(), activation_height: 18_500_000 };
        assert!(!g.is_active(18_444_786));            // before -> dark
        assert!(g.is_active(18_500_000));             // at -> active
        assert_eq!(g.blocks_until_active(18_444_786), 55_214);
    }

    #[test]
    fn validator_approval_is_two_thirds() {
        assert!(validator_approval_passes(2, 3));     // 2/3 -> pass
        assert!(!validator_approval_passes(1, 3));    // 1/3 -> fail
        assert!(validator_approval_passes(4, 6));     // 4/6 -> pass
        assert!(!validator_approval_passes(3, 6));    // 3/6 (half) -> fail
        assert!(!validator_approval_passes(0, 0));    // no validators -> fail-closed
    }

    #[test]
    fn t4_gate_requires_the_full_safety_stack() {
        let green = VerifyOutcome::Green { build_ms: 10, tests_passed: 5 };
        let pass = chronos_verdict(99.6, true, 99.0);
        assert!(t4_gate_passes(&green, Some(&pass), true, CanaryStage::FullRollout, 2, 3));
        assert!(!t4_gate_passes(&green, None, true, CanaryStage::FullRollout, 2, 3));            // no ConsensusGate
        assert!(!t4_gate_passes(&green, Some(&pass), false, CanaryStage::FullRollout, 2, 3));    // not height-gated
        assert!(!t4_gate_passes(&green, Some(&pass), true, CanaryStage::ThreeValidators, 2, 3)); // canary incomplete
        assert!(!t4_gate_passes(&green, Some(&pass), true, CanaryStage::FullRollout, 1, 3));     // below 2/3
    }



    #[test]
    fn chronos_gate_passes_on_real_deterministic_run() {
        // the proven flux-chronos run: 16 nodes, 20% loss, redundancy 3 -> 99.6%, converged
        assert!(chronos_verdict(99.6, true, 99.0).passed);
    }

    #[test]
    fn chronos_gate_fails_closed() {
        assert!(!chronos_verdict(99.6, false, 99.0).passed); // diverged under permutation -> fail
        assert!(!chronos_verdict(95.0, true, 99.0).passed);  // delivery below the floor -> fail
    }

    #[test]
    fn gate_chain_requires_every_gate() {
        let green = VerifyOutcome::Green { build_ms: 10, tests_passed: 5 };
        let pass = chronos_verdict(99.6, true, 99.0);
        let fail = chronos_verdict(90.0, true, 99.0);
        assert!(gate_chain_passes(&green, None));                          // T1/T2: compile+test only
        assert!(gate_chain_passes(&green, Some(&pass)));                   // T3+: + consensus passes
        assert!(!gate_chain_passes(&green, Some(&fail)));                  // consensus fails -> chain fails
        assert!(!gate_chain_passes(&VerifyOutcome::Timeout, Some(&pass))); // compile/test fails -> chain fails
    }



    fn tmp_sandbox(name: &str) -> CrateSandbox {
        let wd = std::env::temp_dir().join(format!("flux_legacy_verify_test_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&wd);
        std::fs::create_dir_all(&wd).unwrap();
        CrateSandbox { repo_root: wd.clone(), work_dir: wd, is_worktree: false, branch: String::new() }
    }
    fn patch() -> CratePatch {
        CratePatch { crate_name: "q-demo".into(), edits: vec![("src/added.rs".into(), "pub fn z() {}\n".into())] }
    }

    #[test]
    fn cmd_templating_substitutes_crate() {
        assert_eq!(cmd_for("cargo build -p {crate}", "q-storage"), "cargo build -p q-storage");
    }

    #[test]
    fn apply_patch_writes_into_sandbox() {
        let sb = tmp_sandbox("apply");
        apply_patch(&sb.work_dir, &patch()).unwrap();
        let written = std::fs::read_to_string(sb.work_dir.join("src/added.rs")).unwrap();
        assert!(written.contains("pub fn z()"));
        std::fs::remove_dir_all(&sb.work_dir).ok();
    }

    #[test]
    fn apply_patch_rejects_sandbox_escape() {
        let sb = tmp_sandbox("escape");
        let evil = CratePatch { crate_name: "x".into(), edits: vec![("../../etc/owned".into(), "x".into())] };
        assert!(apply_patch(&sb.work_dir, &evil).is_err(), "must reject .. escape");
        let abs = CratePatch { crate_name: "x".into(), edits: vec![("/tmp/owned".into(), "x".into())] };
        assert!(apply_patch(&sb.work_dir, &abs).is_err(), "must reject absolute path");
        std::fs::remove_dir_all(&sb.work_dir).ok();
    }

    #[test]
    fn parse_test_counts_sums_binaries() {
        let log = "test result: ok. 12 passed; 0 failed; 1 ignored\n\
                   test result: FAILED. 3 passed; 2 failed; 0 ignored\n";
        assert_eq!(parse_test_counts(log), (15, 2));
    }

    #[test]
    fn verify_green_on_passing_build() {
        let sb = tmp_sandbox("green");
        let cfg = VerifyConfig { run_tests: false, timeout_s: 30, build_cmd: "true".into(), test_cmd: None };
        assert!(verify_in_crate(&sb, &patch(), &cfg).green());
        std::fs::remove_dir_all(&sb.work_dir).ok();
    }

    #[test]
    fn verify_build_failed_rejects() {
        let sb = tmp_sandbox("buildfail");
        let cfg = VerifyConfig { run_tests: false, timeout_s: 30, build_cmd: "false".into(), test_cmd: None };
        let o = verify_in_crate(&sb, &patch(), &cfg);
        assert!(matches!(o, VerifyOutcome::BuildFailed { .. }) && !o.green());
        std::fs::remove_dir_all(&sb.work_dir).ok();
    }

    #[test]
    fn verify_runs_tests_and_counts() {
        let sb = tmp_sandbox("tests");
        let cfg = VerifyConfig {
            run_tests: true, timeout_s: 30, build_cmd: "true".into(),
            test_cmd: Some("echo 'test result: ok. 3 passed; 0 failed'".into()),
        };
        match verify_in_crate(&sb, &patch(), &cfg) {
            VerifyOutcome::Green { tests_passed, .. } => assert_eq!(tests_passed, 3),
            o => panic!("expected Green, got {o:?}"),
        }
        std::fs::remove_dir_all(&sb.work_dir).ok();
    }

    #[test]
    fn verify_tests_failed_rejects() {
        let sb = tmp_sandbox("testfail");
        let cfg = VerifyConfig {
            run_tests: true, timeout_s: 30, build_cmd: "true".into(),
            test_cmd: Some("echo 'test result: FAILED. 1 passed; 2 failed'; false".into()),
        };
        match verify_in_crate(&sb, &patch(), &cfg) {
            VerifyOutcome::TestsFailed { passed, failed, .. } => { assert_eq!(passed, 1); assert_eq!(failed, 2); }
            o => panic!("expected TestsFailed, got {o:?}"),
        }
        std::fs::remove_dir_all(&sb.work_dir).ok();
    }
}
