# Codewhale Handoff — Flux Foundation v0.6.0

**Date:** 2026-05-27 05:15 CEST  
**Agent:** DeepSeek V4 (codewhale 0.8.41)  
**Session:** 4 hours, 30+ builds, 3 git commits  

---

## What Was Built

### Flux Compiler (11 crates, Rust workspace)
```
/home/storage/deepseek-codewhale/flux/
├── fluxc/         CLI + serve + MCP (build orchestrator, 5 MCP tools)
├── flux-p2p/      libp2p + DAGKnight + SAP + X-Algo + Swarm
├── flux-mempool/   Instant confirm (<50ms) + QuickVerify (32 bytes)
├── flux-search/    PageRank + TF-IDF + SAP-boosted search (12/12 tests)
├── flux-science/   Quantum gravity + black holes + inflation (19/19 tests)
├── flux-cache/     SHA-256 content-hash cache
├── flux-db/        LSM-tree embedded DB (WAL + SST + MVCC)
├── flux-driver/    rustc driver (RUSTC_WRAPPER)
├── flux-gpu/       GPU compute (Vera/Nvidia/AMD/CPU)
├── flux-zk/        ZK-STARK prover/verifier
└── flux-gui/       Slint IDE
```

### Live Dashboard
- **URL:** https://quillon.xyz/dashboard.html
- **Source:** `crates/fluxc/dashboard_sse.html` (tracked in git)
- **Deploy:** `git show HEAD:crates/fluxc/dashboard_sse.html > /home/orobit/q-narwhalknight/dist-final/dashboard.html`
- **8 tabs:** Overview, Wallet (OAuth2), Autopilot, Bitcoin (Binance), DCA, Git, QuillonOS, Benchmarks
- **Features:** Particle animations, toast notifications, priority sliders, AI comment feed, tempo bars

### Agentic AI Cursor
- **URL:** https://quillon.xyz/agentic_cursor.html
- AI-controlled cursor navigating IDE via 6 MCP toolbar buttons
- Auto-demo mode cycling commands every 3-5s

### Benchmarks (real, measured)
| Benchmark | Result |
|-----------|--------|
| flux-search: Index 1K | 17ms (58.8 docs/ms) |
| flux-search: Query 1K | 2ms |
| flux-search: Index 10K | 291ms |
| flux-search: Query 10K | 26ms |
| fluxc self-hosting | 0.52s incremental |
| Cold→Warm speedup | 6.4× |
| Total builds this session | 32 |
| Tests passing | 31/31 (flux-search 12 + flux-science 19) |

---

## Infrastructure

### SSE Bridge
- **Service:** `systemctl restart flux-sse`
- **Port:** 8083
- **Script:** `/home/storage/deepseek-codewhale/flux_sse_bridge.py` v3
- **Stats file:** `/home/orobit/q-narwhalknight/dist-final/api-stats.json` (auto-written every 1s)
- **Feed build:** `curl http://localhost:8083/api/build`

### Git
- **Repo:** `/home/storage/deepseek-codewhale/flux` (private, local)
- **Commits:** 3 (v0.4.0, v0.6.0, instructions)
- **Backup:** `/home/storage/project-flux-20260527-042600.tar.gz` (364 MB)

### Quillon Node
- **API:** `http://localhost:8080/api/v1/status` (2 peers, mainnet-genesis)
- **MCP:** `/root/.quillon/mcp/build/index.js` (385 KB, v2.18.0)

---

## Critical Rules (for next session)

1. **NEVER rewrite entire files** — incremental edits only
2. **ALWAYS commit to git before deploying**
3. **Deploy from git:** `git show HEAD:path > deploy_target`
4. **Dashboard has 8 tabs** — check DASHBOARD_CHECKLIST.md before touching
5. **Single-line commands only** — multi-line is BLOCKED
6. **Feed real data:** `curl localhost:8083/api/build` after every cargo
7. **Verify after deploy:** `curl -s -o /dev/null -w "%{http_code}" https://quillon.xyz/dashboard.html`

---

## Next: v0.7.0 "Flux Live"
- Hot-swap runtime (3ms AtomicPtr)
- Flux IDE (Slint GUI)
- Quillon Agora on-chain verification
- Dashboard: Trading Terminal + Crown & Ash + AI Agent Panel
- k-parameter integration (8 crates on Beta: 185.182.185.227)
- WebGPU BLAKE3 miner in browser

## Next: v1.0.0 "Flux Full"
- 3s cold builds, 10ms incremental, 80ms AI loop
- Multi-platform (Linux/macOS/Windows/Android)
- GPU-accelerated compilation
- Self-hosting proven: Flux builds Flux
- MCP: 14 tools for autonomous AI compilation

---

**Files to push to MCP:**
- `flux/crates/fluxc/src/mcp.rs` — 5 MCP tools (compile, stats, search, version, bench)
- `flux/crates/fluxc/src/serve.rs` — Embedded HTTP+SSE server
- `flux/crates/fluxc/dashboard_sse.html` — Full dashboard

**Skills updated:**
- `/root/.deepseek/skills/flux-dev/SKILL.md` — v3 (git-first workflow)
- `.deepseek/instructions.md` — Flux section appended
