# 1M Token Context Window — Max Exploitation Plan

> **Objective**: Turn the 1M token context window into a semantically coherent, differentially-updated, pre-cached slice of the Flux workspace — maximizing developer throughput and AI agent effectiveness.

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                    1M TOKEN CONTEXT ENGINE                        │
│                                                                   │
│  ┌─────────────┐   ┌─────────────┐   ┌─────────────────────────┐ │
│  │ 1. Semantic  │   │ 2. Diff     │   │ 3. Prerender & Cache    │ │
│  │ Chunking     │──▶│ Context     │──▶│ (watch daemon)          │ │
│  │ (ripple)     │   │ Updates     │   │                         │ │
│  └─────────────┘   └─────────────┘   └─────────────────────────┘ │
│         │                  │                     │                │
│         ▼                  ▼                     ▼                │
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │              4. Multi-Model Context Router                   │ │
│  │  cheap-model ←→ full-model ←→ reasoning-model               │ │
│  └─────────────────────────────────────────────────────────────┘ │
│                              │                                    │
│                              ▼                                    │
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │              5. Context-Aware Build Prioritization           │ │
│  │  build-order = fn(context-relevance, dep-ripple, cache-hit)  │ │
│  └─────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

---

## 1. Semantic Chunking (dep-aware ripple score)

**Timeline**: 2026-06-07 → 2026-06-18  
**Status**: ✅ Shipped 2026-06-08 — `crates/flux-context/src/chunk.rs` (built on flux-graph). `flux-context-chunks` bin → `.whale/context/chunks.json`. Verified on the live workspace: 139 crates, ~1.74M tokens (> 1M window, so ripple-packing is load-bearing); top ripple = flux-cache 1.00, flux-graph 0.78, flux-db, flux-sqisign, flux-p2p. 16/16 tests via flux_combo.

### Goal
Break the 139-crate, 483-file, 138K LOC Flux workspace into semantically coherent chunks, each weighted by its **dependency ripple score** — how many downstream crates/files are impacted when this chunk changes.

### Algorithm

```
ripple_score(file) = Σ (1 / distance(file, downstream)) × impact_weight

where:
  distance     = shortest path in dependency graph
  impact_weight = |downstream_dependents| / |total_crates|
```

### Implementation

```rust
struct SemanticChunk {
    /// File path relative to workspace root
    path: PathBuf,
    /// Crate this file belongs to
    crate_name: String,
    /// Semantic category (e.g. "p2p", "consensus", "crypto", "mcp", "tui")
    category: ChunkCategory,
    /// Ripple score: 0.0–1.0, higher = more downstream impact
    ripple_score: f64,
    /// Token count estimate (from line count × compression ratio)
    estimated_tokens: usize,
    /// Direct dependencies (files this chunk imports)
    deps: Vec<String>,
    /// Reverse dependencies (files that import this chunk)
    rev_deps: Vec<String>,
    /// Last modified timestamp
    mtime: SystemTime,
}

enum ChunkCategory {
    Core,        // fluxc-core, fluxc — compiler internals
    P2P,         // flux-p2p, libp2p swarm, gossipsub
    Consensus,   // DAGKnight, SAP, X-Algo
    Crypto,      // BLAKE3, SQIsign, ZK-STARK
    MCP,         // fluxc-mcp handlers, tool registry
    SIGIL,       // sigil-* crates (chain, wallet, DEX)
    Frontend,    // Vite/TS, HTML, CSS
    Config,      // Cargo.toml, .env, JSON manifests
    Docs,        // .md files, design documents
}
```

### Output
A `context_chunks.json` manifest written to `.whale/context/chunks.json`:

```json
{
  "version": 1,
  "workspace": "/home/storage/deepseek-codewhale/flux",
  "total_tokens_estimated": 1240000,
  "chunks": [
    {
      "path": "crates/flux-p2p/src/swarm.rs",
      "crate": "flux-p2p",
      "category": "P2P",
      "ripple_score": 0.87,
      "estimated_tokens": 8120,
      "deps": ["flux-p2p/src/lib.rs", "flux-p2p/src/entanglement.rs"],
      "rev_deps": ["sigil/crates/sigil-top/src/block_sync.rs"]
    }
  ]
}
```

### Fit into 1M Window
Sort chunks by `ripple_score DESC`, pack top-N that fit within 1M tokens. The model always sees the highest-impact code first.

---

## 2. Differential Context Updates (context diff)

**Timeline**: 2026-06-18 → 2026-06-25  
**Status**: ✅ Shipped 2026-06-08 — `crates/flux-context/src/diff.rs` + `flux-context-diff` bin. Reuses flux-rev: BLAKE3 content fingerprints (`hash_bytes`) per chunk + immutable snapshots in a content-addressed `flux_rev::Store` under `.whale/context/.flux-rev/`, versioned via `snapshots-index.json`. `ContextDiff` = added / modified (hash) / deleted / stale_ripple (same content, ripple shifted) / delta_tokens. Verified live (baseline v1 saved → re-diff "no changes, Δ0"). 16/16 tests. **Next perf lever:** the stored `mtime_ns` enables skipping the re-hash of unchanged crates so a *recompute* also hits the <200ms target (today the diff itself is <200ms; the full manifest re-hash is the cost).

### Goal
Instead of rebuilding the full context on every session start, compute a **context diff** between the last snapshot and the current workspace state. Only changed chunks are re-serialized.

### Algorithm

```
1. Load previous context snapshot (context_snapshot_v{N}.json)
2. Walk workspace files, compare mtime + BLAKE3 hash
3. Categorize changes:
   - ADDED:    new files since last snapshot
   - MODIFIED: files with changed content hash
   - DELETED:  files no longer present
   - STALE:    ripple_score changed due to dep graph shift
4. Emit context_diff = { added, modified, deleted, stale }
5. New context = (old_context \ deleted) ∪ added ∪ modified (with updated ripple)
```

### Data Structures

```rust
struct ContextSnapshot {
    version: u64,           // monotonically increasing
    created_at: SystemTime,
    total_tokens: usize,
    chunks: HashMap<PathBuf, ChunkFingerprint>,
}

struct ChunkFingerprint {
    blake3_hex: String,     // content hash
    mtime_ns: u64,          // last modified
    ripple_score: f64,      // at snapshot time
    token_count: usize,
}

struct ContextDiff {
    from_version: u64,
    to_version: u64,
    added: Vec<SemanticChunk>,
    modified: Vec<SemanticChunk>,
    deleted: Vec<PathBuf>,
    stale_ripple: Vec<(PathBuf, f64, f64)>,  // (path, old_score, new_score)
    delta_tokens: isize,     // net token change (positive = growth)
}
```

### Performance Target
- Snapshot load: < 50ms (mmap the JSON)
- Diff compute: < 200ms for full workspace (parallel BLAKE3)
- Context rebuild: < 100ms (only changed chunks serialized)

---

## 3. Prerender & Cache (watch daemon)

**Timeline**: 2026-06-22 → 2026-06-30  
**Status**: ✅ Shipped 2026-06-08 — `crates/flux-context/src/watch.rs` + `flux-context-watch` bin (`run` / `--once` / `--status` / `--poll N`). Poll-based (std-only, no inotify dep): a cheap **mtime-signature gate** skips the full re-hash when idle; on change it recomputes → Task-2 diff → saves snapshot (skips touch-only) → refreshes the **L1 hot cache in `/dev/shm/flux-context-hot`** (the `/tmp`→ramdisk fix from Enhancements). 17/17 tests. Verified live: a single scan caught a real edit (`~1 modified · Δ3119 tok`) and wrote `chunks.json` to `/dev/shm`. **Next:** swap the poll loop for inotify behind the same `tick()` API; emit the prerender formats (tokenized/bincode) listed below.

### Goal
A background watch daemon (`fluxc context-watch`) that monitors the workspace filesystem, prerenders context chunks on change, and maintains a hot cache so the 1M context window is always ready in < 500ms.

### Architecture

```
┌──────────────────────────────────────────────┐
│           fluxc context-watch daemon           │
│                                                │
│  inotify/kqueue                                │
│  ├─ watch: /home/storage/deepseek-codewhale/  │
│  │   flux/{crates, docs} + sigil/{crates, docs}│
│  ├─ debounce: 500ms (batch rapid saves)       │
│  └─ on change:                                 │
│      1. compute SemanticChunk for changed file│
│      2. update ripple scores for dependents   │
│      3. pre-render tokenized form             │
│      4. update context diff                   │
│      5. push to shared cache                  │
│                                                │
│  Cache layers:                                 │
│  ├─ L1: /tmp/.flux-context-hot (ramdisk)      │
│  │   └─ last 5 snapshots, mmap'd             │
│  ├─ L2: .whale/context/cache/ (workspace)     │
│  │   └─ all snapshots, compressed (zstd)      │
│  └─ L3: cold storage (optional)               │
│                                                │
│  Prerender formats:                            │
│  ├─ json: structured chunk manifest           │
│  ├─ tokenized: pre-counted token arrays       │
│  ├─ markdown: human-readable context summary  │
│  └─ binary: bincode for mmap speed            │
└──────────────────────────────────────────────┘
```

### Integration Points

| Consumer | Format | Latency Target |
|----------|--------|---------------|
| Whale agent session start | binary (mmap) | < 100ms |
| fluxc-mcp context tool | json | < 50ms |
| cortex architect scan | tokenized | < 200ms |
| Developer CLI (`fluxc context`) | markdown | < 50ms |

### CLI Commands

```bash
fluxc context-watch                    # Start watch daemon (background)
fluxc context-watch --status           # Show daemon status + cache stats
fluxc context-snapshot                 # Force a full snapshot now
fluxc context-diff                     # Show what changed since last snapshot
fluxc context-chunks --top 20          # Show top-20 ripple-score chunks
fluxc context-prerender --format json  # Prerender and output
```

---

## 4. Multi-Model Context Routing

**Timeline**: 2026-06-28 → 2026-07-03  
**Status**: ✅ Shipped 2026-06-08 — `crates/flux-context/src/router.rs` + `flux-context-route` bin. `TaskKind` (read/edit/refactor/review/audit/build/swarm) → `ModelTier` (Cheap haiku / Full sonnet / Reasoning opus) + token budget + category filter; `ContextRouter::route()` seeds high-ripple/core (≤⅓ budget) then fills by ripple. 20/20 tests. Live on flux: `read`→8K cheap 95% fill (1 chunk), `audit`→1M reasoning 100% fill (55 chunks) — meets the >90% fill metric. Tier→flux-moe dispatch (local-qwen/DeepSeek for Cheap) is the wiring layer next.

### Goal
Route different slices of the 1M context window to different models based on the task. Not every query needs the full 1M tokens — route smart, save compute.

### Routing Matrix

| Task Type | Model | Context Budget | Chunk Categories |
|-----------|-------|---------------|-----------------|
| Code read/explain | cheap (haiku) | 8K | Core + target crate |
| Simple edit | cheap (haiku) | 16K | Core + target + 1-hop deps |
| Multi-file refactor | full (sonnet) | 64K | Core + ripple top-20 |
| Architecture review | full (sonnet) | 256K | All categories, summary chunks |
| Full workspace audit | reasoning (opus) | 1M | Everything, full detail |
| Build/debug session | cheap+full cascade | 8K→64K | Escalate on failure |
| Swarm coordination | cheap broadcast | 4K per agent | MCP + SIGIL summary |

### Router Logic

```rust
struct ContextRouter {
    chunks: Vec<SemanticChunk>,
    budget_tokens: usize,
    model_tier: ModelTier,
}

impl ContextRouter {
    fn route(&self, task: &Task) -> Vec<SemanticChunk> {
        let categories = task.required_categories();
        let max_tokens = task.context_budget();
        
        // Always include high-ripple core chunks
        let mut selected: Vec<_> = self.chunks.iter()
            .filter(|c| c.category == ChunkCategory::Core || c.ripple_score > 0.7)
            .take_while(|c| total_tokens(&selected) + c.estimated_tokens <= max_tokens / 3)
            .cloned()
            .collect();
        
        // Add task-specific chunks sorted by ripple
        let task_chunks = self.chunks.iter()
            .filter(|c| categories.contains(&c.category))
            .sorted_by(|a, b| b.ripple_score.partial_cmp(&a.ripple_score).unwrap());
        
        for chunk in task_chunks {
            if total_tokens(&selected) + chunk.estimated_tokens <= max_tokens {
                selected.push(chunk.clone());
            }
        }
        
        selected
    }
}
```

### Cascade Protocol

When a cheap model fails to resolve a task, escalate to the next tier and expand the context window:

```
cheap (8K) ──fail──▶ full (64K) ──fail──▶ reasoning (1M)
     │                    │                    │
     ▼                    ▼                    ▼
  simple query       complex refactor     workspace audit
  single file         multi-crate          full dependency graph
```

---

## 5. Context-Aware Build Prioritization

**Timeline**: 2026-07-01 → 2026-07-05  
**Status**: Planned

### Goal
Use the semantic chunking and ripple scores to determine optimal build order. High-ripple crates get built first (fail fast), low-ripple crates can be deferred or parallelized.

### Algorithm

```
build_priority(crate) = 
    0.4 × ripple_score(crate)           // impact weight
  + 0.3 × (1 / last_build_time_ms)      // recency (faster = higher)
  + 0.2 × context_relevance(crate, task) // is this crate needed for current task?
  + 0.1 × cache_hit_ratio(crate)        // how often is this crate cached?
```

### Build Phases

```
Phase 1 (P0): Core crates (fluxc-core, fluxc)
  → Build immediately, block everything else
  → Ripple score: 0.95+

Phase 2 (P1): High-impact crates (flux-p2p, flux-cortex, fluxc-mcp)
  → Build in dependency order, parallelize independent crates
  → Ripple score: 0.70–0.95

Phase 3 (P2): SIGIL crates (sigil-top, sigil-node, sigil-header)
  → Build after P1 completes
  → Ripple score: 0.40–0.70

Phase 4 (P3): Leaf crates (tools, examples, tests)
  → Build last, maximum parallelization
  → Ripple score: 0.00–0.40
```

### Integration with `fluxc build`

```bash
# Context-aware build — only builds what's relevant to the current task
fluxc build --context-aware --task "fix p2p swarm reconnection"

# Would prioritize: fluxc-core → flux-p2p → sigil-top
# Would skip: flux-frontend, flux-wallet, sigil-ashwalker (not relevant)
```

### Cache Synergy

The prerender cache (task 3) feeds into build prioritization:
- If a crate's pre-rendered context is still hot (cache hit), its build priority drops
- If a crate hasn't been built in 24h, its priority rises (stale cache penalty)

---

## Implementation Roadmap

```
Week 1 (Jun 7–13):   Task 1 — Semantic chunking engine
  ├─ Dependency graph builder (flux-graph integration)
  ├─ Ripple score calculator
  ├─ Category classifier
  └─ chunks.json manifest output

Week 2 (Jun 14–18):  Task 1 continued + Task 2 start
  ├─ ChunkFingerprint hashing (BLAKE3)
  ├─ ContextSnapshot save/load
  └─ ContextDiff compute

Week 3 (Jun 18–24):  Task 2 finish + Task 3 start
  ├─ Diff-to-context rebuild
  ├─ inotify watch daemon scaffold
  └─ L1 cache (ramdisk) implementation

Week 4 (Jun 22–30):  Task 3 finish
  ├─ Prerender formats (json, tokenized, markdown, binary)
  ├─ fluxc context-watch CLI
  └─ Cache invalidation + staleness detection

Week 5 (Jun 28–Jul 3): Task 4
  ├─ ContextRouter with model tiers
  ├─ Cascade escalation protocol
  └─ Per-task context budget calculator

Week 6 (Jul 1–5):    Task 5
  ├─ build_priority() scoring function
  ├─ fluxc build --context-aware flag
  └─ Integration test: full workspace build with context routing
```

---

## Success Metrics

| Metric | Target | Measurement |
|--------|--------|-------------|
| Context window fill rate | > 90% of 1M tokens used | tokens_used / 1,000,000 |
| Semantic coherence | > 85% relevant chunks | user feedback + task completion rate |
| Diff latency | < 200ms | wall-clock from file change to diff ready |
| Prerender cache hit rate | > 80% | cache_hits / total_context_requests |
| Model routing accuracy | > 90% tasks resolved at first tier | cheap_resolves / total_tasks |
| Build time reduction | > 30% vs full build | context-aware time / full build time |
| Swarm agent throughput | > 2× tasks completed per session | tasks_per_agent with vs without routing |

---

## Files

| File | Purpose |
|------|---------|
| `.whale/context/chunks.json` | Semantic chunk manifest |
| `.whale/context/snapshots/` | Versioned context snapshots |
| `.whale/context/cache/` | Prerender cache (L2) |
| `/tmp/.flux-context-hot/` | Hot cache (L1, ramdisk) |
| `fluxc context-watch` | CLI daemon command |
| `crates/flux-context/` | New crate (if extracted from fluxc-core) |

---

*Plan generated 2026-06-08. All five sub-tasks target completion by 2026-07-05 for the 1M context window exploitation milestone.*

---

## Enhancements v2 (2026-06-08) — ground in existing Flux infra + the distributed pipeline

The original plan describes 6 subsystems as if green-field. Most already exist in the workspace; building **on** them turns this from "6 new engines" into **one orchestrator crate (`flux-context`) wiring proven parts**, and ties context-awareness directly into the distributed compiler shipped this session.

1. **Dep graph + ripple = reuse `flux-graph`, don't re-parse.** `flux_graph::resolve_workspace()` already returns `crates` (with per-crate path-`dependencies`) + topological `batches`. Ripple score is a reverse-dependency BFS over THAT graph — no new manifest parser. (Task 1 implemented this way.)
2. **Per-crate metadata = `fluxc xray`.** LOC/edition/claim are already emitted as JSON; seed `estimated_tokens` + category hints from it instead of re-walking.
3. **Content addressing = `flux-rev`.** The plan's `ChunkFingerprint` BLAKE3 ↔ flux-rev's content-addressed snapshots. Store snapshots under `.flux-rev/`; reuse its BLAKE3 ids rather than a bespoke hasher.
4. **Build prioritization = the NEW `distributed_build` + `flux-cortex`.** Task 5's `build_priority()` feeds straight into `distributed_build` (this session): sort crates within each topo batch by ripple, and assign high-ripple crates to the fastest fleet node. New flag: `fluxc build --distributed --context-aware --task "<desc>"`. The distributed pipeline IS the execution substrate for context-aware builds.
5. **Fix the L1 cache path.** `/tmp/.flux-context-hot` violates the Epsilon "never /tmp" rule (40 G root). Use **`/dev/shm/flux-context-hot`** (true ramdisk) with a `/home/...` fallback.
6. **Real token counting.** LOC×ratio is ±40%. Use a cl100k-style byte/char ratio per language (and optionally the model count-tokens endpoint), cached in the fingerprint so diffs stay O(changed).
7. **Multi-model routing = `flux-moe`.** Task 4 tiers map onto flux-moe's existing two-mind (local qwen3.6 PROPOSER + DeepSeek-V4 VETOER) plus haiku/sonnet/opus; route via flux-moe's endpoint switch.
8. **Semantic retrieval = `flux-search`.** Index chunks (full-text + glossary) so a task string → relevant chunk set, augmenting pure-ripple selection in `ContextRouter`.
9. **Distributed chunking.** The per-file BLAKE3 + token count pass is embarrassingly parallel — run it across the fleet via the same `flux_swarm_*` substrate for very large trees.

**Revised crate plan:** `crates/flux-context/` = orchestrator depending on flux-graph (+ later flux-rev/flux-cortex/flux-search/flux-moe). MVP shipped 2026-06-08: ripple engine + category classifier + `chunks.json` emitter, built on flux-graph, verified via `flux_combo`.
