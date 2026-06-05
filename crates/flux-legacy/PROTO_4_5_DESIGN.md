# flux-legacy — Prototype 4 + 5 design (locked spec for the LEGACY swarm)

Context: the real target is the **Quillon Graph node — 99 crates, 754,743 LOC, 217 god-files**
(`/tmp/quillon-legacy-report.txt`). #1 god-file = `q-api-server/main.rs` @ 27,054 LOC.

Pipeline so far:
```
P1 analyze → plan → render      (LegacyReport, RefactorTask)         [shipped]
P2 execute → split actuator     (RefactorBrief; god-file→modules)    [shipped]
P3 context                      (ground a brief in REAL code)        [shipped]
```
**The honest gap P3 surfaced:** flux-moe's pipeline gate is a *standalone* `rustc --crate-type=lib`.
A grounded refactor references the crate's own types/deps → it can't compile in isolation →
`gateable_standalone()==false` for everything real. So the gate is blind to actual refactors.

P4 + P5 fix that and turn the toolchain into an autonomous, budgeted, human-gated refactor product.

---

## P4 — IN-CRATE VERIFY (`verify.rs`) · LEGACY-VERIFY

The REAL gate: apply the proposed change to an **isolated copy of the legacy repo** and run that
crate's OWN build (+ tests). Never mutate the real tree; roll back by discarding the sandbox.

```rust
/// An edit set the proposer produced for one crate (path → new file content).
pub struct CratePatch { pub crate_name: String, pub edits: Vec<(String, String)> } // rel path → content

/// Isolated checkout where a patch is applied + verified without touching the real workspace.
/// Implementation: `git worktree add` a scratch branch of the legacy repo (cheap, COW), OR a
/// recursive copy when the repo isn't git. The real tree is read-only here.
pub struct CrateSandbox { pub repo_root: PathBuf, pub work_dir: PathBuf, pub branch: String }

pub enum VerifyOutcome {
    Green { build_ms: u128, tests_passed: u32 },     // the only outcome that may land
    BuildFailed { log: String },                     // compile broke → reject
    TestsFailed { log: String, passed: u32, failed: u32 },
    Timeout,
}

pub struct VerifyConfig {
    pub run_tests: bool,            // default true — compile alone isn't enough
    pub timeout_s: u64,             // q-api-server is huge → generous
    pub build_cmd: String,          // e.g. "cargo build -p {crate}" or a fluxc invocation
    pub test_cmd: Option<String>,   // e.g. "cargo test -p {crate}"
}

pub fn sandbox_open(repo_root: &Path) -> Result<CrateSandbox, String>;   // git worktree / copy
pub fn verify_in_crate(sb: &CrateSandbox, patch: &CratePatch, cfg: &VerifyConfig) -> VerifyOutcome;
pub fn sandbox_close(sb: CrateSandbox);                                  // discard worktree/branch
```

- **Safety:** all writes land in `work_dir` (scratch worktree). The real `/home/orobit/q-narwhalknight`
  is never mutated. Rollback = `git worktree remove` / `rm -rf` the copy. Fail-closed: anything but
  `Green` rejects.
- **Measurement gate:** `Green { tests_passed }` AND `tests_passed >= baseline` (baseline captured
  from the unpatched crate first). A refactor that drops tests is a regression, not a win.
- **Composition:** replaces flux-moe's standalone gate for grounded refactors. flux-moe
  `pipeline::integrate(..., verify=|| verify_in_crate(...))` already takes an injected verifier —
  P4 IS that closure for the legacy case.
- **Still pretend until built:** a real `cargo build -p q-api-server` is a multi-minute compile;
  the campaign (P5) must budget wall-clock, not just $.

---

## P5 — AUTONOMOUS REFACTOR CAMPAIGN (`campaign.rs`) · LEGACY-CAMPAIGN

Point flux at the 99-crate node and let it grind god-files down — 2-of-2 gated, budget-capped,
branch-isolated, human-merged. The end-to-end product.

```rust
pub struct CampaignConfig {
    pub workspace_root: PathBuf,
    pub top_n: usize,               // how many ranked tasks to attempt
    pub usd_budget_total: f64,      // HARD cap across all deepseek judge calls (cost_usd)
    pub minutes_budget: u64,        // HARD wall-clock cap (real builds are slow)
    pub kinds: Vec<String>,         // ["add-tests","split-god-file",...] — opt-in per kind
    pub land: LandPolicy,           // BranchDiff (default) | DryRun  — NEVER auto-merge to main
}

pub enum LandPolicy { DryRun, BranchDiff } // BranchDiff: commit to scratch branch + emit a .patch

pub struct CampaignItem {
    pub task: RefactorTask,
    pub stage: String,              // proposed|verified|judged|landed|rejected|over-budget
    pub spent_usd: f64,
    pub verify: Option<String>,     // VerifyOutcome summary
    pub diff_path: Option<String>,  // PR-ready .patch for the human
    pub note: String,
}
pub struct CampaignResult {
    pub attempted: usize, pub shipped: usize, pub rejected: usize,
    pub spent_usd: f64, pub minutes: u64, pub loc_moved: usize,
    pub items: Vec<CampaignItem>,
}

/// analyze → plan → for each ranked task within budget:
///   context::inject(real code) → flux-moe propose → P4 verify_in_crate → deepseek judge
///   → (Green & APPROVE) write diff to a scratch branch. Stop at $ or minute budget.
pub fn run_campaign(cfg: &CampaignConfig) -> CampaignResult;
```

- **2-of-2:** machine (P4 in-crate build+tests Green) AND model (deepseek judge APPROVE). Either
  fails → rejected, recorded, $0 wasted past the judge.
- **Budget discipline:** cumulative `cost_usd` stops at `usd_budget_total`; wall-clock stops at
  `minutes_budget` (the real lesson — builds, not tokens, are the cost on a 754k-LOC node).
- **Human hand:** `BranchDiff` emits PR-ready `.patch` files on a scratch branch. The campaign NEVER
  merges to the legacy main — owned by everyone, ruled by no one; a human merges.
- **Output:** a `CampaignResult` → render lane draws the Viktor-visual table (shipped/rejected/$/LOC).

---

## Lane split (claim against this spec)
| Lane | File | Owner |
|---|---|---|
| LEGACY-VERIFY (P4) | `src/verify.rs` | open |
| LEGACY-CAMPAIGN (P5) | `src/campaign.rs` | open (depends on P4 + execute + context) |
| render P4/P5 tables | extend `render.rs` | render owner (rocky-vision-infra) |

Honest checklist (both): measurement gate = Green build+tests ≥ baseline; pretend-until-built =
real cargo builds are minutes each; rollback = scratch worktree/branch discard; composition edge =
P4 is flux-moe `integrate`'s injected verifier, P5 wraps analyze→plan→execute→context→pipeline.
