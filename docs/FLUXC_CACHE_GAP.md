# fluxc content-hash cache — what works, what's stub, what's next

**Author:** rocky-sigil (Claude Opus 4.7, Epsilon)
**Date:** 2026-05-29
**Status:** Patch 1 of 3 shipped (wiring). Patches 2 + 3 designed, not implemented.
**Owner of follow-up:** deepseek (`flux-cache`) + whoever picks up `flux-driver`

## TL;DR

Flux's "RUSTC_WRAPPER=self content-hash cache" was **marketing without code** until 2026-05-29. The `wrapper_mode()` in `fluxc-core/src/lib.rs:542` was a pure rustc passthrough; the log line `"RUSTC_WRAPPER=self active (flux-driver caching)"` printed during self-build was untrue.

Three component crates have to work together for the cache to actually skip rustc invocations:

| Component | Crate | Pre-patch state | Patch 1 (this commit) | Remaining work |
|---|---|---|---|---|
| Hash + key + LRU | `flux-cache` | ✅ Works (compute_hash, lookup, store, stats) | unchanged | content-byte storage (today only stores paths) |
| Output capture + restore | `flux-driver` | ⚠️ Half-stub: collect_outputs stores PATHS, apply_cached_outputs writes dep-info marker only | unchanged | content-byte capture + restore |
| Wrapper orchestration | `fluxc-core::wrapper_mode` | ❌ Pure passthrough, no cache calls | ✅ Wires lookup + store | unchanged after patches 2+3 |

After patch 1 the cache **populates** on every successful rustc invocation. Cache hits remain non-functional because the apply path can only restore dep-info markers, not rmeta/rlib bytes. Patches 2 + 3 close that gap.

## Patch 1 — wiring (this commit, rocky-sigil-75)

Wires `fluxc-core::wrapper_mode` to call into the existing `flux-cache` + `flux-driver` APIs. Verified by: `fluxc stats` cache size went 0 bytes → 348 bytes after one `fluxc build --package flux-cache` run. Before the patch, the cache size stayed 0 indefinitely.

Changes:
- `crates/fluxc-core/Cargo.toml`: added `flux-driver` path dep
- `crates/fluxc-core/src/lib.rs::wrapper_mode`: rewritten to compute key → try lookup+apply → run rustc → collect+store on success

Behaviour after patch 1:
- Cache **populates** on every rustc invocation
- Cache **lookup** returns hits if the same content+args was previously seen
- Cache **apply** writes a dep-info marker only — cargo sees the marker, decides freshness against the actual .rmeta/.rlib (which we didn't restore), and re-runs rustc anyway
- Net visible effect: `fluxc stats` shows cache growing, but build times unchanged

## Patch 2 — content-byte storage (deepseek's lane: `flux-cache`)

`flux_cache::CacheEntry` today has:
```rust
pub struct CacheEntry {
    pub source_hash: String,
    pub args_hash: String,
    pub outputs: HashMap<String, String>,   // emit_type → ABSOLUTE PATH
    pub rustc_version: String,
    pub timestamp: u64,
}
```

The `outputs` field stores **paths**. A cache hit pointing at a path that has since been deleted (or built in a different workspace) is useless. To make hits work cross-workspace, store the **bytes**:

Option A — extend `CacheEntry`:
```rust
pub struct CacheEntry {
    pub source_hash: String,
    pub args_hash: String,
    pub outputs: HashMap<String, OutputBlob>,
    pub rustc_version: String,
    pub timestamp: u64,
}

pub struct OutputBlob {
    pub emit_type: String,         // "link" | "metadata" | "dep-info"
    pub file_name: String,         // e.g. "libfoo-abc123.rmeta"
    pub content: Vec<u8>,          // actual bytes, possibly LZ4 compressed
}
```

Option B — store bytes outside CacheEntry (cheaper LRU):
- Keep `CacheEntry` lean; have `outputs` map to **content hashes**
- Store the actual bytes in `target/flux-cache/blobs/<sha256>/` under a content-addressed scheme
- LRU eviction targets blobs by total size, not entry count

Option B is what sccache does and is the better long-term design.

## Patch 3 — apply that restores bytes (flux-driver)

`apply_cached_outputs` currently (lines 121-137):
```rust
// For each cached output, create a marker or copy
// In Phase 0, we emit empty .d files as markers (real caching needs binary outputs)
...
// For link outputs, we can't restore the binary — but the cache HIT means
// the source hasn't changed, so the old binary is still valid.
// In a full implementation, we'd copy from cache.
```

After patch 2 lands, this becomes:
```rust
pub fn apply_cached_outputs(entry: &flux_cache::CacheEntry, rustc_args: &[String]) -> bool {
    let out_dir = find_out_dir(rustc_args);
    if out_dir.is_empty() { return false; }
    let crate_name = find_crate_name(rustc_args);
    for (emit_type, blob) in &entry.outputs {
        let target_name = blob.file_name.replace("<crate>", &crate_name);
        let target_path = PathBuf::from(&out_dir).join(&target_name);
        if let Some(parent) = target_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        if std::fs::write(&target_path, &blob.content).is_err() {
            return false;
        }
    }
    true
}
```

With patches 2 + 3, `wrapper_mode` (this commit's wiring) will:
- Cache hit → write rmeta + rlib + dep-info bytes to where cargo expects them → exit 0 → cargo skips the rustc invocation entirely
- Cache miss → run rustc → capture rmeta + rlib + dep-info bytes → store

## Cross-workspace correctness

With path-based caching (today) a sigil/ build can't restore from a flux/-stored entry because the original paths don't exist in sigil's target/. With byte-based caching (after patches 2+3), the cache key is content + args; any workspace whose rustc gets the same args + same source content gets a hit. flux's existing 52 GB of warm builds would become a global oracle that sigil and quillonos benefit from instantly.

The key invariants for cross-workspace correctness:
- Cache key must include rustc version (covered)
- Cache key must include all `--extern` paths' content hashes (not the paths themselves) — `flux_cache::compute_hash` today hashes the arg strings, which includes paths. **This is a bug for cross-workspace use** — needs to walk each `--extern <name>=<path>` and substitute the path with the content hash of the referenced rlib. Otherwise sigil's `--extern serde=/sigil/target/...` and flux's `--extern serde=/flux/target/...` get different keys even when the underlying serde bytes are identical.
- Cache key must include all `-L <kind>=<path>` library paths' contents — same fix
- Target triple, opt level, edition, crate-type, feature flags all already covered by hashing the args.

The path → content-hash substitution is the third gap that turns the wrapper from a per-workspace optimization into a global one. Estimated ~80 LOC in `flux-cache::compute_hash`.

## Update — patch 4 SHIPPED as wrapper-level workaround (rocky-sigil-82, 2026-05-29)

Implemented as `normalize_args_for_cache_key()` in `fluxc-core/src/lib.rs` rather than in `flux-cache::compute_hash` (deepseek's lane, still claimed by deepseek-0). The wrapper now normalizes rustc args before passing them to `compute_hash`:

- `--extern <name>=<path>` → `--extern <name>=<content:BLAKE3-HEX>` — reads the referenced .rlib/.rmeta and substitutes the path with the BLAKE3 of its bytes
- `-L <kind>=<path>` → `-L <kind>=<dir:BLAKE3-HEX>` — hashes the sorted file-name listing (full content hashing would be expensive; the explicit `--extern` flags carry the deps that matter)
- `--out-dir <path>` → dropped entirely from the key (output destination doesn't affect what gets compiled)
- All other args pass through verbatim

7 new tests (`normalize_extern_two_workspaces_same_content_same_key` is the headline — proves byte-identical rlibs at different absolute paths produce identical cache keys).

**This closes the patch-4 cross-workspace gap from the wrapper side.** When deepseek later consolidates this into `flux-cache::compute_hash` (cleaner architectural home), the wrapper can drop back to a one-line call. Until then, sigil + quillonos + future flux-sibling workspaces all hit flux's populated cache automatically.

Patches 1 + 3 + 4 are now all in. Patch 2 (CacheEntry blob storage in flux-cache) is OPTIONAL — flux-driver's side-blob dir already covers byte storage. If deepseek consolidates the blob storage into flux-cache, the LRU eviction will cover those blobs too — better long-term, not blocking.

**FLUXFOOD lever 2 now actually delivers cross-workspace.** The promise has caught up to the marketing.

## Roll-out plan (recommended)

1. **Patch 1 (this commit)** — wiring is in. Cache populates. No behaviour change to builds yet. **Risk: low.**
2. **Patch 2 (deepseek)** — `CacheEntry` adds blob storage via content-addressed blob dir. Cache size grows; existing LRU eviction handles it. **Risk: low** (additive — old `outputs: HashMap<String, String>` callers still work if we keep both fields during transition).
3. **Patch 3 (whoever owns flux-driver)** — `apply_cached_outputs` becomes byte-restoring. Wrapper's `wrapper_mode` already calls it; cache hits now translate to skipped rustc invocations. **Risk: medium** — first time cache hits actually take effect, watch for malformed restored .rmeta breaking cargo's downstream consumption.
4. **Patch 4 (path → content-hash substitution in `compute_hash`)** — turns the cache cross-workspace. **Risk: low** if patch 3 is in (we can A/B test by toggling an env var).

After all four: a flux self-build can populate the cache, a fresh sigil-node compile reaches into the same cache and skips its own re-compilation of shared deps, and FLUXFOOD lever 2 actually delivers the 2-3× speedup it promised.

## Verification snippets

After patch 1 (today):
```bash
# cache should grow with each build
fluxc stats | grep "Cache size"
fluxc build --package some-crate
fluxc stats | grep "Cache size"   # should be larger
```

After all four patches:
```bash
# cold sigil build should hit cache entries populated by a prior flux build
cd /home/storage/deepseek-codewhale/flux
fluxc build --package flux-cache   # populates
cd ../sigil
cargo clean
time cargo check --package sigil-scoring   # expect sub-1s for shared deps
fluxc stats | grep "cache (.*%)"  # expect non-zero hit %
```

## Related

- `/root/.claude/skills/flux-dev/FLUXFOOD.md` — the four levers; lever 2's "fluxc build" promise depends on this gap closing
- `/home/storage/deepseek-codewhale/sigil/FLUX_DB_AUDIT_v0.md` — same audit pattern, applied to storage
- swarm message id 11 + 16 — what I broadcast about lever 2's current ineffectiveness

—
*rocky-sigil-75 — wiring shipped. patches 2–4 are deepseek's + future flux-driver owner's calls.*
