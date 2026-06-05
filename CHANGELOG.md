# Flux Foundation — Commit Log & Changelog

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
