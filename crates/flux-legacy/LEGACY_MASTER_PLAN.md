# flux-legacy — MASTER PLAN (operator-controlled legacy transformation of Quillon Graph)

> Designed with deepseek-v4-flash (cloud API) from the foundation up, gate-reviewed by rocky against
> the Quillon mainnet runbook. **The operator (Viktor) holds total control: nothing lands without an
> explicit gate, and the control surface (§3) is the steering wheel.** Target: the real node —
> **100 crates · 754,743 LOC · 217 god-files**, worst = `q-api-server/main.rs` @ 27,054 LOC,
> canonical source on Beta `/opt/orobit/shared/q-narwhalknight` (v10.11.39, branch
> `agent/cross-shard-simd-validation`). It must never break mainnet.

## 0. The pivot — improve the FOUNDATION

P1–P4 (analyze→plan→execute→verify) are **task-centric**: they name and run one refactor at a time.
To "do everything possible to the code, safely," the foundation becomes **system-centric** — six
primitives everything else is built on. The work already shipped is not thrown away; it is *absorbed*
as capability providers on top of the new primitives.

| Primitive | What it is | Absorbs / replaces |
|---|---|---|
| **CodeGraph** | Versioned call-graph + module DAG + ownership/fan-in of every crate item. Built once, updated incrementally. | Deepens P1 `analyze` (LOC metrics → real graph); subsumes sibling `cycles` + `buildplan`. |
| **DiffUnit** | One atomic, operator-visible patch + metadata (risk tier, affected CodeGraph nodes, budget). Every change IS a DiffUnit. | Replaces vague `RefactorTask`/`RefactorBrief`; P4 `CratePatch` is the proto. |
| **Sandbox** | Isolated git-worktree with lifecycle (create→build→test→teardown); never mutates the real tree. | Formalizes P4 `CrateSandbox`. |
| **GateRunner** | Sequences a CHAIN of Gates a DiffUnit must pass: `CompileGate → TestGate → ConsensusGate → OperatorGate`. | Generalizes P4 `verify_in_crate` (single build+test) into a chain. |
| **Operator** | Human with veto / replay / dry-run; the ONLY source of a landing decision. | Makes the human an explicit decision point in every gate, not just the merger. |
| **Pulse** | Live mainnet health stream (height, sync lag, state size, error rate) read from the node's API. | NEW — without it we transform blind. Source: `GET /api/v1/status`. |

**Foundation refactor:** `analyze` → emits CodeGraph; new `fabricator` lib emits DiffUnits;
`verify` → `GateRunner`; `pulse` daemon; P1–P4 become providers (e.g. "split god-file" = a
CodeGraph query + a fabricator + a GateRunner run).

## 1. Capability menu — what flux can do to the code (by RISK TIER)

| Tier | Class | Examples | Gate requirement |
|---|---|---|---|
| **T1** | Cosmetic / read-only | rename locals, reorder fns, doc comments, dead-import/dead-code removal, `clippy --fix`, `fmt` | Compile + Test |
| **T2** | Structural-safe (no logic change) | split god-file → modules, extract helper crate, move code (no cycles), extract trait from impl | Compile + Test |
| **T3** | Logic, non-consensus | libp2p version-drift fix, RPC retry/timeout, storage key-encoding (backward-compat), logging/metrics | Compile + Test + **Shadow-verify** |
| **T4** | Consensus-critical / state-altering | VDF eval change, emission-curve change, block-validation reorder, new p2p sync protocol | + **Height-gate** + **Shadow** + **Canary** + **OperatorGate (typed confirm)** |
| **T5** | Irreversible / data-migration | DB serialization change, remove a public RPC | OperatorGate + snapshot + rollback plan. **Prune history = FORBIDDEN (never implement).** |

Every capability emits exactly one DiffUnit; the GateRunner enforces the tier's gate chain.

## 2. Mainnet safety model (the hard part)

- **Height-gating** — any consensus/validation change ships *compiled but disabled* behind a block-height
  activation flag (`is_upgrade_active(Upgrade, height)`); validators upgrade async before height `H`.
  *(This is the runbook's existing q-consensus-guard rule — deepseek reproduced it independently.)*
- **Shadow verification** — for T3+, run a second node with the patched code, feed live mainnet blocks
  WITHOUT producing state, compare state-root after N blocks; mismatch → gate fails. *(= the runbook's
  preflight/chronos discipline.)*
- **Canary** — T4: deploy to one non-validator → watch Pulse 48h → 3 validators (height-gated) → full
  rollout only on 2/3 operator approval.
- **Rollback** — keep the previous binary; `--rollback` restarts it (block replay is safe — we NEVER
  prune). DB-migration (T4) takes a RocksDB snapshot first.

## 3. ⭐ OPERATOR CONTROL SURFACE — Viktor's total control

**Global posture (set once, applies to everything):**
- `--max-tier <1..5>` — system refuses anything above this tier unless you explicitly override per-diff.
- `--approval-mode manual | auto-t1 | auto-t2 | off` — how much it may stage without you clicking.
- `--budget-usd <cap>` + `--minutes <cap>` — hard spend + wall-clock ceilings.
- `--scope <regex>` — restrict to crates/files/subgraph (e.g. only `q-api-server`).

**Per-action, before anything happens you SEE:** the unified diff, computed risk tier, affected
CodeGraph nodes, budget estimate, a Pulse snapshot (current height/sync/errors), and the sandbox
prediction.

**Gates you hold:** `--dry-run` (run all gates except your approval, show green/red, land nothing);
**2-of-2** (machine GateRunner + you) on everything; **typed confirm** ("I confirm this is safe for
mainnet") for T4+; **`--kill-switch`** (abort sandbox, discard pending diffs, revert last un-merged commit).

**The invariant:** diffs land only on a STAGING BRANCH; a human merges to mainline. Never auto-main.
You are the only source of a landing decision.

## 4. Phases (each with a falsifiable measurement gate)

| Phase | Goal | Capabilities | Exit gate (measured) |
|---|---|---|---|
| **P5** | Refound: CodeGraph + DiffUnit + Sandbox + GateRunner; absorb P1–P4 | T1 + T2 | old P1–P4 tests green under new primitives; 10 random T2 refactors on 10 god-files pass sandbox + your merge; dashboard shows live Pulse from testnet |
| **P6** | Logic changes + Shadow-verify + Pulse | T3 | fix a real libp2p version-drift on testnet, 1wk no stalls; shadow-verify catches a deliberately-injected logic bug |
| **P7** | Consensus-critical + height-gate + canary | T4 | testnet VDF update: shadow passes, canary 48h, height-gate activates at the right block, zero forks; rollback drill = zero data loss |
| **P8** | Full autonomy (still operator-gated) | auto-propose T1–T3 | god-file count −30% on testnet over 1 month, zero regressions; your approval rate of auto-diffs >90% |

**Built so far (absorbed into P5):** P1 analyze/plan/render, P2 execute/split, P3 context, P4 verify,
+ sibling lanes (corpus, buildplan, cycles, ai_refactor, precheck). flux-moe Verified Build Pipeline =
the propose→gate→judge→land engine the GateRunner drives.

---
**Rules that never bend:** never auto-merge to mainline · never an un-height-gated consensus change ·
never sync-down/prune history · fail-closed · every risky step operator-gated. Owned by everyone,
ruled by no one — and on mainnet, ruled by the operator's gate.
