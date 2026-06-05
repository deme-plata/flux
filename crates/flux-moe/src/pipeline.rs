//! pipeline.rs — the **Verified Build Pipeline**: the whole agentic-money build loop in one call.
//!
//! ```text
//!   propose (qwen3.6, local)  →  COMPILE-GATE (rustc)  →  cloud-judge (deepseek-v4-flash)  →  ship│reject
//!        cheap draft              free, fail-fast            paid 2nd opinion                 2-of-2
//! ```
//!
//! The money insight that makes this the right shape: **the free compile gate runs BEFORE the paid
//! cloud judge.** Code that doesn't even build is rejected for $0 — we never spend a DeepSeek call
//! refuting a snippet `rustc` already refuted. Only compiling code reaches the judge, and only code
//! the judge ALSO approves ships (2-of-2: machine + model). Every judge call is priced via
//! [`crate::serve::cost_usd`] and capped by a USD budget.
//!
//! [`BuildOps`] abstracts the three side-effecting steps (propose / compile / judge) so the
//! orchestration in [`run`] is unit-tested deterministically with [`MockOps`]; [`LiveOps`] wires the
//! real qwen3.6-local + rustc + DeepSeek-cloud path.

use crate::builder::extract_rust;

/// Where a run ended. Each later stage implies the earlier ones passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// The proposer returned no usable code.
    NoProposal,
    /// Code was produced but failed the compile gate — rejected for $0 (no judge spend).
    CompileFailed,
    /// Compiled, but the estimated judge cost exceeded the budget — not judged.
    OverBudget,
    /// Compiled + judged, but the judge said REJECT.
    JudgeRejected,
    /// Compiled AND judge-approved — 2-of-2 passed, it ships.
    Shipped,
}

impl Stage {
    pub fn shipped(self) -> bool { self == Stage::Shipped }
    pub fn label(self) -> &'static str {
        match self {
            Stage::NoProposal => "no proposal",
            Stage::CompileFailed => "compile failed (rejected, $0)",
            Stage::OverBudget => "over budget (not judged)",
            Stage::JudgeRejected => "judge rejected",
            Stage::Shipped => "SHIPPED (compile + judge, 2-of-2)",
        }
    }
}

/// The full record of one verified build.
#[derive(Debug, Clone)]
pub struct RunReport {
    pub task: String,
    pub code: String,
    pub compiled: bool,
    pub compile_log: String,
    pub judged: bool,
    pub approved: bool,
    pub verdict: String,
    pub stage: Stage,
    /// USD actually spent on the judge (0.0 if the compile gate or budget stopped us first).
    pub spent_usd: f64,
    pub budget_usd: f64,
}

impl RunReport {
    pub fn shipped(&self) -> bool { self.stage.shipped() }
}

/// The three side-effecting steps the pipeline orchestrates. Abstracted so [`run`] is pure logic
/// (testable with [`MockOps`]); the real impl is [`LiveOps`].
pub trait BuildOps {
    /// Ask the local proposer (qwen3.6) for a change; returns the raw reply (code is extracted).
    fn propose(&self, task: &str) -> Result<String, String>;
    /// Compile-gate the extracted code: `(ok, log)`. `ok==false` ⇒ rejected before any judge spend.
    fn compile(&self, code: &str) -> (bool, String);
    /// Estimated USD cost of judging this code (so we can refuse before paying).
    fn judge_cost_estimate(&self, task: &str, code: &str) -> f64;
    /// The paid cloud judge: `(approved, verdict_text, actual_cost_usd)`.
    fn judge(&self, task: &str, code: &str) -> Result<(bool, String, f64), String>;
}

/// Run the verified build pipeline for `task` under `budget_usd`. Pure orchestration over `ops`.
pub fn run<O: BuildOps>(ops: &O, task: &str, budget_usd: f64) -> Result<RunReport, String> {
    let raw = ops.propose(task)?;
    let code = extract_rust(&raw);
    let mut rep = RunReport {
        task: task.to_string(),
        code: code.clone(),
        compiled: false,
        compile_log: String::new(),
        judged: false,
        approved: false,
        verdict: String::new(),
        stage: Stage::NoProposal,
        spent_usd: 0.0,
        budget_usd,
    };
    if code.trim().is_empty() {
        return Ok(rep); // NoProposal
    }

    // ── free compile gate (fail-fast, $0) ──
    let (ok, log) = ops.compile(&code);
    rep.compiled = ok;
    rep.compile_log = log;
    if !ok {
        rep.stage = Stage::CompileFailed;
        return Ok(rep); // never pay the judge for non-compiling code
    }

    // ── budget guard before the paid call ──
    if ops.judge_cost_estimate(task, &code) > budget_usd {
        rep.stage = Stage::OverBudget;
        return Ok(rep);
    }

    // ── paid cloud judge ──
    let (approved, verdict, cost) = ops.judge(task, &code)?;
    rep.judged = true;
    rep.approved = approved;
    rep.verdict = verdict;
    rep.spent_usd = cost;
    rep.stage = if approved { Stage::Shipped } else { Stage::JudgeRejected };
    Ok(rep)
}

// ── live wiring: qwen3.6 (local ollama) + rustc + DeepSeek cloud ────────────────────────────────

/// Production [`BuildOps`]: local proposer via [`crate::generate`], `rustc` compile gate, DeepSeek
/// cloud judge via [`crate::cloud`].
pub struct LiveOps {
    /// Local ollama endpoint for the proposer (e.g. `http://127.0.0.1:11434`).
    pub local_endpoint: String,
    /// Proposer model (e.g. `qwen3.6:latest`).
    pub proposer: String,
    /// Cloud judge model (e.g. `deepseek-v4-flash`).
    pub judge_model: String,
}

impl LiveOps {
    /// Engineer prompt that asks for ONE compile-ready function/item in a single ```rust block.
    fn propose_prompt(task: &str) -> String {
        format!(
            "You are a senior Rust engineer. Implement EXACTLY this as idiomatic, std-only Rust \
             (no external crates). Task: {task}\n\nOutput ONLY the code in a single ```rust block, \
             no prose."
        )
    }
}

impl BuildOps for LiveOps {
    fn propose(&self, task: &str) -> Result<String, String> {
        crate::generate(&self.local_endpoint, &self.proposer, &Self::propose_prompt(task))
            .map_err(|e| format!("proposer {}: {e}", self.proposer))
    }

    fn compile(&self, code: &str) -> (bool, String) {
        compile_rust_lib(code)
    }

    fn judge_cost_estimate(&self, task: &str, code: &str) -> f64 {
        // a flash judge call ~= (task+code)/4 input tokens + a short verdict; estimate generously.
        let approx_in = ((task.len() + code.len()) / 4 + 200) as u64;
        let usage = crate::serve::Usage {
            prompt_tokens: approx_in,
            completion_tokens: 120,
            ..Default::default()
        };
        crate::serve::cost_usd(&usage, &crate::serve::Price::DEEPSEEK_V4_FLASH)
    }

    fn judge(&self, task: &str, code: &str) -> Result<(bool, String, f64), String> {
        let system = "You are a strict, impartial Rust reviewer. Reply with EXACTLY 'APPROVE' or \
                      'REJECT' on the first line, then ONE sentence why.";
        let user = format!(
            "Task:\n{task}\n\nProposed Rust (already compiles):\n{code}\n\nIs it correct, complete, \
             and idiomatic for the task?"
        );
        let r = crate::cloud::deepseek_complete(&self.judge_model, system, &user)?;
        Ok((crate::cloud::parse_approval(&r.content), r.content, r.cost_usd))
    }
}

/// Compile `code` as a standalone library with `rustc` (metadata only, no binary). Returns
/// `(ok, log)`; `log` is rustc's stderr (truncated). This is the free gate that fails fast.
pub fn compile_rust_lib(code: &str) -> (bool, String) {
    use std::io::Write;
    let dir = std::env::temp_dir();
    let src = dir.join(format!("flux_moe_gate_{}.rs", std::process::id()));
    if let Ok(mut f) = std::fs::File::create(&src) {
        let _ = f.write_all(code.as_bytes());
    } else {
        return (false, "could not write temp source".into());
    }
    let out = std::process::Command::new("rustc")
        .args(["--crate-type=lib", "--edition=2021", "--emit=metadata", "-A", "warnings", "-o"])
        .arg(dir.join(format!("flux_moe_gate_{}.rmeta", std::process::id())))
        .arg(&src)
        .output();
    let _ = std::fs::remove_file(&src);
    match out {
        Ok(o) => {
            let log = String::from_utf8_lossy(&o.stderr);
            (o.status.success(), log.chars().take(1200).collect())
        }
        Err(e) => (false, format!("rustc not runnable: {e}")),
    }
}

// ── auto-integration: the pipeline lands its own code, safely ───────────────────────────────────

/// How shipped code is written into the target file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LandMode {
    /// Create a new file; refuse if it already exists.
    New,
    /// Replace the file's whole contents.
    Overwrite,
    /// Append after the existing contents (e.g. add a function to an existing module).
    Append,
}

/// The result of trying to land shipped code.
#[derive(Debug, Clone)]
pub struct Landing {
    pub path: String,
    /// We wrote the file at all (always true unless a precondition refused).
    pub wrote: bool,
    /// The post-write verification passed → the change is KEPT.
    pub verified: bool,
    /// Verification failed → we restored the prior state (tree left untouched).
    pub rolled_back: bool,
    pub log: String,
}

impl Landing {
    /// Landed AND kept (the only state in which the repo changed for good).
    pub fn landed(&self) -> bool { self.verified && !self.rolled_back }
}

/// Land a **shipped** report's code into `path`, then run `verify`. If verify fails, restore the
/// previous state — delete the file if we created it, or rewrite the old contents — so a bad land
/// NEVER leaves the tree broken. Refuses outright to land anything that didn't pass the 2-of-2 gate.
///
/// `verify` is injected (the bin wires the real whole-crate build, e.g. `fluxc build --package …`,
/// or `FLUX_MOE_VERIFY_CMD`); tests pass a deterministic closure.
pub fn integrate<F: Fn() -> (bool, String)>(
    report: &RunReport,
    path: &str,
    mode: LandMode,
    verify: F,
) -> Result<Landing, String> {
    if report.stage != Stage::Shipped {
        return Err(format!("refusing to land: stage is '{}' — only a 2-of-2 SHIP lands", report.stage.label()));
    }
    let exists = std::path::Path::new(path).exists();
    if mode == LandMode::New && exists {
        return Err(format!("refusing to land: {path} already exists (use overwrite/append)"));
    }
    let backup = if exists { Some(std::fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?) } else { None };

    let content = match mode {
        LandMode::Append => {
            let prev = backup.clone().unwrap_or_default();
            format!("{}\n\n{}\n", prev.trim_end(), report.code.trim())
        }
        LandMode::New | LandMode::Overwrite => format!("{}\n", report.code.trim()),
    };
    std::fs::write(path, &content).map_err(|e| format!("write {path}: {e}"))?;

    let (ok, log) = verify();
    if ok {
        return Ok(Landing { path: path.into(), wrote: true, verified: true, rolled_back: false, log });
    }
    // verify failed → roll back to the exact prior state
    match &backup {
        Some(orig) => { let _ = std::fs::write(path, orig); }
        None => { let _ = std::fs::remove_file(path); }
    }
    Ok(Landing { path: path.into(), wrote: true, verified: false, rolled_back: true, log })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scriptable ops for deterministic orchestration tests.
    struct MockOps {
        reply: String,
        compiles: bool,
        cost_est: f64,
        judge_approves: bool,
        judge_cost: f64,
    }
    impl BuildOps for MockOps {
        fn propose(&self, _t: &str) -> Result<String, String> { Ok(self.reply.clone()) }
        fn compile(&self, _c: &str) -> (bool, String) { (self.compiles, "log".into()) }
        fn judge_cost_estimate(&self, _t: &str, _c: &str) -> f64 { self.cost_est }
        fn judge(&self, _t: &str, _c: &str) -> Result<(bool, String, f64), String> {
            Ok((self.judge_approves, if self.judge_approves { "APPROVE\nok".into() } else { "REJECT\nno".into() }, self.judge_cost))
        }
    }

    fn ops(compiles: bool, approves: bool, cost_est: f64) -> MockOps {
        MockOps { reply: "```rust\nfn x() {}\n```".into(), compiles, cost_est, judge_approves: approves, judge_cost: 0.0004 }
    }

    #[test]
    fn ships_when_compiles_and_approved() {
        let r = run(&ops(true, true, 0.0004), "t", 1.0).unwrap();
        assert_eq!(r.stage, Stage::Shipped);
        assert!(r.shipped() && r.compiled && r.approved && r.judged);
        assert!(r.spent_usd > 0.0, "a real judge call was paid for");
    }

    #[test]
    fn compile_gate_rejects_for_free_no_judge_spend() {
        let r = run(&ops(false, true, 0.0004), "t", 1.0).unwrap();
        assert_eq!(r.stage, Stage::CompileFailed);
        assert!(!r.shipped() && !r.judged, "must NOT reach the paid judge");
        assert_eq!(r.spent_usd, 0.0, "non-compiling code costs $0");
    }

    #[test]
    fn budget_guard_blocks_before_paying() {
        // compiles, but the estimate (0.01) blows a 0.001 budget → never judged, $0 spent.
        let r = run(&ops(true, true, 0.01), "t", 0.001).unwrap();
        assert_eq!(r.stage, Stage::OverBudget);
        assert!(!r.judged && r.spent_usd == 0.0);
    }

    #[test]
    fn judge_rejection_does_not_ship() {
        let r = run(&ops(true, false, 0.0004), "t", 1.0).unwrap();
        assert_eq!(r.stage, Stage::JudgeRejected);
        assert!(r.compiled && r.judged && !r.approved && !r.shipped());
        assert!(r.spent_usd > 0.0, "the judge still ran (and was paid)");
    }

    #[test]
    fn no_code_in_reply_is_no_proposal() {
        let mut o = ops(true, true, 0.0004);
        o.reply = "I cannot help with that.".into();
        // extract_rust falls back to whole text; force empty by making reply blank instead:
        o.reply = "   ".into();
        let r = run(&o, "t", 1.0).unwrap();
        assert_eq!(r.stage, Stage::NoProposal);
        assert!(!r.compiled && !r.judged);
    }

    fn shipped_report(code: &str) -> RunReport {
        RunReport {
            task: "t".into(), code: code.into(), compiled: true, compile_log: String::new(),
            judged: true, approved: true, verdict: "APPROVE".into(), stage: Stage::Shipped,
            spent_usd: 0.0004, budget_usd: 1.0,
        }
    }
    fn tmp(name: &str) -> String {
        std::env::temp_dir().join(format!("flux_moe_land_{}_{}.rs", std::process::id(), name)).to_string_lossy().into()
    }

    #[test]
    fn lands_and_keeps_when_verify_passes() {
        let p = tmp("keep");
        let _ = std::fs::remove_file(&p);
        let l = integrate(&shipped_report("pub fn ok() {}"), &p, LandMode::New, || (true, "built".into())).unwrap();
        assert!(l.landed() && l.verified && !l.rolled_back);
        assert!(std::fs::read_to_string(&p).unwrap().contains("pub fn ok()"));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn refuses_to_land_unshipped() {
        let mut r = shipped_report("fn x() {}");
        r.stage = Stage::JudgeRejected;
        assert!(integrate(&r, &tmp("never"), LandMode::Overwrite, || (true, String::new())).is_err());
    }

    #[test]
    fn rolls_back_new_file_on_verify_fail() {
        let p = tmp("rbnew");
        let _ = std::fs::remove_file(&p);
        let l = integrate(&shipped_report("pub fn bad() {}"), &p, LandMode::New, || (false, "crate broke".into())).unwrap();
        assert!(l.rolled_back && !l.verified && !l.landed());
        assert!(!std::path::Path::new(&p).exists(), "a failed land must delete the file it created");
    }

    #[test]
    fn rollback_restores_prior_contents_on_append_fail() {
        let p = tmp("rbappend");
        std::fs::write(&p, "// original\n").unwrap();
        let l = integrate(&shipped_report("pub fn extra() {}"), &p, LandMode::Append, || (false, "broke".into())).unwrap();
        assert!(l.rolled_back);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "// original\n", "original content must be restored exactly");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn new_mode_refuses_existing_file() {
        let p = tmp("exists");
        std::fs::write(&p, "x").unwrap();
        assert!(integrate(&shipped_report("fn y() {}"), &p, LandMode::New, || (true, String::new())).is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "x", "must not touch an existing file in New mode");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn real_rustc_gate_passes_good_code_fails_bad() {
        // free local gate, no network — proves compile_rust_lib actually shells rustc.
        let (ok, _) = compile_rust_lib("pub fn add(a: i32, b: i32) -> i32 { a + b }");
        assert!(ok, "valid Rust must pass the gate");
        let (bad, log) = compile_rust_lib("pub fn nope( { this is not rust");
        assert!(!bad, "broken Rust must fail the gate");
        assert!(!log.is_empty(), "rustc errors are captured for the report");
    }
}
