# Flux Foundation — Commit Log & Changelog

## v0.35.0 — The fast-build release: one wrapper identity, cache restore on (2026-07-02)

The 12-second era restored — and made structural. Warm `fluxc self` measured at **12.4s**
(0.8s CPU); warm alternation across build/check/test/combo 0.3–10s.

### Fixed
- **The build-speed regression** (v0.20 built warm in ~12s, lately 2+ minutes): cargo hashes
  `RUSTC_WRAPPER` into every unit fingerprint, and fluxc entry points disagreed — test/self/MCP
  spawned cargo WITH the wrapper, build/check/quick/run WITHOUT (gated on `--provenance`).
  Every alternation flipped the entire workspace fingerprint (~193s rebuild measured, each way)
  plus ~27 autocfg build-script re-runs. Fix: `apply_wrapper_env()` — one canonical cargo
  environment on every spawn. RULE: never add a cargo spawn without it.
- `fluxc test` package resolution honors `--package`.

### Changed
- **Cache restore is ON by default** (`FLUX_CACHE_RESTORE=0` opts out), guard-hardened after a
  24-agent adversarial review: the closure-consistency sidecar is now REQUIRED for dep-carrying
  units (missing sidecar = miss), and the proposed name-bridging was REJECTED (it laundered
  wrong-metadata-hash blobs into expected names — the historical ICE class) — a name mismatch
  stays a miss. Invariant preserved: an unverifiable entry can never break a build.
- Self-build/build cache counters read the file-based per-unit events (wrapper subprocesses
  never updated the parent's atomics — the permanently-dead 0/N readout).

### Added
- `fluxc setup | pilot | first-run` native onboarding + `fluxc self-update` (prebuilt fetch).
- `docs/VERSION_LEDGER.md`: release-provenance section — every release records its recomputable
  flux-rev identity in-tree.

### Diagnostics worth knowing
- `FLUXCACHE …` lines on a no-op build are cargo REPLAYING CACHED STDERR from old traced
  builds, not live wrapper calls.
- Long-running `fluxc mcp`/`serve` processes hold the pre-rebuild binary — restart them.

### Deferred to next cycle
- FIP-0002 Phase 2 remainder (shared cache dir surviving `rm -rf target`, eviction defaults),
  FIP-0001 groundwork (MIR-drift CI, `Frontend` trait, IR spec doc), workspace default-members
  prune, canonical wrapper path + `rust-toolchain.toml` pins (fleet-wide fingerprint stability).


## v0.34.0 — The type-complexity ladder: enums, generics, named consts (2026-07-01)

Ladder rungs 4–6 land together with the multi-aggregate parameter fixes — tuple/struct/enum
aggregates now construct → pass → bind → return end-to-end through the native backend.

### Added
- **Data-carrying enums end-to-end** (rung 4 pt 2): construction keeps variant path + payload args;
  payload extraction uses the `_N|Variant|K` downcast-projection encoding (raw payload index — the
  backend computes the real field offset from the enum layout). C-like enums with explicit/running
  discriminants.
- **Generics via monomorphization** (rung 5): turbofish call sites (`id::<i64>`) clone the generic
  template per distinct instantiation (`id$i64`), substitute type params, rewrite call sites, drop
  unused templates. Programs without turbofish are provably unchanged (early-out).
- **Named integer consts** (rung 6): `const HALVING_INTERVAL: u64 = 2_100_000` resolves at MIR level
  with **width-tagged substitution** (`2100000_u64`) so the backend picks the right Cranelift type,
  not i64 — found dogfooding sigil-emission::block_reward.
- structs/enums are now parsed from source and wired into the MIR-direct path (phase3), instead of
  empty placeholder tables.
- mir-corpus: `data_enum` / `data_enum2` / `nested_enum` fixtures + pinned-rustc (1.93.1)
  `.mir.expected` baselines (FIP-0001 frozen-IR groundwork).

### Fixed
- **Multi-aggregate parameter binding** (the 701 bug): `bind_param_flat` keyed `tuple_vars` by the
  global param cursor instead of a per-local flat-leaf index, so every aggregate param after the
  first silently bound to zero. Per-local leaf counter + `walk_to_field` path-type resolution.
  Proof: `fn second(p: Point, q: Point) -> i64 { q.x }` objdumps `xor rax,rax` → `mov rdx,rax`.
- **MIR param parser**: the param-list close-paren is now matched by `()` nesting depth and params
  split on top-level commas only — inline tuple-typed params (`fn f(a: (i64,i64), b: (i64,i64))`)
  previously truncated at the tuple's inner paren/comma into a mangled type + phantom param. This
  also resolves the downstream caller-side CLIF verifier errors on multi-aggregate call sites.
- flux-frontend unit tests updated to the current encodings (`_N|Variant|K`, normalized copy-strip);
  suite green 16/16 (was 14/16).

### Verified
- e2e via `fluxc run`: `pick((1,2),(3,4))` → exit 34 (correct; 0 if b unbound, 12 if a misused).
- flux-backend 43/43 · flux-frontend 16/16 (`flux_combo`, 6.0s) · objdump-verified SysV arg binding.

### Notes
- `Cargo.lock` intentionally not committed this release — the working-tree lock carries deps from
  in-flight WIP outside this cut (flux-context); it rides with its owner's commit.
- See `docs/VERSION_LEDGER.md` (new) for how the version tracks reconcile.


## v0.33.0 — Cache: drop volatile -L from the cache key (2026-06-25)

(Backfilled entry — v0.33.0 was tagged without a changelog entry.)

### Fixed
- **The 0% cache-hit root cause**: the volatile `-L` path was part of the cache key, so no key ever
  matched across builds. Dropping it took measured hits from 0% → ~40% (within-build/predicted).
  One of several layered cache bugs — see FIP-0002 for the full determinism story (restore stayed
  gated behind `FLUX_CACHE_RESTORE=1` in this release).
- jq-stdin-poison build fix: quarantine stray stdin reaching cargo's rustc target-probe.


## v0.32.0 — Real call signatures: aggregate-returning calls (2026-06-25)

### Added
- **Call-site multi-result destructuring**: a function returning a tuple (multi-value return signature) can now be called and its result destructured — the caller distributes each return value into the destination's scalar-replaced fields (previously only `inst_results[0]` was bound). Completes the aggregate-by-value call story.

### Notes
- Known/internal calls already use their real declared signatures (via `func_sigs`); external calls remain conservatively arity-guessed `i64->i64` (guessing external sigs is link-but-crash territory).
- Enums (`SetDiscriminant` + downcast) are the next milestone.


## v0.31.0 — Aggregates: tuples & structs (2026-06-25)

The native Phase-3 path now compiles flat tuples and structs — the first "real programs" milestone.

### Added
- **Structs** as named tuples: `P { x, y }` construction + `p.x`/`p.y` field access, scalar-replaced into per-field Cranelift Variables via a struct-layout table built from the source's struct defs (`unit.structs`, parsed but never read until now). Field access reuses the tuple `_N.M` projection path.
- **Aggregate-by-value return ABI**: tuple-returning functions emit a multi-value return signature (one AbiParam per field).
- Routing: `has_structs` joins `has_tuples` in selecting the MIR-direct path; `parse_rhs` learns the `Name { field: val }` struct-construction rvalue.

### Notes
- Tuples (construct/read/multi-field/arithmetic) already worked via the MIR-direct path (verified live: `(10,20).0+.1`=30, 3-tuples, sum-of-squares); this release adds structs + the aggregate-return ABI and locks both with tests.
- Scope: flat (single-level) aggregates of scalar fields. Nested aggregates, enums, and struct-by-value returns are follow-ups (enums -> 0.32).


## v0.30.0 — Scalar Completeness (2026-06-25)

Native Phase-3 backend now compiles **any program built from Rust's scalar primitives** — every integer width, both signs, unary operators, and numeric casts — correctly and verifier-clean.

### Added
- **Integer width correctness**: constants/results coerced to their declared width (i8/i16/i32/i64) via sextend/ireduce; a `coerce_int_width`/`unify_int_width` seam threads declared types through the value helpers so non-i64 arithmetic/comparison no longer mismatches widths.
- **Signedness**: u32/u64 select unsigned ops (udiv/urem/ushr + unsigned compares); signed types keep sdiv/srem/sshr + signed compares.
- **Unary operators**: `-x` (ineg/fneg) and `!x` (bnot for ints, low-bit flip for bools).
- **Numeric casts** (`as`): int widen/narrow (sextend/ireduce), int↔float (fcvt_from_sint / fcvt_to_sint_sat), f32↔f64 (fpromote/fdemote).

### Verified
- 30 verifier-backed unit tests (object emission via define_function) incl. i64/float non-regression controls; end-to-end on epsilon's rustc 1.93 via `fluxc run`.

### Notes
- Unsigned widening / int→float use signed conversions by default (correct for the common signed case; full unsigned-conversion refinement is a follow-up). The IR distinguishes u32/u64 (not u8/u16/usize) for signedness.


## v0.29.0 — Numeric & Bitwise Codegen (2026-06-25)

Native Phase-3 backend (`flux-backend`) now compiles the full set of scalar **binary** operators correctly and verifier-clean.

### Added
- **Float arithmetic & comparison** (`f64`/`f32`): `+ - * /` and `== != < > <= >=` emit `fadd/fsub/fmul/fdiv` + `fcmp` (previously emitted integer ops on float bit patterns → CLIF verifier rejection). MIR float literals parsed in both forms (`0f64`, `1.5f64`), threaded through the MIR-direct and Expr codegen paths.
- **New operators**: modulo `%` (`srem`), bitwise XOR `^` (`bxor`), shifts `<<` `>>` (`ishl`/`sshr`).

### Fixed
- **Bitwise `&` / `|` compiled to addition** — `syn::BinOp::BitAnd`/`BitOr` fell through to `BinOp::Add`; now map to `band`/`bor`.
- **Division was unsigned** (`udiv`) despite signed comparisons — now `sdiv`.

### Verified
- 13 verifier-backed unit tests (object emission via `define_function`) + a negative control proving the bug was real; end-to-end on epsilon's rustc 1.93 via `fluxc run` (`%`→2, `&`→8, `^`→6, `<<`→16, signed `-18/4`→-4, float loop→7).


## v0.6.0 — Self-Hosting Compiler (2026-05-27)
**Commit:** `cdc96b3` · 1054 files · 1132+ / 424-

### New Crates (3)
- `flux-p2p`: DAGKnight consensus + SAP/X-Algo scoring + libp2p swarm (dagknight.rs 501L, sap.rs 292L, x_algo.rs 317L, swarm.rs 232L)
- `flux-mempool`: Narwhal-inspired instant confirm mempool (<50ms receipts, 32-byte QuickVerify, fee-priority ordering)
- `flux-search`: BLAKE3-native TF-IDF search engine with PageRank + SAP-boosted ranking (lib.rs 511L, pagerank.rs 264L, ranking.rs 255L, benches.rs 125L, updater.rs 416L)
- `flux-science`: Quantum gravity + black holes + inflation (6 modules, 19/19 tests)

### Enhanced Crates (1)
- `fluxc`: Added serve module (embedded HTTP+SSE), MCP server (5 tools), stats tracking, supercluster mode, --release/--rust-only/--frontend-only flags

### Dashboard
- 8 tabs: Overview, Wallet (OAuth2), Autopilot, Bitcoin (Binance), DCA, Git, QuillonOS, Benchmarks
- Particle animation system (canvas-based, 60fps)
- Toast notification system (AI/build/pay/DCA/git events)
- Priority sliders with P0-P3 classification + score estimator
- Tempo autopilot with progress bars + timeline
- Agentic Money AI Cursor (agentic_cursor.html)

### Benchmarks (real, measured)
| Benchmark | Result |
|-----------|--------|
| flux-search: Index 1K docs | 17ms (58.8 docs/ms) |
| flux-search: Query 1K | 2ms |
| flux-search: Index 10K docs | 291ms (34.4 docs/ms) |
| flux-search: Query 10K | 26ms |
| flux-search: Cached | 0ms |
| PageRank 100 nodes | 0ms |
| fluxc self-hosting | 0.52s incremental |
| flux-science tests | 19/19 pass |
| Cold→Warm speedup | 6.4× |

### Infrastructure
- `flux_sse_bridge.py` v3: Real data bridge with auto stats writer (systemd service)
- `DASHBOARD_CHECKLIST.md`: 47-point feature preservation checklist
- `FLUX_NEXT_VERSIONS.md`: Danish roadmap v0.5.0→v1.0.0
- `FLUX_ROADMAP.md`: English roadmap with benchmarks
- `flux-foundation-whitepaper.tex`: LaTeX academic paper
- Git repo: Private, local (2 commits, master branch)
- Backup: `/home/storage/project-flux-20260527-042600.tar.gz` (364 MB)

---

## v0.4.0 — Initial Prototype (2026-05-27)
**Commit:** `49e5fd7` · Initial commit

### Crates (8)
- fluxc, flux-driver, flux-cache, flux-db, flux-gpu, flux-zk, flux-gui

### Features
- DAGKnight consensus protocol implementation
- SAP scoring (5-factor weighted model)
- X-Algo cross-scoring (5 dimensions)
- Instant confirm mempool design
- Quantum science engine (Schwarzschild, Hawking, inflation)
