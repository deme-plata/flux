# CLAUDE.md

**Read [`AGENTS.md`](./AGENTS.md) first — it is the canonical orientation for this repo.**

Quick pointers (full detail in `AGENTS.md`):

- **`fluxc` is a hybrid compiler (~⅓ of standalone):** owns its IR + a real Cranelift→ELF backend
  (`crates/flux-frontend`, `crates/flux-backend`, `crates/fluxc-core/src/phase3.rs`); **borrows rustc for all
  parse-with-semantics** (`rustc --emit=mir`) and **`cc`** for linking; does **not** self-host. It is *not*
  "just a rustc wrapper" and *not* a finished self-hosting compiler.
- **Trust live introspection over static strings** (badges/CHANGELOG/`mcp.rs:571` are stale):
  `fluxc version`, `fluxc status --json`, `fluxc xray --json`, `flux_health_report`.
- **Never run raw `cargo`** here — use `fluxc` / `flux_*` MCP tools. Money moves are **propose-only**.
- `fluxc --version → rustc …` is **wrapper passthrough**, not "no compiler." `fcx` ≠ the compiler.
  `sigil-top` version ≠ Flux's version (Flux = `0.28.0`).
- **Current (v0.36.0, 2026-07-03):** `fluxc self` is **GREEN, ~12.4s warm** (see the build-time invariant below);
  cache restore **ON by default** at `$HOME/.flux/cache` (survives `rm -rf target`). The old "RED / 0% hit"
  status is fixed. Verify live before asserting.

See `AGENTS.md` for the phase model, the full mis-reads list, and `FLUX_FACTS.json` for machine-readable facts.

## ⛔ Performance work — MEASURE THE LIVE PATH FIRST (5-day-stall lesson, 2026-06-27)
See sigil SKILL.md RULE 0. TL;DR: measure the real *deployed* end-to-end number BEFORE optimizing; one closed loop (measure -> fix -> deploy -> re-measure); no DARK / fall-back "wins"; ONE owner of the live number, not a swarm of green microbenches. The SIGIL sync cap is the producer's `<=1 full-block serve per 120ms` throttle (sigil-node/src/main.rs) + the serial/stub fetch (block_sync/fetch.rs) — NOT the local pipeline (which benches at millions/s and was never the bottleneck).

## Build-time invariant — hold `fluxc self` at/under 12.4s warm (2026-07-02, keep it here)
`fluxc self` (whole workspace, wrapper+cache) is **12.4s warm steady-state** (0.8s CPU — a pure
fingerprint scan). This is the v0.20-era number, **lost for weeks and reclaimed** — treat regressions past
~15s as a bug, not a fact of life. What broke it and must never recur:
- **One wrapper identity.** cargo hashes `RUSTC_WRAPPER` into every unit fingerprint. EVERY cargo spawn in
  fluxc MUST go through `fluxc_core::apply_wrapper_env` (never a bare `Command::new("cargo")`); mismatched
  entry points flip the whole-workspace fingerprint = ~193s rebuild each alternation. Across agents/checkouts,
  export `FLUX_WRAPPER_PATH=$HOME/.flux/bin/fluxc` (kept symlinked) so the fleet shares ONE fingerprint universe.
- **Do not chase ghosts.** `FLUXCACHE APPLYFAIL ...` lines on a no-op build are cargo REPLAYING cached stderr,
  not live wrapper calls. A 60s "no-op" right after a `fluxc` rebuild or version bump is legit one-time work.
- **Restart long-running `fluxc mcp`/`serve` after any rebuild** — they hold the pre-rebuild binary.
- **Measure it, do not assume:** `time fluxc self` (twice; the second run is the steady-state number). See the
  flux-wrapper-fingerprint-regression memory for the full root cause.
