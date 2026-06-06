# Flux Search — BLAKE3-Native Code Search

> **Crate:** `flux/crates/flux-search/`  
> **MCP wiring:** `fluxc-mcp` handlers in `handlers/ops.rs`  
> **Default index:** `target/flux-search/index.json`

Flux Search is a persisted TF-IDF search engine with PageRank, query intelligence (spell correction + synonyms), semantic fallback, SAP domain scoring, and MCP activity indexing. It indexes workspace source/docs and makes swarm tool calls searchable.

---

## Architecture

```
flux-search/
├── lib.rs          — SearchEngine, Document, SearchQuery, persistence
├── ranking.rs      — TF-IDF + composite ranking factors
├── pagerank.rs     — Link-graph PageRank
├── query_intel.rs  — Spell correction, synonym expansion, QueryPlan
├── semantic.rs     — SemanticIndex fallback embeddings
├── ml_ranker.rs    — LearningRanker signal blending
├── facets.rs       — Category/language aggregation
├── secret_scrape.rs — Redact secrets before indexing MCP args
├── mcp_tap.rs      — Tool-call / broadcast / settled-task → Document
└── updater.rs      — Index maintenance helpers
```

### Document model

```rust
Document { id, url, title, content, category, page_rank, content_hash, ... }
SearchResponse { results, total_results, corrected_query, query_time_ms, ... }
```

Index format: `SearchSnapshot` JSON (`version`, `documents`, `links`, `sap_domain_scores`).

### Indexed file types

`.rs`, `.md`, `.toml`, `.json`, `.ts`, `.tsx`, `.js`, `.jsx`, `.css`, `.html`, `.txt`

Max content per file: 200,000 chars (searchable excerpt).

---

## SearchEngine API

```rust
SearchEngine::new()
SearchEngine::load_or_new(path)
engine.index_path(dir, recursive) -> usize
engine.index_file(path)
engine.index_document(doc)
engine.search(SearchQuery { q, page, per_page, category, language })
engine.save_to_path(path)
engine.stats() -> SearchIndexStats
```

Features:

- Spell correction (`serach` → `search`)
- Synonym expansion
- Semantic fallback when TF-IDF returns thin results
- Result cache keyed by query
- Snippet generation with term highlighting

---

## MCP tools (`fluxc mcp`)

| Tool | Description |
|------|-------------|
| `flux_search` | Query persisted index |
| `flux_search_index` | Index a directory (recursive default) |
| `flux_search_status` | Index stats + path |
| `flux_search_combo` | Index (optional) + search in one call |
| `flux_aether_search_combo` | Index Vizily + Aether crates + search |
| `flux_vizily_import` | Import Vizily backend into index |

### Example MCP session

```json
{"method":"tools/call","params":{"name":"flux_search","arguments":{"query":"libp2p crawler","per_page":5}}}
```

### Aether search paths (default reindex)

When `reindex: true` on `flux_aether_search_combo`, indexes:

- Flux workspace crates (Aether-facing)
- Vizily backend (`/home/storage/migration/home-myuser/vizily/backend` unless `vizily_path` override)
- Custom `paths` array from arguments

---

## MCP activity → searchable docs (`mcp_tap.rs`)

Swarm activity becomes first-class search documents:

| Source | `doc_from_*` | Category |
|--------|--------------|----------|
| Tool calls | `doc_from_tool_call` | `tool` |
| Swarm broadcasts | `doc_from_broadcast` | `broadcast` |
| Settled tasks | `doc_from_settled` | `settled` |

Args pass through `redact_args()` — API keys and secrets become `[REDACTED]` before indexing.

Wiring target: subscribe to `/tmp/flux-events` or `fluxc-mcp` tap in `sigil-node` / `fluxc serve`.

---

## Shell combo gate

`tools/mcp-combos/flux_combo_fluxc_search.sh`:

1. SSH to Epsilon (`FLUX_EPSILON_HOST`, default `epsilon`)
2. Pipe JSON-RPC to `target/debug/fluxc mcp`
3. Probe `flux_search_status` + `flux_aether_search_combo` + typo probe
4. POST result to local Aether webhook

Guardrails: default read-only (loads existing index). Set `FLUX_SEARCH_REINDEX=1` to rebuild.

---

## Quick start (Epsilon)

```bash
cd /home/storage/deepseek-codewhale/flux
fluxc build --package flux-search

# Index workspace
fluxc mcp   # then tools/call flux_search_index with path=.

# Or one-shot combo
# flux_search_combo { "query": "distributed libp2p", "path": ".", "reindex": true }
```

---

## Tests

Built-in tests cover: tokenization, index+search, spell correction, snapshot roundtrip, snippet generation, MCP doc redaction.

— Documented from `flux-search` + `fluxc-mcp/ops.rs`, 2026-06-06.