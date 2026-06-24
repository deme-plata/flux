# Flux — Master Overview & Development Handoff

**Version:** 0.28.0  ·  **Prepared:** 2026-06-24 (v2)  ·  **Audience:** the next developer/agent (Codex 5.5)
**Provenance tags:** `[live]` = read from epsilon / the running `fluxc` on 2026-06-24 (read-only) · `[repo]` = from source/config · `[memory]` = prior operational note, re-verify before acting.

> **Keep this document honest by construction.** Flux's *static* strings drift badly (README badges, hardcoded version strings, the README "Honest status" table — all stale; see §6). Its *live* introspection is honest. **Before trusting any number here, re-confirm with Flux's own tools:** `fluxc xray --json`, `fluxc status --json`, and the `flux_health_report` / `flux_diagnose` MCP combos. When a static doc and live introspection disagree, **live wins.** (This is also why the repo now ships an `AGENTS.md` — see §12.)

> **The one thing to get right first time:** `fluxc` is a **real partial compiler (~33% of a standalone one)** — **not** "just a `rustc` wrapper" (the inverse error), and **not** a finished self-hosting compiler (the marketing). It **owns** an IR + a genuine Cranelift→ELF backend; it **borrows** rustc for all parse-with-semantics and `cc` for linking; it does not self-host; and its default build path is currently red.

---

## 1. What Flux actually is

Flux is an **AI-native build platform / "Universal Build Orchestrator"** in Rust, plus the **SIGIL / Quillon** blockchain + "agentic-money" stack it builds and operates. At its center is **`fluxc`**, best understood as a **hybrid compiler: it owns a real backend but borrows a real frontend.**

**What `fluxc` genuinely owns** `[repo]`:
- **Its own IR** (`flux-frontend`: `TranslationUnit` / `FunctionDef` / `Expr` / `TypeRef`) and a **real Cranelift backend** (`flux-backend`, ~621 LOC, `cranelift-* 0.114`) that emits **native ELF object files** — not just text. The MIR-direct codegen path handles loops, mutable locals, and arbitrary CFG.
- A working **compile → link → run** path: `fluxc run x.rs` emits an ELF object, links it with `cc`, runs it, returns the exit code — with a **BLAKE3 object cache** and optional **SQIsign provenance proofs** on artifacts.

**What `fluxc` borrows / does not own** `[repo]`:
- **The entire frontend's semantics.** Parsing on the simple path is the `syn` crate; on the high-fidelity path, **all** typecheck, borrow-check, trait resolution, and monomorphization come from **rustc**, which `fluxc` literally shells out to via `rustc --emit=mir` and then parses the MIR text. **Every memory-safety guarantee is therefore still rustc's, not Flux's own.**
- **The linker** is `cc` (gcc/clang), not an integrated Flux linker.
- **`fluxc` itself is built by cargo + rustc — it does not self-host.** The whitepaper's "self-hosting" claim is **aspirational, not realized.**

**Operationally** `[live]`:
- `fluxc --version` → `rustc 1.93.1 (01f6ddf75 2026-02-11)`. This is **wrapper passthrough** (it proxies the wrapped rustc), *not* proof that Flux has no compiler. `fluxc version` (subcommand) prints **`fluxc 0.28.0`**.
- The **default workspace build path is Phase 1** (`fluxc self` = cargo build with `RUSTC_WRAPPER=self`) — pure orchestration/caching that owns nothing of compilation. **As of 2026-06-24 14:17 it is GREEN ✅** (`fluxc self` exits 0; the earlier "jq probe" red was a poisoned cargo target-info cache, now quarantined — §6). The genuine compiler is **Phase 3** (§4).
- The memory-safety guarantees in `flux-philosophy/flux-rc1-memory-safety.tex` remain **"design intent under test," not proven** — and are presently rustc's, not Flux's (§8 item 7).

**Honest one-liner:** *Flux is an AI-native Rust build-and-agent platform (with a large blockchain/DeFi/PQ-crypto surface) that owns real Cranelift codegen + its own IR, but borrows rustc as its semantic frontend and `cc` as its linker — a MIR-consuming codegen + toolchain on the road to standalone, not a from-scratch compiler and not a mere wrapper.*

### The surrounding system (context you'll need)
- **SIGIL** — the `sigil-g0` blockchain / node (`sigil-top`, `sigil-rpcd`). `sigil-top` is an **application built by fluxc**, at `1.0.0-rc9` — *do not confuse its version with Flux's 0.28.0.* `[memory]`
- **Quillon** — `q-api-server` mainnet-genesis network, native token **QUG**. The bank surface is **propose-only / observer mode** (`policy_mode=observer`, `proposal_only=true`); money moves need a signed intent + 2-of-2. `[memory]`
- **xAlgo** — a multi-dimensional *earned* trust score (temporal trust, consensus alignment, tx quality, topology rank, economic efficiency). `[repo]` (cockpit `main.js`)
- **flux-moe** — local agentic-money LLM work (Qwen3 on an RTX 2080). Separate workstream; **not** the compiler. `G:\dev\flux-moe-cli`, `flux-moe-swarm-prompts.md`. `[memory]`

---

## 2. Where it lives & how to reach it

| Thing | Location |
|---|---|
| **Flux repo (source of truth)** | `epsilon:/home/storage/deepseek-codewhale/flux` `[live]` |
| **GitHub remote** | `https://github.com/deme-plata/flux.git` (org `deme-plata`) `[live]` |
| **`fluxc` binary** | `…/flux/target/debug/fluxc` (575 MB debug, last built **2026-06-19 01:17**) `[live]` |
| **Cross-build cache** | `/home/storage/deepseek-codewhale/.target-shared` `[memory]` |
| **MCP server** | `fluxc mcp` (stdio JSON-RPC, ~162 tools per `--help`, ~185 enumerated live) `[live]` |
| **Dev cockpit (local)** | `C:\Users\Viktor S. Kristensen\flux-cockpit` — `node combo-bridge.mjs` → `http://127.0.0.1:7799` |

**`epsilon` is a shared production node** — overloaded, SSH flaky (q-api RAM leak, periodic OOM). Default to **read-only**; never run raw `cargo` in the workspace (`fluxc`/`flux_*` only); be sparing with heavy MCP combos (some compile).

**MCP combo wiring:** `flux-cockpit/combo-bridge.mjs` (`:7799`) serves the cockpit UI and proxies `/rpc/*`→`epsilon:8099` (sigil-rpcd), `/qnk/*`→`:18088` (Quillon), `/api/*` & `/sse`→`:8083` (build stats + event feed), and `POST /mcp` → `ssh epsilon 'fluxc mcp'`.

> **Landmine `[memory]`:** editing `~/.ssh/config` with tooling breaks the bridge (strict Windows OpenSSH "Bad owner or permissions") → MCP calls silently return `null`. Fix: `G:\ollama-installer\fix-ssh-perms.bat`.

---

## 3. Architecture (crate map)

**143 crates · 155,265 LOC · self-reported 62.2% "ideal"** `[live]` (`find crates -maxdepth 1` lists 144 entries incl. the parent — health report's **143** is the true crate count). One massive Rust monorepo (`crates/`). Rough domains:

- **Compiler / build platform:** `fluxc` (CLI dispatch), **`fluxc-core`** (orchestration core — the weak crate, §6/§7), `fluxc-mcp` (the ~185-tool surface), **`flux-frontend`** (IR + parsers, §4), **`flux-backend`** (Cranelift codegen, §4), `flux-graph` (dep resolution / Phase 2), `flux-driver` + `flux-cache` (the wrapper cache, §6), `flux-cortex` (architect→predict→optimize→learn loop), `flux-search`, `flux-context`, `flux-hotswap`.
- **Self-hosted VCS:** `flux-rev` (real ~1.2k-LOC content-addressed VCS, §6) + `flux-aether` (content store; `flux_aether_rev_bridge`/`_watch`).
- **Blockchain / SIGIL:** `flux-consensus`, `flux-narwhal-core`, `flux-mempool`, `flux-db` (native `SkeletonStore` fast-sync — actively built this week), `flux-bank-*`, `flux-history`, `flux-keel`.
- **DeFi / markets:** `flux-cashewswap` (AMM), `flux-0x`, `flux-gpu-market`, `flux-market`, `flux-agent-trade`.
- **PQ crypto / ZK / provenance:** `flux-sqisign` (SQIsign-L5 + Ed25519 hybrid), `flux-kyberkem`, `flux-lattice-guard`, `flux-zk-stark`, `flux-ivc`(+wasm).
- **Distributed / infra:** `flux-p2p` (libp2p; **currently red**, §6), `flux-net`, `flux-dns`, `flux-fleet`, `flux-visor`, `flux-gpu`, `flux-vast` (GPU rental).
- **UI tooling (NOT the compiler):** `flux-fcx` (TSX-dialect → Slint transpiler to beat Electron), `flux-fcx-logic` (QuickJS), `flux-fcx-wasm`, `flux-gui` (Slint).
- **AI / agents:** `flux-moe`, `fluxc-gemma`, `flux-ai`, `flux-ai-bench`.
- **Legacy smell:** `flux-legacy`, `flux-legacy2`, `flux-legacy3`.

---

## 4. Compiler architecture — the Phase 1 / 2 / 3 pipeline

**Phases 1 & 2 are rustc-driven orchestration; Phase 3 is the actual Flux compiler.** Treat phase numbers as *distinct build paths*, not maturity levels of one thing. `[repo/live]`

| Phase | Command | What it is | Owns compilation? | Status |
|---|---|---|---|---|
| **Phase 1** | `fluxc self` | `cargo build` with `RUSTC_WRAPPER=self`. Cargo drives the build graph; `fluxc` wraps each rustc call (passthrough + content-hash cache). **THE DEFAULT workspace build.** | No — orchestration/caching only | **🔴 RED** (exit 101, jq) |
| **Phase 2** | `fluxc build --no-cargo` | Drops cargo; uses **`flux-graph`** dep resolution to drive rustc directly (per-batch / distributed). | No — still rustc; removes the cargo driver | 🟡 |
| **Phase 3 (3d)** | `fluxc compile` / `compile-native` / `run` / `heal` | **The actual Flux compiler.** `crates/fluxc-core/src/phase3.rs` (1247 LOC). 3a–3c scaffold the native line; **3d is realized.** | **Yes (codegen + IR)** — borrows rustc frontend, `cc` link | 🟢 works, MVP |

Source anchors `[repo]`: Phase 1 status at `crates/fluxc/src/mcp.rs:1173` + `fluxc-core/src/lib.rs:1007`; Phase 2 at `fluxc-core/src/lib.rs:837,841,861,969` + `distributed.rs`; Phase 3 entry `compile_file()`/`compile_impl()`/`compile_run()`/`heal()` in `phase3.rs`.

### `flux-frontend` — two parse paths → Flux IR `[repo]`
`crates/flux-frontend` (lib.rs 377 + mir.rs 722 = 1099 LOC). Two front ends feed one IR:
- **`syn` parse path** (`parse_file()`): the `syn` crate parses Rust → Flux IR (`TranslationUnit{file_path,functions,structs,imports}`, `FunctionDef`, `Expr`, `Literal`, `BinOp`, `TypeRef`). **No typecheck, no borrow-check** — syntax only.
- **rustc-MIR-text parse path** (`mir.rs`): parses the **text** of `rustc --emit=mir` → `MirFunction`/`MirBlock`/`MirStmt`/`MirTerminator`, then `parse_mir()` → `lower_mir_to_ir()`. High-fidelity: it inherits rustc's full semantics because the MIR is already typechecked/borrow-checked/monomorphized. In-code: *"Parses the text MIR format that rustc --emit=mir produces. Feeds into flux-backend for Cranelift codegen."*

### `flux-backend` — Cranelift → ELF object `[repo]`
`crates/flux-backend` (621 LOC). Deps `cranelift-codegen/frontend/module/object/native 0.114` + `target-lexicon 0.12`. `compile_unit_to_object_with_mir()`:
1. builds a `cranelift_object::ObjectModule`,
2. declares local fns `Linkage::Export`,
3. scans bodies for external `Call`s, declares them `Linkage::Import` with **arity-guessed `i64→i64` sigs** (in-code: *"Real signature inference from rlib metadata is TBD"*),
4. defines each body via **MIR-direct path** (`compile_mir_into_function`, Cranelift `Variable`s — supports loops/mutable locals/arbitrary CFG) or **Expr path** (`compile_expr_into_function`, pure-expression bodies),
5. emits a **native ELF object**. `cl_type` maps Flux types → Cranelift types (i64-centric).

### `phase3.rs` commands `[repo]`
- **`fluxc compile FILE`** — *"Phase 3d (syn parser)."* `parse_file` (syn) → IR → `compile_unit` → **prints CLIF text**. Stops at CLIF — no typecheck, no ELF.
- **`fluxc compile-native`** — *"MIR Bridge (rustc MIR → CLIF)."* `rustc --emit=mir` → `parse_mir` → `lower_mir_to_ir` → `compile_unit_to_object_with_mir` → **ELF `.o`** → optional SQIsign **provenance proof**.
- **`fluxc run x.rs`** (`compile_run`, JIT) — BLAKE3-hash source (cache key) → `rustc --emit=mir` → parse → lower → Cranelift object (**cached at `temp/flux_jit`**) → **link `cc obj -o exe`** → execute.
- **`fluxc self-heal` / `heal()`** — self-healing loop: compile→test→fail→**AI diagnose (two-mind: local `qwen3.6` proposer + DeepSeek-V4 vetoer)**→fix→recompile; provenance-sign + webhook on success, escalate to swarm on exhaustion.

Shell-outs to `rustc`/`cc` confirmed at `phase3.rs:158,205,316,343,568,811,819,938,1052`; wrapper passthrough at `fluxc-core/src/lib.rs:670–750`. `[repo]`

> **Two different caches — don't confuse them.** The Phase-3 path keys objects by **BLAKE3 of source** under `temp/flux_jit` (works). The Phase-1 `RUSTC_WRAPPER=self` **build cache** is the one measured at **0% hit** (§6) — its gap is in `apply_cached_outputs` (writes a dep-info marker, not rmeta/rlib bytes; `docs/FLUXC_CACHE_GAP.md`).

### What is REAL vs STUBBED / TBD `[live/repo]`

| Area | Status | Detail |
|---|---|---|
| Own IR | ✅ **REAL** | `TranslationUnit`/`FunctionDef`/`Expr`/`TypeRef` in `flux-frontend` |
| Cranelift backend → ELF | ✅ **REAL** | `flux-backend` ~621 LOC, native ELF objects + CLIF |
| MIR-direct codegen | ✅ **REAL** | loops, mutable locals, arbitrary CFG via Cranelift `Variable`s |
| compile→link→run JIT | ✅ **REAL** | `fluxc run`: BLAKE3 object cache → `cc` link → execute |
| Provenance signing | ✅ **REAL** | SQIsign proof on emitted artifacts |
| Self-healing loop | ✅ **REAL** | `heal()`: compile→test→AI-diagnose→fix→retry |
| **Parse-with-semantics** (typeck/borrowck/traits/mono) | ❌ **BORROWED** | done by **rustc** (`--emit=mir`); syn path does **not** typecheck. Memory safety is rustc's. |
| **Linker** | ❌ **BORROWED** | `cc obj -o exe`; no own/vendored linker |
| **`fluxc` self-compilation** | ❌ **NOT DONE** | built by cargo+rustc; "self-hosting" aspirational |
| Type system | 🟡 **STUB** | **i64-centric**; no f64/i32/u*/bool/pointer ABI yet (`cl_type`) |
| External call sigs | 🟡 **STUB** | **arity-guessed `i64→i64`**; rlib-metadata inference TBD |
| Aggregates / generics | 🟡 **STUB** | structs/enums/tuples/generics/traits largely unsupported natively |
| `fluxc compile` (syn leg) | 🟡 **PARTIAL** | stops at CLIF — no ELF, no semantics |

**Bottom line:** the *codegen half* is real and credit-worthy; the *frontend half* and the *linker* are rustc/`cc`, and `fluxc` doesn't self-host. Honest characterization: **"a MIR-consuming Cranelift codegen + toolchain," ≈ one-third of a standalone compiler.** The load-bearing identity decision — own the frontend (native typeck/borrowck, XL) vs. formally adopt `rustc --emit=mir` as a version-pinned frontend (S, doc-only) — is unmade (§8 item 7), and the whitepaper's "self-hosting" claim is only honest under one of those.

---

## 5. The MCP tool surface ("the combos")

`fluxc mcp` exposes **~162 tools (`--help`) / ~185 (live `tools/list`)** — reconcile that count (§6). High-value clusters:

- **Read/introspect (safe — use these for ground truth):** `flux_health_report`, `flux_diagnose`, `flux_stats`, `flux_swarm_status`, `flux_peer_list`, `flux_bank_status`, `flux_swot`, `flux_version_status`; CLI `fluxc xray --json`, `fluxc status --json`.
- **Dev pack (inner loop; some compile on epsilon — slow):** `flux_combo`, `flux_combo_supersonic`, `flux_compile`, `flux_test`, `flux_fullcheck`, `flux_optimize`, `flux_predict`, `flux_dev`, `flux_qspec` (explain a compile error), `flux_format`, `flux_search`.
- **Refactor/quality:** `flux_refactor_audit/extract/generate/score`, `flux_context_audit`, `flux_legacy_analyze/plan/stability`.
- **Swarm:** `flux_swarm_register/claim/release/message/inbox/status`, `flux_file_claim/list/release`, `flux_goal_post/list/consensus`.
- **Provenance/crypto:** `flux_sign_sqisign`, `flux_zk_verify_10ms`, `flux_zk_pq_status`.
- **Money (PROPOSE-ONLY — human signs, 2-of-2):** `flux_sigil_txn_send`, `flux_sigil_dex_swap`, `flux_bank_propose_transfer`.

> **Rule `[memory]`:** the bridge returns `ok:true` even for "Tool not found" — confirm names via `tools/list`, never guess.

---

## 6. Current state — ground truth (2026-06-24) `[live]` unless noted

### Version reality — rampant drift; pin one line
| Signal | Source | Claims | Verdict |
|---|---|---|---|
| `Cargo.toml` / `fluxc version` / health | live | **0.28.0** | ✅ **canonical** |
| `crates/fluxc/src/mcp.rs:571` | repo | hardcoded `"fluxc 0.9.6 … MCP: 30 tools"` | 🔴 stale hardcode |
| README badges | repo | `beta_v0.22` · `crates-111` · `MCP_tools-130+` | 🔴 stale (0.28.0 / 143 / ~185) |
| `CHANGELOG.md` (top) | repo | `v0.6.0 — Self-Hosting Compiler` | 🔴 stale + aspirational title |
| `docs/PLATFORM_STATUS_2026-06-06.md` | repo | `v0.25` | 🔴 stale |
| Git branch | live | `v0.5.2-dev` | ⚠️ separate namespace |
| Last git tag | live | `sigil-v0.8.1` (HEAD +71) | ⚠️ app/tag line |
| `sigil-top` app (built *by* flux) | live | `1.0.0-rc9` | ⚠️ app-version, not platform |
| Philosophy doc | repo | `1.0.0-rc1` | ⚠️ aspirational |

**Recommendation:** pin the platform to **`0.28.0`**; replace the `mcp.rs:571` hardcode + README badges with build-time reads from `Cargo.toml`; namespace app-/doc-versions so they're never mistaken for the platform version. (Punch-list in the `flux-repo-onboarding` bundle, §12.)

### Health (live `flux_health_report — v0.28.0`)
- Architecture **62.2% ideal · 143 crates · 155,265 LOC**
- SWOT: **121 strengths · 135 weaknesses · 4 opportunities · 3 threats** (weaknesses > strengths)
- **Prediction(fluxc): 504 ms · 72% cache (predicted) · 92% test pass · 36% confidence**
- **Cache: 0.0% hit rate (0/5) · 739 builds · 66,267 ms avg**
- Top priority: decouple/test/doc **`fluxc-core`** (coupling 10%, ~18% coverage, ~30% docs)
- Same-day trend: test pass **96%→92%**, confidence **42%→36%** (regressing)

### Git
- ~200 commits **2026-06-05 → 06-23**, cadence tapering 8–18/day → **1–4/day**; sole author Viktor S. Kristensen. Recent work real and well-scoped (`flux-db` SkeletonStore, p2p fixes, stats instrumentation, v0.28.0 supersonic combo, cashewswap AMM, zk-stark soundness +12, SQIsign+Ed25519 provenance).
- **Working tree dirty:** modified `flux-context` (lib + route bin + Cargo.toml), `fluxc-mcp/swarm_compile.rs`; untracked `dispatch.rs`, `swarm_compile.rs.bak`, bench scripts. Commit or discard; the `.bak` is hand-edit WIP.
- `origin/HEAD` not resolvable locally — **can't confirm push state to GitHub; likely unpushed work.** Fetch origin, check ahead/behind.

### Build / test — base now **GREEN** ✅ (2026-06-24 14:17)
- **`fluxc self` exits 0** — full-workspace self-build, **~102s cold / ~15s incremental** `[live]`. `fluxc` binary rebuilt 2026-06-24 14:17 (591 MB).
- **Root cause of the old "jq probe" failure was a POISONED cargo target-info cache, not a live filter** (diagnosis by the Codex P0 session, verified): `target/.rustc_info.json` + `.target-shared/.rustc_info.json` had cached a *failed* rustc probe whose stderr contained a stray `jq` compile error (`{height:.h, hash:(.tiphash // .hash // ""), …}`), and cargo **replayed the cached failure without launching rustc** (confirmed via `strace`: no rustc `execve` for the failing probe). Fixed by **quarantining** those two cache files (→ `/home/storage/tmp/flux-rustc-info-quarantine-20260624-1`; they regenerated clean) + restoring the real `RUSTC_WRAPPER`/`FLUXC_WRAPPING`/`REAL_RUSTC` env in `fluxc-core::self_build` (it previously only *printed* the dogfood claim). → **Debugging rule:** if `fluxc self` ever fails at "target-specific information," grep `*/.rustc_info.json` for a cached `success:false` entry before suspecting rustc/cargo.
- **Still red — next P0:** `flux-p2p` *tests* don't compile (`p2p-recheck.log` `RECHECK_RC=101`: `error[E0282]: type annotations needed` in `tests/two_node_gossipsub.rs` + stale `libflux_p2p-….rlib` extern). The base *self-build* is green; the p2p *test* target is not yet fixed.

### Cache — 0% by construction (root cause documented)
`docs/FLUXC_CACHE_GAP.md` (2026-05-29) `[repo]`: Patch 1 (wiring) shipped → cache **populates** on every rustc call. But **hits are non-functional** — `apply_cached_outputs` writes a **dep-info marker, not the rmeta/rlib bytes**, so cargo checks freshness against the missing `.rmeta`/`.rlib` and **re-runs rustc anyway**. **Patches 2 + 3 (content-byte storage in `flux-cache` — its `outputs` map stores paths, not bytes — + byte-restoring apply in `flux-driver`) are designed, not implemented.** This precisely explains the live 0/5 hit / 66 s avg.

### flux-rev (self-hosted VCS) — **real, but under-adopted**
- `crates/flux-rev` is a genuine **~1,151-LOC content-addressed VCS** + binary (lib 531, search 319, hooks 203, sync 98): Blob/Manifest/Genesis/Revision objects keyed by **BLAKE3**, store `<workdir>/.flux-rev/objects/<blake3hex>`, P2P object sync over flux-p2p gossipsub, CLI `genesis/snapshot/checkout/log/diff/head` `[repo, docs/FLUX_REV.md]`. Used internally by `phase3.rs::search_code()` (`flux_rev::search::search_live`).
- **Gap is adoption, not realness:** the live `.flux-rev/` store held only **~2 objects** (HEAD + objects/, mtime Jun 18) `[live]`; it's a **separate `flux-rev` binary not wired as `fluxc rev`** (which just prints top-level help); an **argv-parse bug** created junk dirs `./--help/.flux-rev` and `./--message/.flux-rev`. **Action:** surface as `fluxc rev`, fix the argv parse, delete junk.

### Fleet / swarm `[memory/live]`
4 nodes: **epsilon** `89.149.241.126` (prod/src), **beta** `185.182.185.227` (dev/git, firewalled), **gamma** `109.205.176.60` (sync stalls), **delta** `5.79.79.158`. Observed `flux_swarm_status`: **2/4 online**, online nodes lacked `fluxc`/`sigil-top` binaries. Distributed-build story is aspirational now.

### Hygiene
Root littered: a file literally named **`&1`**, ~20 `fix_*.py`, scattered `*.log`, `swarm_compile.rs.bak`, the `--help`/`--message` junk dirs. Three `flux-legacy*` crates. Low-risk, high-signal cleanup.

---

## 7. Are we developing the compiler correctly? — honest verdict

**Short answer: the *method* is sound and unusually honest, and the *compiler is more real than it first looks* — `fluxc` genuinely owns an IR and a Cranelift backend that emits native ELF (Phase 3 is real work, not vapor). But the project is at an inflection point where breadth and velocity have outrun the *foundation*, and the foundation — the default build path, the cache, and `fluxc-core` quality — is visibly cracking. The native compiler is a credit-worthy ~33%-of-standalone MVP; the base it stands on is red. Freeze the surface, green the base, then extend the compiler (§8).**

### What's genuinely right ✅
1. **The compiler is real, not "just a rustc wrapper."** Flux owns its IR + a Cranelift backend (`flux-backend`, ~621 LOC) emitting **native ELF**, with a MIR-direct path (loops/mutable-locals/CFG), a working `compile→link(cc)→run` JIT + BLAKE3 cache, and provenance signing `[repo]`. Phase 3 (`phase3.rs`, 1247 LOC) is genuine compiler engineering — this progress deserves credit.
2. **Substantial and coherent** — 143 crates / 155k LOC, ~185-tool agent surface, a git history of well-scoped features. Not vaporware.
3. **Radical metric honesty is implemented.** `flux_health_report`/`diagnose`/`xray` surface the *bad* numbers (0% cache, 135 weaknesses, 36% confidence) `[live]`. The "earned, not asserted" philosophy is lived in the tooling — the project's best asset. Protect it (and extend it to the *docs*, §6).
4. **A credible, honestly sub-phased path** (Phase 1 wrapper → Phase 2 no-cargo → Phase 3 native), not an over-claimed finished compiler. §8 makes the remaining ~67% concrete.
5. **Pragmatic architecture** — bridging through rustc `--emit=mir` for semantics while owning codegen is the right call for fast leverage. *Catch:* it must be **named** as a deliberate posture (§8 item 7), not left ambiguous.

### What's going wrong 🔴 (priority order)
1. **The default build path — was red, now GREEN ✅ (2026-06-24).** Phase-1 `fluxc self` now exits 0 after the poisoned-cache fix (§6, §8 #1). *Remaining:* `flux-p2p` *tests* still don't compile (`E0282`) — the next P0 item — and the green baseline must be kept green (re-run `fluxc self` in CI). The deeper point stands: a self-hosting platform's own build must never be allowed to go red unnoticed.
2. **The cache value-prop is structurally non-functional — cause known.** 0% hit / 66 s avg over 739 builds; `apply_cached_outputs` restores a marker not bytes (Patches 2+3 unimplemented; `FLUXC_CACHE_GAP.md`) `[repo]`. fluxc's entire speed thesis is blocked on landing them (§8 #2).
3. **Core-quality is inverted.** `fluxc-core` — the heart, where Phase 3 lives — is the **most coupled (10%), least tested (~18%), least documented (~30%)** crate, #1/#2/#3 priority for days with no movement `[live]`.
4. **The native compiler is an i64 MVP.** Owns codegen but borrows the whole frontend + linker; i64-centric, arity-guessed `i64→i64` external sigs, no structs/enums/generics/traits natively `[repo]`. Expected for ~33%-of-standalone — but "self-hosting" is a goal, not a property, and the whitepaper/CHANGELOG state it as fact.
5. **Metrics trending down** — test pass 96%→92%, confidence 42%→36% in a day `[live]`.
6. **Scope outruns depth.** 0x, Vast, Cosmos, AMMs, DNS, email, bluetooth, "nations," three legacy generations — while the compiler core, default build, and cache aren't solid. Breadth is now a *liability*.
7. **Docs over-claim vs the live picture.** README "compiler self-builds ✅ green" (FALSE), "cache measured speedups" (0% live); whitepaper frames self-hosting as realized (aspirational); version strings incoherent (§6). `fluxc --version → rustc 1.93.1` is correct *passthrough* and should be documented as such. Honesty in the *docs* is the same asset as honesty in the *tooling*.

### The corrective
**Freeze new surface. Green the base — Phase-1 build, cache hits, `fluxc-core` quality — until the self-build is reproducible and the cache hits; *then* extend the genuinely-real Phase 3 compiler up the §8 roadmap.** The compiler progress is real and worth protecting; it just can't be exercised, measured, or self-hosted on a red, uncached base. And before shipping: correct the README/whitepaper/CHANGELOG/version claims to match live numbers.

---

## 8. Roadmap to a true standalone compiler

`fluxc` today ≈ **33% of a standalone compiler**. Sizes: **S** = days · **M** = ~1–2 wk · **L** = multi-week · **XL** = multi-month. Items 1–2 unblock-and-measure; 3–8 build the compiler up the type-complexity ladder; 9 is the final milestone, gated on 3–7.

| # | Step | What | Why | Size |
|---|---|---|---|---|
| **1** | ✅ **DONE (2026-06-24)** — Fix the Phase-1 build | Was a **poisoned `.rustc_info.json` cargo cache** (cargo replayed a cached failed probe), not a live filter. Quarantined the two cache files + restored the real `RUSTC_WRAPPER`/`FLUXC_WRAPPING`/`REAL_RUSTC` env in `self_build`. `fluxc self` now exits 0. | Gave the **green baseline** everything else needs. | **S** ✅ |
| **2** | **Make the cache actually hit** | Implement designed Patches 2+3: store rmeta/rlib **bytes** in `flux-cache` (not path markers); byte-restore in `apply_cached_outputs`. | Live 0% hit / 66 s avg; the wrapper's headline value-add is non-functional `[repo]`. | **M** |
| **3** | **Real signature inference from rlib metadata** | Replace arity-guessed `i64→i64` external-call sigs with true sigs from rlib/rmeta. | Cross-crate calls are unsound guesses, blocking non-trivial programs `[repo]`. | **M** |
| **4** | **Backend type coverage beyond i64** | Add `f64`/`i32`/`u*`/`bool`/pointer types + correct ABI lowering in `cl_type`/codegen. | i64-only can't represent real programs. | **M–L** |
| **5** | **Aggregate types: structs, enums, tuples** | Layout + field/discriminant codegen in the MIR→CLIF path. | Nothing realistic compiles without them. | **L** |
| **6** | **Generics + trait dispatch (native path)** | Either consume rustc's monomorphized MIR end-to-end, or implement monomorphization/vtable lowering. | Traits/generics are pervasive in the workspace. | **L** |
| **7** | **Decide frontend posture — own it OR formalize rustc-as-frontend** | EITHER build native typeck/borrowck/trait-resolution (own the memory-safety guarantees), OR explicitly adopt `rustc --emit=mir` as a contracted, version-pinned frontend and document Flux as a "MIR-consuming codegen + toolchain." | **Load-bearing identity decision** — the whitepaper's "self-hosting" claim is only honest under one of these. Route via a FIP (§12). | **XL** own / **S** doc |
| **8** | **Own/vendor the linker** | Replace `cc obj -o exe` with an integrated linker (drive `lld`, or vendor a minimal ELF linker). | "No own linker" is a standalone gap `[repo]`. | **M** (lld) |
| **9** | **Self-host `fluxc` via Phase 3** | Compile `fluxc` itself with the Flux compiler instead of cargo+rustc. | The whitepaper's central claim and the true test of standalone status — realistically last, gated on 3–7 (fluxc uses structs/enums/generics/traits heavily). | **XL** |

> **Sequencing:** 1–2 are pure unblock/measure; until Phase 1 is green and the cache hits there's no baseline to develop or benchmark the native path against. 3–8 climb the type-complexity ladder. 9 is what *becomes possible* once 3–7 exist. The honest identity to put in the whitepaper **today** is item 7's second option; the first option is the multi-month ambition.

---

## 9. Recommended next steps for Codex 5.5 (ordered)

> Guardrails `[memory]`: **never raw `cargo`** in the workspace (use `fluxc`/`flux_*`); **honest numbers only** (run+read before quoting); **all money moves propose-only**; before scoring anything 0/N, dump one raw output (most "failures" are harness/prompt).

**P0 — make the build green & reproducible**
1. Fix the cargo target-probe `jq` contamination (§8 #1). Validate `env -i … cargo metadata` under a clean shell.
2. Green `flux-p2p`: fix `E0282` in `tests/two_node_gossipsub.rs` (explicit annotations) + the stale `libflux_p2p` extern (clean dep rebuild). Run `fluxc test`, capture the real pass count.
3. Commit/discard the dirty tree; fetch origin; confirm push state to `github.com/deme-plata/flux`.
4. **Adopt the repo-onboarding bundle** (§12): commit `AGENTS.md` / `CLAUDE.md` / `FLUX_FACTS.json` to repo root and apply the stale-signal punch-list. (Cheap; prevents the next analyst repeating this audit.)

**P1 — restore the foundation**
5. Fix the cache (§8 #2). Re-measure with `flux_stats`/`flux_health_report`.
6. Harden `fluxc-core`: decouple (<10%), raise coverage from ~18%, document the public API.

**P2 — reflexive story + tidy**
7. Wire `fluxc rev`, fix the argv bug, delete `./--help` `./--message` `&1` `*.bak` (§6).
8. Reconcile versioning to one scheme; reconcile the MCP tool count (162 vs ~185).
9. Scope triage: keep/freeze/excise `flux-legacy/2/3` and the long-tail crates. No new surface until P0/P1 done.

**P3 — only after green:** advance the standalone-compiler ladder (§8 items 3→9).

---

## 10. Quick-start for a new agent

```bash
ssh epsilon            # shared prod, read-only first; SSH is flaky
cd /home/storage/deepseek-codewhale/flux

# Orient via LIVE introspection (trust these over static docs/badges):
./target/debug/fluxc status --json
./target/debug/fluxc xray --json
git --no-pager log --oneline -30 ; git status -s

# Inner dev loop — via fluxc, NEVER raw cargo:
export RUSTC_WRAPPER=$(pwd)/target/debug/fluxc FLUXC_WRAPPING=1 REAL_RUSTC=rustc CARGO_INCREMENTAL=1
./target/debug/fluxc check
./target/debug/fluxc test            # or flux_combo{package} / flux_test via MCP
# On a compile error: flux_qspec → explained diagnosis

# Exercise the actual Phase-3 compiler on a single file:
./target/debug/fluxc compile path/to/file.rs      # syn → CLIF text
./target/debug/fluxc run     path/to/file.rs      # MIR→Cranelift→cc→run

# MCP (one tool call over stdio):
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}' | ./target/debug/fluxc mcp
printf '%s\n' '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"flux_health_report","arguments":{}}}' | ./target/debug/fluxc mcp
```

---

## 11. Gotchas / landmines
- **Never raw `cargo`** in the workspace — go through `fluxc`/`flux_*`. `[memory]`
- **The "jq probe" red was a poisoned `.rustc_info.json` cache** (cargo replays a cached failed probe), **fixed 2026-06-24** (quarantined + `self_build` env restored; `fluxc self` exits 0). If it recurs, grep `*/.rustc_info.json` for `success:false` before suspecting rustc/cargo. `[live]`
- **`fluxc --version → rustc 1.93.1`** is wrapper passthrough, not "no compiler." `[live]`
- **Editing `~/.ssh/config` via tools** breaks the cockpit bridge → MCP returns `null`. Fix: `G:\ollama-installer\fix-ssh-perms.bat`. `[memory]`
- **epsilon is overloaded** (q-api leak, OOM, SSH timeouts). Don't raise sigil-rpcd's 6 G cap. Heavy combos sparing. `[memory]`
- **Bridge returns `ok:true` for unknown tools** — verify via `tools/list`. `[memory]`
- **575 MB debug `fluxc`** — mind disk (C: full; use G:). `[memory]`
- **`fcx` ≠ the compiler** (it's a Slint UI transpiler); **`sigil-top 1.0.0-rc9` ≠ Flux's version**; **two caches** (Phase-1 wrapper vs Phase-3 object). `[repo]`
- **Money is propose-only** — never auto-execute; a human signs (2-of-2). `[memory]`

---

## 12. Making Flux easy to analyze (so the next agent gets it right first time)

The reason a fresh agent (or a fresh Codex/Claude session) misreads Flux is that it **reads `README.md` and version badges first, and those are stale landmines** (§6). The fix is two-pronged and ships as the **`flux-repo-onboarding/`** bundle (staged at `C:\Users\Viktor S. Kristensen\flux-repo-onboarding\`):

1. **Plant accurate, agent-facing orientation at the repo root:**
   - **`AGENTS.md`** — canonical, tool-agnostic orientation (Codex & most agents auto-load it). Leads with "**don't trust static strings — run `fluxc xray --json` / `fluxc status --json` / `flux_health_report`**," then the hybrid-compiler one-liner, the Phase 1/2/3 table, build rules, a "common mis-reads" list, and current caveats.
   - **`CLAUDE.md`** — a thin pointer to `AGENTS.md` (Claude Code auto-loads `CLAUDE.md`).
   - **`FLUX_FACTS.json`** — machine-readable canonical facts (version, crate count, build path, real-vs-stub, introspection commands) for programmatic analyzers.
2. **Remove the landmines** (`STALE-SIGNALS-PUNCHLIST.md`): unify version strings, replace the `mcp.rs:571` hardcode + README badges with build-time `Cargo.toml` reads, and correct the README "Honest status" line. Adding the guide matters as much as removing the drift.

**Codex P0 task (§9 #4):** commit `AGENTS.md`/`CLAUDE.md`/`FLUX_FACTS.json` to the repo root and apply the punch-list. The durable principle: **make Flux self-describe truthfully and point every reader at the live introspection**, because static docs will always drift.

Flux already has a **FIP (Flux Improvement Proposal)** process (`docs/flux-standards-v0.md`) — BIP/EIP-style, SAP-100% quality gate, ≥3 swarm-agent co-sponsors for "Accepted." Route the load-bearing identity decision (§8 item 7) and any version/standard change through a FIP.

---

## 13. References
- **Repo:** `epsilon:/home/storage/deepseek-codewhale/flux` · **GitHub:** `github.com/deme-plata/flux`
- **Compiler:** `crates/flux-frontend` (IR + parsers), `crates/flux-backend` (Cranelift), `crates/fluxc-core/src/phase3.rs` (pipeline)
- **Cockpit / MCP combos:** `C:\Users\Viktor S. Kristensen\flux-cockpit\` (`combo-bridge.mjs`, `src/main.js`, `index.html`)
- **Onboarding bundle:** `C:\Users\Viktor S. Kristensen\flux-repo-onboarding\` (`AGENTS.md`, `CLAUDE.md`, `FLUX_FACTS.json`, `STALE-SIGNALS-PUNCHLIST.md`)
- **Key in-repo docs:** `docs/FLUX_WHITEPAPER.tex`, `docs/FLUXC_CACHE_GAP.md`, `docs/FLUX_REV.md`, `docs/FLUX_FOUNDATION_PHILOSOPHY.md`, `docs/PLATFORM_STATUS_2026-06-06.md`, `docs/flux-standards-v0.md`, `CHANGELOG.md`, `README.md`

*Provenance: `[live]` facts read from epsilon + the running `fluxc` on 2026-06-24 (read-only); `[repo]` from source/config; `[memory]` from prior notes — re-verify `[memory]` before acting. This document is verified against an adversarial 4-lens audit of the standalone-compiler question (converging verdict: ~33% standalone — owns IR + Cranelift codegen, borrows rustc frontend + `cc` linker, not self-hosting, default path red).*
