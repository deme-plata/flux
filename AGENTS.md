# AGENTS.md — read this before analyzing Flux

> **You are looking at Flux, an AI-native Rust build platform + an emerging native compiler (`fluxc`).**
> Most agents (and humans) get Flux wrong on the first pass because they read `README.md` and the
> version badges first — **those are stale.** This file is the accurate, maintained orientation.
> When this file and any other doc/badge disagree, **this file and live introspection win.**

## ⚡ First move: trust live introspection, not static strings
Flux's static docs drift; its live introspection is honest (it reports its own *bad* numbers). Before
asserting anything about versions, scale, or health, run:

```bash
./target/debug/fluxc version          # real platform version (NOT `fluxc --version`)
./target/debug/fluxc status --json    # crates, workspace agility
./target/debug/fluxc xray --json      # health, deps, risks
# or the MCP combo:
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"flux_health_report","arguments":{}}}' | ./target/debug/fluxc mcp
```

> `fluxc --version` returns a **rustc** version string (e.g. `rustc 1.93.1`). That is **wrapper passthrough**,
> NOT proof that Flux has no compiler. Use `fluxc version` (subcommand) for the real `fluxc 0.28.0`.

## What `fluxc` is — in one paragraph (get this right)
`fluxc` is a **hybrid compiler: it owns a real backend but borrows a real frontend.** It **owns** its own IR
(`crates/flux-frontend`) and a genuine **Cranelift codegen backend** (`crates/flux-backend`) that emits
**native ELF objects**, plus a working `compile → cc-link → run` JIT with a BLAKE3 object cache and SQIsign
provenance. It **borrows** rustc for *all* parse-with-semantics (typecheck / borrow-check / trait resolution /
monomorphization — via `rustc --emit=mir`, whose MIR text it parses) and **`cc`** for linking, and it does
**not** self-host (fluxc is built by cargo+rustc). Net: **a "MIR-consuming Cranelift codegen + toolchain,"
roughly one-third of a standalone compiler** — *not* "just a rustc wrapper" (the inverse error) and *not* a
finished self-hosting compiler (the marketing). Memory-safety guarantees are presently **rustc's, not Flux's**.

## The build model — three distinct paths (NOT maturity levels)
| Phase | Command | What it is | Owns compilation? |
|---|---|---|---|
| **Phase 1** (default) | `fluxc self` | `cargo build` + `RUSTC_WRAPPER=self` (wrap + cache rustc) | No — orchestration only |
| **Phase 2** | `fluxc build --no-cargo` | drop cargo; `flux-graph` drives rustc directly | No — still rustc |
| **Phase 3 (3d)** | `fluxc compile` / `compile-native` / `run` / `heal` | **the actual Flux compiler** (`crates/fluxc-core/src/phase3.rs`) | **Yes — codegen + IR** (borrows rustc frontend, `cc` link) |

Compiler code lives in: `crates/flux-frontend` (IR + `syn` parser + rustc-MIR-text parser),
`crates/flux-backend` (Cranelift → ELF), `crates/fluxc-core/src/phase3.rs` (the pipeline).

## Build rules (do not violate)
- **Never run raw `cargo`** in this workspace — use `fluxc` / the `flux_*` MCP tools. (Raw cargo bypasses the wrapper/cache and the dogfood path.)
- **Honest numbers only:** never quote a score/speed/count you haven't run and read from live output.
- **All money moves are propose-only** (`flux_sigil_txn_send`, `flux_sigil_dex_swap`, `flux_bank_propose_transfer`) — a human signs (2-of-2). Never auto-execute.

## Common mis-reads (don't make these)
- `fluxc --version → rustc …` ⇒ **wrapper passthrough**, not "no compiler." (Use `fluxc version`.)
- **`fcx` is NOT the compiler** — `flux-fcx*` is a TSX-dialect → Slint UI transpiler (to beat Electron).
- **`sigil-top 1.0.0-rc9` is an app built BY flux**, not Flux's version. Flux is `0.28.0`.
- **Two different caches:** the Phase-3 BLAKE3 object cache (`temp/flux_jit`, works) vs the Phase-1
  `RUSTC_WRAPPER=self` build cache (currently 0% hit — see below). Don't conflate them.
- **README badges / `CHANGELOG` top / `crates/fluxc/src/mcp.rs:571` version strings are STALE.** Ignore them; use live introspection.
- `flux-rev` is a **real** ~1.2k-LOC content-addressed VCS, just under-adopted (not yet wired as `fluxc rev`).

## Current state caveats (verify live; true as of 2026-06-24)
- **Default build is GREEN ✅ (2026-06-24 14:17):** `fluxc self` exits 0 — full workspace self-build (~102s
  cold / ~15s incremental). The earlier "jq probe" failure was a **poisoned cargo target-info cache**, not a
  live filter: `target/.rustc_info.json` + `.target-shared/.rustc_info.json` had cached a *failed* rustc probe
  whose stderr contained a stray `jq` compile error, and cargo **replayed the cached failure without launching
  rustc**. Fixed by quarantining those caches (they regenerated clean) + restoring the real
  `RUSTC_WRAPPER`/`FLUXC_WRAPPING`/`REAL_RUSTC` env in `fluxc-core::self_build` (which previously only *printed*
  the claim). → If `fluxc self` ever fails at "target-specific information" again, grep `*/.rustc_info.json` for
  a cached `success:false` entry **before** suspecting rustc/cargo.
- **Still not green:** `flux-p2p` *tests* fail to compile (`E0282` in `tests/two_node_gossipsub.rs` + stale
  `libflux_p2p` rlib extern) — the next P0 item.
- **Wrapper cache 0% hit** (`docs/FLUXC_CACHE_GAP.md`): it populates but `apply_cached_outputs` restores a
  dep-info marker, not rmeta/rlib bytes, so cargo re-runs rustc. Patches 2+3 designed, not implemented. (The
  `self_build` fix makes the wrapper *engage*; cache **hits** still await Patches 2+3.)
- **`fluxc-core`** is the weakest crate (high coupling, ~18% test coverage, ~30% docs) — the standing #1 priority.
- Memory-safety / "self-hosting" claims in the whitepaper & RC notes are **aspirational**, not yet realized.

## Where to look first
- Compiler: `crates/flux-frontend`, `crates/flux-backend`, `crates/fluxc-core/src/phase3.rs`
- Build/cache truth: `docs/FLUXC_CACHE_GAP.md`, `docs/FLUX_WHITEPAPER.tex`
- Live facts: `fluxc xray --json`, `fluxc status --json`, `flux_health_report`, and `FLUX_FACTS.json` (this dir)
- Governance for any identity/version change: `docs/flux-standards-v0.md` (the FIP process)

*Maintainers: regenerate `FLUX_FACTS.json` from live introspection when it drifts; keep this file's "Current state caveats" honest. Honesty in the docs is the same asset as honesty in the tooling.*
