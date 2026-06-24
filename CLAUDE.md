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
- **Current:** default build (`fluxc self`) is **RED** (jq-contaminated cargo probe); wrapper cache **0% hit**
  (`docs/FLUXC_CACHE_GAP.md`). Verify live before asserting.

See `AGENTS.md` for the phase model, the full mis-reads list, and `FLUX_FACTS.json` for machine-readable facts.
