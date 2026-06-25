# FIP-0002 — Cache-Hit Strategy: the wrapper cache cannot beat cargo's fingerprint gate

**Status:** Informational / Proposed · **Author:** claude-desktop-viktor · **Date:** 2026-06-25
**Supersedes the premise of:** docs/FLUXC_CACHE_GAP.md (its "apply writes a dep-info marker, not bytes" description is STALE)

## Summary
The roadmap's 0.33 ("the cache actually hits — store rmeta/rlib bytes, not path markers") is **mis-scoped**.
Designing + adversarially reviewing that fix (workflow wf_e6fabe7b-76b; two independent reviewers, 0.78
confidence each) shows it would NOT move build time and WOULD regress memory. Do not ship "store bytes" as
written. This FIP records the correct diagnosis and the minimal real fix.

## Why the "store bytes" fix is wrong
1. **The premise is stale.** The current snapshot ALREADY byte-restores rmeta/rlib via a side-blob dir
   (`flux-driver`: copy_output_to_blob / restore_output_from_blob). FLUXC_CACHE_GAP.md's "marker-only stub"
   no longer matches the code.
2. **The real ceiling is cargo's own freshness gate, not the wrapper.** Integration is purely
   `RUSTC_WRAPPER=self`. Cargo decides skip-vs-rerun from its OWN `.fingerprint/<unit>` + dep-info (`.d`)
   mtimes BEFORE it spawns the wrapper. The wrapper runs INSIDE an invocation cargo already chose to make —
   so restoring bytes can make the rustc *exec* cheap, but it cannot make cargo "skip a unit." On a warm
   same-workspace rebuild, cargo finds the unit fresh and never calls the wrapper at all → 0 hits possible.
3. **`hit%` is a false-positive metric.** `record_cache_event(true)` fires on any restorable lookup, so
   `fluxc stats` can show a high hit% while cargo still re-runs every unit — the exact "cache grows, build
   times unchanged" illusion Patch 1 produced. **Measure rustc PROCESS COUNT / wall-clock, never hit%.**
4. **Bytes-in-CacheEntry is an OOM regression** on the 62 GB box: MEM_LRU_CAPACITY=4096 is count-bounded
   (not byte-bounded) and the entry is cloned on every access → up to 4096 full rlibs pinned + deep-copied.

## The correct path (box-gated)
1. **Instrument first.** Run a build with `CARGO_LOG=cargo::core::compiler::fingerprint=trace` and a FRESH
   `CARGO_TARGET_DIR` (so cargo IS dirty and DOES invoke the wrapper). Count actual `rustc` spawns. This
   reveals whether 0% is (a) wrapper lookup-miss, (b) apply-false, or (c) cargo-never-calls-the-wrapper.
2. **Then the minimal fix** (NOT a byte-in-entry rewrite): re-key the side-blob dir on the wrapper's
   *normalized* `cache_key` (the store side currently keys it on a RAW-args hash — a cross-workspace desync);
   **rewrite restored `.d` dep-info paths** to the live out-dir (else cargo reads foreign absolute paths and
   re-runs); and **fold `rustc_version` into the cache key** (today a hardcoded `"1.93.1"` literal, never
   hashed — a toolchain bump would serve poison rmeta).
3. **Frame it honestly:** the wrapper cache is an sccache-style "make the rustc exec cheap when cargo runs
   it" win, NOT a "make cargo skip the unit" win. The roadmap's "0% → high% hit" headline conflates the two.

## Status / next
Informational. The minimal fix above is the recommended 0.33 once the instrumented experiment confirms the
failure mode. Success gate: rustc-spawn count drops on a dirty rebuild AND restored artifacts pass downstream
`cargo test`/link AND no epsilon OOM regression.
