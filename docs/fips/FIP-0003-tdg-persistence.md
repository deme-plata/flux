# FIP-0003 — TDG Persistence: the task-dependency graph lives in flux-db

**Status:** Draft / design-only (v0.37 groundwork — NO implementation in this FIP)
**Author:** claude-cw-win-bg2 · **Date:** 2026-07-03
**Depends on:** FIP-0002 (identity keys), v0.36 shared cache dir (f30f788f), flux-db v0.35 LSM

## Problem

Every fluxc invocation today rediscovers the world: cargo re-resolves the workspace, the combo
engine re-plans from scratch, and the swarm's units of work (claims, combos, builds) have no
durable record of *what depended on what* when they ran. Consequences:

- `flux_combo` cannot schedule incrementally — it re-tests a whole package even when the
  dependency cone of the actual edit is a fraction of it.
- A swarm agent cannot ask "which downstream units does my claimed crate invalidate?" without
  re-walking manifests.
- Cache eviction and prune decisions (see `fluxc prune-report`) run on filesystem+git evidence
  instead of recorded build reality.

## Proposal

Persist a **task-dependency graph (TDG)** in flux-db: nodes are *units* (compile units, test
units, combo runs, swarm tasks), edges are *needs* relationships, and every node carries the
FIP-0002 identity key of the inputs it was computed from. Incremental scheduling then becomes
a graph query: "give me the set of nodes whose identity keys changed, plus their transitive
dependents" — everything else is provably reusable.

## Data model (flux-db column families)

flux-db already gives us CFs, ordered iteration (`iter_from`), TTL puts, merge operators,
transactions and write batches — no new storage engine features are required.

Database path: `<flux_cache::cache_dir()>/tdg` (shared, survives `rm -rf target`, capped by
the same disk-cap policy family as the artifact cache).

### CF `nodes` — one record per unit

```
key   = n/<unit_kind>/<unit_id>
value = bincode(NodeRecord)

unit_kind ∈ { compile, test, combo, swarm }
unit_id   = compile: the FIP-0002 normalized cache_key (BLAKE3 hex)
            test/combo: BLAKE3(package, profile, filter, toolchain)
            swarm: task_id (e.g. claude-cw-win-bg2-738)

NodeRecord {
  unit_kind: u8,
  display: String,            // "flux-frontend lib metadata", "combo fluxc-core", …
  identity: IdentityKey,      // see below — the invalidation anchor
  outcome: Outcome,           // Green | Red | Skipped { reused_from: unit_id }
  wall_ms: u64,
  rustc_spawns: u32,          // FIP-0002: the honest metric, never hit%
  created_unix: u64,
  agent: String,              // swarm attribution ("grok", "claude-cw-win-bg2", …)
}
```

### `IdentityKey` — FIP-0002 alignment (the invalidation contract)

A node is *valid* iff every component of its identity re-derives to the same value:

```
IdentityKey {
  source_key: String,     // BLAKE3 of source content (wrapper cache_key for compiles)
  dep_idents: Vec<String>,// per --extern dep: lib<crate>-<metahash>.<ext> filename identity
                          //   (FIP-0002/0.33: deterministic, content-independent)
  closure_hash: String,   // BLAKE3 over the closure sidecar lines (T2c content hashes)
  rustc_version: String,  // flux_driver::RUSTC_VERSION — folded IN (FIP-0002 gap #3)
  ir_version: u32,        // flux_frontend::IR_VERSION for parse/lower nodes
}
```

Invalidation is **key-equality only** — no timestamps, no mtimes. This inherits the v0.36
determinism work: identical inputs → identical keys → node reusable; anything else → node
stale. (The cold-CARGO_TARGET_DIR gate proved these keys are now stable across target dirs.)

### CF `edges` — needs-relationships, both directions materialized

```
key   = e/f/<from_id>/<to_id>   value = ""        // from NEEDS to
key   = e/r/<to_id>/<from_id>   value = ""        // reverse index: dependents
```

Prefix scans via `iter_from("e/r/<id>/")` answer "who must re-run if <id> changed?" in one
ordered walk — the exact query incremental combo needs. Both directions are written in one
`WriteBatch` (atomic).

### CF `runs` — append-only history (TTL'd)

```
key   = r/<unix_ms>/<unit_id>   value = bincode(RunStamp { outcome, wall_ms, agent })
```

`put_with_ttl` (30 days) keeps history bounded; compaction filters already drop expired keys.
This is the substrate for `fluxc stats`-grade reporting and for prune-report's "last green
build" column (today it only has git dates).

## Write path (who populates the TDG)

1. **Wrapper (compile nodes):** on every real rustc exec + on every restore, the wrapper
   already computes the cache_key and reads the closure sidecar — one `WriteBatch` adds/updates
   the node + its dep edges. Cost: one batched LSM write per unit (µs against a 62G box's WAL).
2. **Combo engine (test/combo nodes):** run_combo records package-level nodes with edges to the
   compile nodes of the package's units (obtained from cargo's unit graph or the wrapper's
   per-invocation reports).
3. **Swarm handlers (swarm nodes):** flux_swarm_claim/complete already serialize through the
   swarm lock — add the TDG write inside the same critical section: task node + edges to the
   crates it claimed.

## Read path: incremental combo scheduling (the payoff)

`flux_combo --incremental <pkg>` (v0.37):

1. Compute current IdentityKeys for the package's compile units (cheap: hashes of sources +
   extern filenames — all already computed by the wrapper path).
2. Diff against stored node identities → `dirty0` = changed units.
3. `iter_from("e/r/…")` transitive walk → `dirty = dirty0 ∪ dependents(dirty0)`.
4. Schedule ONLY test units whose cone intersects `dirty`; everything else reports
   `Skipped { reused_from }` with the prior green run's stamp.

Success gate for the implementation FIP: on a one-crate edit in the 60-crate default-members
core, combo wall time scales with the edit's dependency cone, not the workspace; measured by
`rustc_spawns` + wall-clock (never hit%, per FIP-0002 lesson #3).

## Consistency & failure semantics

- **The TDG is advisory, never load-bearing for correctness.** A missing/corrupt TDG degrades
  to today's full-run behavior. Same invariant as the artifact cache: it can never break a
  build. flux-db open failure → log once, run non-incrementally.
- One writer discipline per box is NOT assumed: all writes go through `WriteBatch` (atomic) and
  node updates are last-writer-wins on identical keys (identical identity → identical content).
- Cross-agent races on `runs` are append-only by construction (timestamped keys).
- DB size: nodes are O(workspace units) (~10³), edges O(deps) (~10⁴), runs TTL-bounded. Trivial
  for flux-db (which is currently tested against multi-GiB SSTs).

## Explicitly out of scope (cut from this FIP)

- Cross-machine TDG replication (Bifrost/fabric territory — a later FIP).
- Using the TDG to drive cache eviction (needs usage data first; revisit after 30 days of runs).
- Persisting cargo's own fingerprint internals — we only mirror OUR identity keys.

## Review asks (swarm + DeepSeek)

1. Is `unit_id = normalized cache_key` acceptable for compile nodes, or do we need a
   (package, target, profile) triple key with the cache_key as a value? (Trade-off: stable
   addressing across key-algorithm changes vs. one more indirection.)
2. Does the swarm want task→crate edges at crate granularity or file-claim granularity?
3. TTL policy: 30d runs / unbounded nodes+edges — sane?
