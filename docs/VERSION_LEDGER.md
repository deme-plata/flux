# Flux Version Ledger — the one place the tracks reconcile

> Problem this solves: Flux has shipped under **three parallel numbering tracks** (platform drops,
> the compiler ladder, and a planned hardening release), plus separate SIGIL tags and an MCP server
> version string. The numbers do not reconcile into one line by themselves. This ledger records what
> each number IS, and fixes the rule going forward. Update this file in the same commit as any
> version bump or tag.

## The rule going forward (binding from v0.34.0)

1. **One version line**: the workspace `Cargo.toml` `version` is the single source of truth. A
   release = a commit that bumps it + an annotated tag `vX.Y.0` on that commit + a CHANGELOG entry.
   No tag without all three (v0.33.0 was tagged without bump or changelog — backfilled, not repeated).
2. **Next release is 0.35.0.** The planned hardening release keeps its documented target number
   **0.36.x** (`docs/RELEASE_0.36.1_PLAN.md`) and lands after 0.35 — its plan predates the ladder
   numbering and jumping to it directly would orphan 0.35.
3. **SIGIL tags** (`sigil-vX.Y.Z`) stay a separate prefixed track — they version the chain, not the
   compiler.
4. The MCP server version string reports the workspace version of the **binary actually running**;
   it lags the tree until `fluxc` is rebuilt. Do not read it as "latest release."

## Track A — Platform / product drops (RELEASES.md)

| Version | Date | What |
|---|---|---|
| v0.26.0 | 2026-06 | (last pre-drop platform tag) |
| v0.27.0 "Midnight Sun" | 2026-06-08 | Surprise drop: SyncProgress, SQIsign, Agora, Cortex. "Not on the roadmap. Already running." |
| v0.28.0 | 2026-06 | Platform state documented in FLUX-MASTER-OVERVIEW (rustc-wrapping self-hosting build platform) |

## Track B — The compiler ladder (fluxc releases, this tree's tags)

| Tag | Date | Theme |
|---|---|---|
| v0.29.0 | 2026-06-24 | Float + bitwise codegen |
| v0.30.0 | 2026-06-25 | Scalar completeness (all int widths/signs, unary, casts) |
| v0.31.0 | 2026-06-25 | Flat aggregates: tuples + structs, aggregate-return ABI |
| v0.32.0 | 2026-06-25 | Real call signatures; aggregate-returning calls |
| v0.33.0 | 2026-06-25 | Cache `-L` key fix (tag only; changelog + bump backfilled in 0.34.0) |
| v0.34.0 | 2026-07-01 | Ladder rungs 4–6: data enums, generics/monomorphization, named consts + multi-aggregate param fixes |

## Track C — Planned / future

| Number | Status | What |
|---|---|---|
| 0.35.0 | next | Whatever ships next under rule 1 (candidates: FIP-0001 0.34-groundwork items — frozen IR spec, `RUSTC_VERSION` MIR-diff CI, `Frontend` trait; FIP-0002 Phase 2 — stable `--extern` identity → full-closure cache hits → restore-by-default) |
| 0.36.x | planned (2026-06-10 plan, held) | Security-hardened Flux (SEC-001..022), flux-db ≤2s cold-open, SIGIL sync hardening, musl-static deploy — `docs/RELEASE_0.36.1_PLAN.md` |
| Phase H | horizon | SIGIL DEX swap engine, yield-farming agent, sigil-vm (RELEASES.md) |

## Strategic frame (what governs the roadmap)

- **FIP-0001** (ACCEPTED 2026-06-26): fluxc = MIR-consuming Cranelift codegen + toolchain on a
  version-pinned `rustc --emit=mir` (1.93.1) contracted frontend. Native frontend (Option A) is the
  north star, gated on the self-host trigger; until then frontend work is deferred.
- **FIP-0002**: cache strategy. Phase 1 done (build-safe, populate-only default). Phase 2 = stable
  `--extern` identity → full-closure hits → re-enable restore by default.

## Known reconciliation debts (deliberate, tracked)

- `Cargo.lock` was not committed with v0.34.0 (it carries deps from in-flight flux-context WIP owned
  by another lane); it syncs with that owner's commit.
- The Track A ↔ Track B relationship pre-0.29 is narrative, not linear — the ladder took over the
  number line at 0.29. Do not try to back-interpolate.
- A working-tree cache change (restore-on-by-default flip in `fluxc-core/src/lib.rs` + flux-driver
  name-bridging) exists UNCOMMITTED and unowned as of 2026-07-01 — it is FIP-0002 Phase-2 material
  and must be claimed, verified against the ICE history, and committed by its owner before any tag
  includes it.
