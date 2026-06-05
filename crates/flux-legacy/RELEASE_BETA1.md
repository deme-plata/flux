# flux-legacy 0.1.0-beta.1 — release notes

**What it is:** point flux's refactor / verify / consensus-gate powers at a brownfield Rust workspace
it didn't build — e.g. the 100-crate, 754k-LOC Quillon Graph mainnet node — and modernize it
**safely**: analyze → diagnose → split → verify in a sandbox → land a verified refactor on a branch
synced back to the source of truth. Multi-agent (lanes claimed + settled over the swarm bus),
runtime-evidence-driven (live-log Pulse), and reasoned at whole-node scale (DeepSeek-v4 1M window).

## The one thing beta-1 delivers
A single command produces a **consultancy-grade node-modernization assessment** (`release::Beta1Assessment`):
health verdict · sickest crates (triage + live runtime pain) · a prioritized treatment plan where
**every treatment shows the exact safety gate-path it must pass before it can touch mainnet** · and a
beta-1 readiness statement.

## Capability matrix (honest status)
| rung | capability | status |
|---|---|---|
| P1 | analyze + plan + render | ✅ shipped |
| P2 | split + cycles | ✅ shipped |
| P3 | context (ground in real code) | ✅ shipped |
| P4 | verify (sandbox build+test) | ✅ shipped |
| P5 | precheck (Safe/Review/Unsafe) | ✅ shipped |
| P6 | shadow (state-root equivalence) + pipeline (branch+push) | ✅ shipped |
| P7 | consensus (height-gate + canary + 2/3 quorum) | ✅ shipped |
| P8 | drive (swarm orchestrator, fail-closed) | ✅ shipped |
| P9 | autopilot (autonomous loop, T4 deferred to human) | ✅ shipped |
| P10 | stability (live-node health audit) | ✅ shipped |
| Pulse | live-journald → per-crate runtime pain | ✅ shipped |
| 1M | corpus + bundle + ask (DeepSeek-1M whole-node) | ✅ shipped |
| P11 | triage + psych (hospital board + code-smell DSM) | 🔗 integration |
| P12 | consult (cross-crate 1M specialist consult) | ⬜ planned |

## The safety contract (non-negotiable)
- **Never auto-merges to mainline.** Every change lands on a `refactor/…` branch; a human merges.
- **Never the baseline.** `PROTECTED_BASELINE` guards `agent/cross-shard-simd-validation`.
- **Never sync-down / never prunes history.** Verified add-only.
- **Tiered gate-path** (escalates with risk):
  - T1–T2 (structural): precheck → sandbox build+test.
  - T3 (logic): + shadow (state-roots match over N real blocks).
  - T4 (consensus): + height-gate + canary + 2/3 validator quorum + **human activation**.
- **Fail-closed** at every gate. Dry-run by default; `--confirm` is the only thing that writes.

## Run it
```bash
flux-legacy report  /home/orobit/qnk         # health + ranked plan (the assessment inputs)
flux-legacy corpus  /home/orobit/qnk --window 1000000 --out /tmp/quillon-1m.txt   # 1M DeepSeek bundle
flux-legacy split   <god-file> --stage       # dry-run module-split preview
flux-legacy sync    <repo> <god-file> --confirm   # land a verified refactor on a branch (never baseline)
```

## Git topology (source of truth ↔ Epsilon)
Canonical history on **Beta** `/opt/orobit/shared/q-narwhalknight`; pushes flow through the **bare hub**
`q-narwhalknight-hub.git` (clean push semantics) + git daemon `:9418` for fast LAN read. Epsilon
syncs via a **blob-filtered** clone (the full history is 51 GB, bloated with committed binaries).

## Known / next
- Apply the **blob-filtered** Epsilon clone (a plain clone of the 51 GB history does not complete).
- P11 triage/psych integration into the assessment; P12 consult.
- Wrap `flux_legacy_*` as MCP tools so the swarm drives the whole pipeline from one call.

*Built by a multi-agent flux swarm; verified with `flux_combo`/`flux_test` (never raw cargo);
DeepSeek-v4 used for whole-node reasoning under the operator's gate.*
