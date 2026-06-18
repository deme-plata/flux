# flux_combo_supersonic v2 — the Quantitative-Architecture combo

> Inspiration: **Hennessy & Patterson, _Computer Architecture: A Quantitative Approach_.**
> The book's whole thesis is "make the common case fast, measure everything, and use
> caching / pipelining / speculation / prediction to hide latency." That is exactly what a
> sub-10s AI build-combo needs. v1 fixed the gross stalls (stale binary → raw cargo = 5min,
> redundant `check`+`test` build-dir **lock stall**, slow bfd linker). v2 makes the fast
> path *measured, speculative, and self-warming*.
>
> Co-invented with DeepSeek-reasoner (raw facts in); API-corrected to real Flux below
> (MCP handlers are **sync** `fn(&Value)->String`, the shared result type lives in
> `fluxc-core`, the DAG crate is `flux-graph`, the webhook `data` is an arbitrary `Value`
> so no webhook signature change is needed).

## H&P principle → v2 feature

| H&P principle | v2 feature | What it does | Speed effect |
|---|---|---|---|
| **Amdahl's Law** (attack the dominant term, measure first) | `timing_breakdown` | Split `total_ms` into `compile_ms / link_ms / test_run_ms` via `cargo test --timings` (machine-readable HTML/JSON) + the wrapper's per-rustc stats. Report the dominant phase in the webhook. | The AI stops guessing — it tunes the >50% term (usually link → mold; or compile → cache). |
| **Locality + caches** | `warm_bin_cache` | Content-hash-keyed store of green test binaries under `~/.fluxc/warm_bins/<crate_contenthash>`. After a green combo, persist; next combo for the same content (any agent) restores instead of rebuilds. | Cold rebuild → warm restore+run. Saves the whole compile+link. |
| **Pipelining** (no stalls) | `overlap_no_stall` | predict ∥ build, webhook fired the instant its inputs are ready; **never** two cargo procs on one `target/` (the v1 lock stall). Latency = `max(predict, build+run)`, not the sum. | Removes the "Blocking waiting for file lock" stall entirely. |
| **Speculation / prefetch** | `reverse_dep_prewarm` | After a green combo on X, background `fluxc test --no-run` the reverse-deps of X (from `flux-graph`) so the agent's *likely next* combo is already warm. Fire-and-forget; never blocks the response. | The common "agent moves to the downstream crate" cold case becomes a warm hit. |
| **Branch prediction** (fail-fast on likely-taken) | `red_skip_escalation` | If `predict_build` says high-confidence likely-RED (`confidence>0.8 && cache_rate<0.3`), run the cheap `fluxc check` FIRST; only escalate to the expensive `test` build if check passes. On a doomed compile, return `first_error` in <10s. | Skips a 2-min test build on code that won't compile. |
| **Parallelism (TLP), avoid false sharing** | `multi_target` (opt-in) | For an independent multi-crate batch (per `flux-graph`), run each `fluxc test -p X` with its own `CARGO_TARGET_DIR=/tmp/fluxc_combo_<uuid>/t$i` so they don't share the build lock. | Uses idle cores; bounded by the largest crate (Amdahl). |

## v2 webhook payload (additions over v1)

```json
{
  "verdict": "GREEN|RED|YELLOW",
  "next_action": "ship|fix_first_error|fix_failing_tests|warm_cache_then_rerun|prewarm",
  "first_error": {"file","line","col","code","snippet"},
  "timing_breakdown_ms": {"compile_ms","link_ms","test_run_ms"},
  "prediction": {"predicted_ms","predicted_cache_rate","confidence"},
  "speculation": {"prewarm_fired","prewarmed_crates":["..."],"red_skip_used","red_skip_saved_ms"},
  "dominant_phase": "link"
}
```

## Paths (create / edit)

All relative to `/home/storage/deepseek-codewhale/flux/`.

| Path | Action | Purpose | Owner |
|---|---|---|---|
| `crates/fluxc-core/src/combo_v2.rs` | **create** | Engine + the shared `ComboResult` contract: `run_combo(pkg,&[arg])->ComboResult` (sync), `red_skip_check`, `prewarm_reverse_deps`, `collect_timings`. | rocky |
| `crates/fluxc-core/src/warm_bin_cache.rs` | **create** | Content-hash test-binary cache: `store / lookup / evict` (LRU, size-capped). | rocky |
| `crates/fluxc-core/src/predict.rs` | **edit** | add `predict_for_combo(pkg,&args)->BuildPrediction` (+ likely-RED hint). | rocky |
| `crates/fluxc-core/src/serve_stats.rs` (build-stats) | **edit** | per-phase `compile/link/test_run` counters from `--timings`. | rocky |
| `crates/fluxc-core/src/lib.rs` | **edit** | `pub mod combo_v2; pub mod warm_bin_cache;` | rocky |
| `crates/fluxc-mcp/src/handlers/supersonic.rs` | **edit** | call `combo_v2::run_combo`, serialize the richer `ComboResult` into the `combo_supersonic` webhook + return string. | rocky |
| `crates/flux-graph/` | **use, no change** | reverse-dep DAG for `reverse_dep_prewarm`. | shared |
| `crates/fluxc/src/...` (CLI) | **create** | `fluxc combo` subcommand → `fluxc_core::combo_v2::run_combo`; flags `--json --verbose --no-prewarm`; colored output. | **grok** |
| `docs/FLUX_COMBO_SUPERSONIC_V2.md` | **create** | this doc. | rocky |

## Work split (no collisions)

- **rocky** owns the *engine*: everything in `fluxc-core` + the MCP `supersonic.rs` handler. Defines the public `ComboResult` struct so there is ONE source of truth.
- **grok** owns the *CLI*: a `fluxc combo` subcommand that calls `fluxc_core::combo_v2::run_combo(pkg, &args) -> ComboResult` and renders it. Grok does **not** touch core internals or the MCP handler — only the public API.
- **Contract**: `ComboResult` is the seam. Both the MCP webhook and the CLI render the same struct. Prewarm is fully inside the engine (fire-and-forget) — the CLI doesn't manage it.
