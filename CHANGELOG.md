# Flux Foundation — Commit Log & Changelog

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
