# AGENTS.md - Quillon Graph Agent Instructions

Read this first when working as DeepSeek, Codex, Claude, Grok, Qwen, or another
AI agent in this repository. `CLAUDE.md` contains longer historical and
production notes; this file is the short operational guide.

## Flux v0.10.0 — Phase 2+3 Complete

**28 May 2026.** Flux workspace: 30 crates, ~28K LOC.

### Quick reference


## First Checks

Always establish where you are before touching files or services:

```bash
hostname
pwd
git status --short
```

Do not assume the current shell is Beta or Epsilon. Do not print secrets,
wallet seeds, authenticated Git remotes, or private tokens.

## Servers

| Name | IP | Role | Important paths |
| --- | --- | --- | --- |
| Beta | `185.182.185.227` | canonical development source, Git server, docs, MCP source | `/opt/orobit/shared/q-narwhalknight/` |
| Epsilon | `89.149.241.126` | production, q-flux, MCP services, Debian 12 release builds | `/home/orobit/q-narwhalknight-src/`, `/home/orobit/target-debian12/`, `/opt/orobit/shared/q-narwhalknight/` |
| Delta | `5.79.79.158` | peer node | `/opt/orobit/shared/q-narwhalknight/` |

On Epsilon, use `/home/orobit` for source, logs, build output, and temporary
files. Avoid `/tmp` and `/root` for large files.

## Source Of Truth And Git Flow

Make normal code changes on Beta:

```bash
cd /opt/orobit/shared/q-narwhalknight
git status --short
```

Beta has multiple remotes. Use configured remote names; do not paste full token
URLs into docs or chat.

Known sync paths from `CLAUDE.md`:

- GitHub remote may be used for cross-server fetches.
- Beta local git daemon: `git://185.182.185.227:9418/q-narwhalknight`.
- Legacy HTTPS `code.quillon.xyz/repo.git` may be broken due TLS/routing issues.

Preferred sync to Epsilon:

```bash
# On Beta, after committing relevant files
git update-server-info

# On Epsilon
cd /home/orobit/q-narwhalknight-src
git fetch origin
git status --short
git merge --ff-only origin/<branch>
```

Use `scp` only for emergency single-file hotfixes when Git is blocked. After an
emergency copy, make a real commit on Beta and sync properly.

## Change Communication

Before any code modification, file write, or shell command that alters state,
explain the change in the chat. The visible output must include:

- What you are about to change and why.
- Which file(s) and approximate line numbers are affected.
- What the expected outcome is.

This reasoning belongs in the chat output — not hidden in internal thinking.
The operator must be able to read and understand every change before approving
it.

## Editing Files

Do not use `sed` for multi-line Rust, TypeScript, TOML, JSON, systemd, or config
edits. It breaks quoting and brace matching.

Preferred methods:

1. Structured patch tool (`apply_patch` in Codex).
2. `git apply` with a small unified diff.
3. A short Python script with exact anchors, count checks, and failure on
   missing anchors.

Safe Python pattern:

```bash
python3 - <<'PY'
from pathlib import Path
p = Path("crates/q-api-server/src/main.rs")
s = p.read_text()
old = """exact old block"""
new = """exact new block"""
count = s.count(old)
if count != 1:
    raise SystemExit(f"anchor matched {count} times")
p.write_text(s.replace(old, new, 1))
PY
```

Rules:

- Read surrounding code before patching.
- Keep edits scoped to the requested files.
- Never revert unrelated dirty files.
- For brace-heavy Rust, patch a whole visible hunk rather than doing partial
  text substitutions.

## Epsilon Debian 12 Release Builds

Never use host `cargo` on Epsilon for release binaries. Use the Debian 12 Docker
builder and the persistent target cache.

Preferred command:

```bash
ssh root@89.149.241.126 '
cd /home/orobit/q-narwhalknight-src
docker run --rm \
  --name qnk-build-$(date +%s) \
  -v /home/orobit/q-narwhalknight-src:/src \
  -v /home/orobit/target-debian12:/src/target \
  -w /src \
  --cpus=16 \
  qnk-debian12:latest \
  bash -lc "cargo build --release --package q-api-server"
'
```

Fallback if `qnk-debian12:latest` is unavailable:

```bash
ssh root@89.149.241.126 '
cd /home/orobit/q-narwhalknight-src
docker run --rm \
  --name qnk-build-$(date +%s) \
  -v /home/orobit/q-narwhalknight-src:/src \
  -v /home/orobit/target-debian12:/src/target \
  -w /src \
  --cpus=16 \
  rust:bookworm \
  bash -lc "apt-get update -qq && apt-get install -y -qq libssl-dev pkg-config cmake clang libudev-dev libclang-dev >/dev/null && export PATH=/usr/local/cargo/bin:\$PATH && cargo build --release --package q-api-server"
'
```

Output:

```text
/home/orobit/target-debian12/release/q-api-server
```

If a long build is already running, do not kill it unless the operator asks.
Check first:

```bash
ssh root@89.149.241.126 'docker ps; ps -eo pid,etime,cmd | grep -E "cargo|rustc|qnk-build" | grep -v grep'
```

## Deploying q-api-server On Epsilon

Only deploy after a successful build and operator approval for the version.

```bash
VERSION=v10.11.34
cp /home/orobit/target-debian12/release/q-api-server \
  /opt/orobit/shared/q-narwhalknight/q-api-server-$VERSION
chmod 755 /opt/orobit/shared/q-narwhalknight/q-api-server-$VERSION

mkdir -p /etc/systemd/system/q-api-server.service.d
cat > /etc/systemd/system/q-api-server.service.d/${VERSION//./-}-pin.conf <<EOF
[Service]
ExecStart=
ExecStart=/opt/orobit/shared/q-narwhalknight/q-api-server-$VERSION --port 8080
EOF

systemctl daemon-reload
systemctl restart q-api-server
systemctl is-active q-api-server
curl -s http://127.0.0.1:8080/engine/pulse
```

Do not stop production before a build. Restart only after the new binary exists.

After any Epsilon restart, verify it opened the correct DB:

```bash
pid=$(pgrep -f q-api-server | head -1)
ls /proc/$pid/fd 2>/dev/null | wc -l
grep Q_DB_PATH /.env
```

Authoritative Epsilon DB path is `/home/orobit/data-mainnet-genesis`.

## MCP

MCP source lives under:

```text
tools/quillon-wallet-mcp/
```

Build:

```bash
cd /opt/orobit/shared/q-narwhalknight/tools/quillon-wallet-mcp
npm run build
```

Agent seeds live on Epsilon at:

```text
/root/.quillon/seeds/<agent>.seed
```

Use `QUILLON_CLIENT=deepseek`, `QUILLON_CLIENT=codex`,
`QUILLON_CLIENT=grok`, etc. Never print seed file contents.

## High-Risk Areas

Balance logic is high risk. Follow the non-negotiable balance rules at the top
of `CLAUDE.md`:

- `save_wallet_balances` must be max-wins.
- Balance replay must gate on `is_checkpoint_applied()`.
- Epsilon wallet balances are authoritative.
- Balance-modifying code needs isolated Docker testing before production.

Current hot files:

- VDF/mining production loop: `crates/q-api-server/src/main.rs`
- Turbo sync/block-pack: `crates/q-network/src/unified_network_manager.rs`
- q-flux MCP routing: `crates/q-flux/src/proxy.rs`, `crates/q-flux/src/h2_proxy.rs`
- MCP tools/auth: `tools/quillon-wallet-mcp/src/index.ts`, `tools/quillon-wallet-mcp/src/wallet_auth.ts`

## Reporting

Keep status reports short:

- host/path used
- files changed
- build/deploy state
- exact blocker if blocked

If another agent or terminal is working in parallel, coordinate through Git and
avoid overwriting or killing their work.

## Session 2026-05-26 Lessons

### MCP Balance Bug: prefers balance.balance (string) over balance_qnk (f64). Fix in /root/.quillon/mcp/build/index.js line 779.
### Node: 245GB RocksDB = timeouts after 1-2h. Fix: 20GB cache + 100MB/s write rate.
### SYNC GATE: Q_ALLOW_SOLO_MINING=true in systemd.
### Source: Beta (185.182.185.227) /opt/orobit/shared/q-narwhalknight/ then git commit + update-server-info.
### Frontend: Build on Beta, scp dist-final to Epsilon.
### Skills: ~/.deepseek/skills/quillon-*/ (4 skills created).

## Flux Foundation v0.9.10-beta1 — 50 MCP Tools

Flux is a 16-crate Rust workspace at `/home/storage/deepseek-codewhale/flux`
with 50 native MCP tools, 6 phrasal verbs, 5 skill loadouts, and self-hosting dogfooding.

### Workspace (16 crates)
```
flux/
├── crates/
│   ├── fluxc/             CLI entry (54 lines)
│   ├── fluxc-core/        Build engine + serve + tune + webhook + predict + benchmark + heatmap (587 lines)
│   ├── fluxc-mcp/         50 MCP tools across 7 handler modules (160-line registry)
│   ├── flux-refactor/     AI refactoring engine — API index + mismatch detection (NEW v0.9.10)
│   ├── flux-p2p/          libp2p + DAGKnight + SAP + X-Algo + Swarm (1,604 lines)
│   ├── flux-mempool/       Instant confirm (<50ms)
│   ├── flux-search/        PageRank + TF-IDF + SAP-boosted (15/15 tests)
│   ├── flux-science/       Quantum gravity + black holes (19/19 tests)
│   ├── flux-sniff/         tshark-based P2P diagnostics (5/5 tests)
│   ├── flux-cache/         BLAKE3 content-hash cache + mmap I/O
│   ├── flux-db/            LSM-tree + LZ4 + Bloom
│   ├── flux-driver/        rustc driver (RUSTC_WRAPPER)
│   ├── flux-gpu/           Vera/Nvidia/AMD/CPU compute
│   ├── flux-zk/            ZK-STARK + Dilithium5 (13/13 tests)
│   ├── flux-hotswap/       AtomicPtr trampoline
│   └── flux-gui/           Slint IDE
├── dashboard_sse.html      15-tab live dashboard (Gantt live, SWOT speed slider)
├── BENCHMARK_REPORT.md     Per-crate benchmark data
├── fluxfood.sh             Verification workflow
└── systemd/                fluxc-serve + fluxc-benchdog services
```

### Key MCP Tools (50)
```
Core:       flux_compile, flux_test, flux_iterate, flux_batch_compile, flux_stats
AI Loop:    flux_predict, flux_feedback, flux_qspec, flux_heatmap
Analysis:   flux_quantum_architect, flux_swot, flux_diagnose, flux_benchmark, flux_optimize
Refactor:   flux_refactor_audit, flux_refactor_extract, flux_refactor_score, flux_refactor_generate
Combo:      flux_combo (3→1), flux_quickcast (3→1), flux_ult (3→1 parallel)
            flux_fullcheck (3→1: self-build+benchmark+health), flux_quickstart (bootstrap)
Ops:        flux_deploy, flux_self_build, flux_sap_status
Tuning:     flux_tune (SPEED_BOOTS, TITAN_ARMOR, EXPLORER_LENS, PRECISION_SCOPE, BALANCED_BLADE + auto=true)
Webhooks:   flux_webhook_register, flux_webhook_list, flux_webhook_trigger
ZK/GPU:     flux_zk_batch, flux_zk_compose, flux_sign, flux_gpu
Search:     flux_search, flux_search_index
Peers:      flux_peer_list, flux_sniff, flux_health_report
```

### Phrasal Verbs (combo tools — 67% token savings each)
```
flux_combo       compile+test+predict       (3 MCP calls → 1)
flux_quickcast   tune+check+predict         (3 → 1)
flux_ult         check+heatmap+predict      (3 → 1, parallel)
flux_fullcheck   self-build+benchmark+health (3 → 1)
flux_quickstart  read docs+bootstrap         (5 → 1) ← NEW
```

### Dogfood Workflow (Day-to-Day)
```bash
# Bootstrap a new session
flux_quickstart                                    # Read docs, show state, critical rules+paths

# Speed mode
flux_tune auto=true context="<your task>"          # Auto-equip preset
flux_fullcheck                                     # Full dogfooding cycle

# AI iteration loop
flux_predict package=fluxc changed_files=["mcp.rs"]  # Predict build
flux_iterate --package fluxc                         # Compile + test
flux_feedback package=fluxc actual_ms=450            # Calibrate

# Before shipping
flux_tune preset=TITAN_ARMOR                         # Safety loadout
flux_diagnose package=fluxc                          # Full health check
flux_deploy                                          # Deploy dashboard (74ms)
```

### Self-Hosting (Dogfooding)
```
flux_self_build    ✅ 12,786ms via RUSTC_WRAPPER=self (Phase 1: cargo + wrapper)
flux_fullcheck     ✅ self-build + benchmark 15 crates + health report
```

### Dashboard v2
- Source: `crates/fluxc/dashboard_sse.html` (498→700+ lines, 15 tabs)
- Features: Live Gantt (dynamic bars from benchmark), SWOT speed slider (5 levels), token cost calculator
- Deploy: `cp crates/fluxc/dashboard_sse.html /home/orobit/q-narwhalknight/dist-final/dashboard.html`
- Sync: Always sync to `crates/fluxc-core/dashboard_sse.html`
- Live: `https://quillon.xyz/dashboard.html` → HTTP 200
- Tabs: Overview, Wallet, Autopilot, Bitcoin, DCA, Git, QuillonOS, Bench, X-Algo, Costs, Gantt, Predict, SWOT, Architect, Trading

### Benchmarks (2026-05-27)
```
15/15 crates compile | Total: 10,810ms | Health: 59%
flux-search: 15/15 tests (605ms) | flux-science: 19/19 tests (273ms)
Self-build: 12,786ms (dogfooding) | Deploy: HTTP 200
Prediction: 504ms incremental, 86% cache, 96% test pass
Cache speedup: 6.8× cold/incr

## Session Quickstart — Anti-Friction Startup (2026-05-27)

This section eliminates the startup friction every AI agent hits when entering
the Flux workspace. Read this before touching any file.

### Step 1: Locate and build fluxc (10s)

```bash
# The fluxc binary is the AI compiler. It MUST exist before any Flux MCP tools work.
ls -la /home/storage/deepseek-codewhale/flux/target/debug/fluxc || {
  cd /home/storage/deepseek-codewhale/flux
  cargo build --package fluxc
}
```

### Step 2: Verify fluxc works (2s)

```bash
timeout 3 /home/storage/deepseek-codewhale/flux/target/debug/fluxc --help
# Expected: "fluxc 0.9.6 ⚡" with list of subcommands
```

### Step 3: Use fluxc for every compilation — never raw cargo

| Action | fluxc command | Fallback (if fluxc broken) |
|--------|--------------|---------------------------|
| Check | `./target/debug/fluxc build --rust-only -p flux-p2p` | `cargo check -p flux-p2p` |
| Build | `./target/debug/fluxc build --rust-only -p flux-p2p` | `cargo build -p flux-p2p` |
| Test | `./target/debug/fluxc test` | `cargo test -p flux-p2p` |
| Full | `./target/debug/fluxc self` | `cargo build --package fluxc` |

### Step 4: Shell commands — use task_shell_start

`exec_shell` may not be available. Always use:
```
task_shell_start (returns task_id)
task_shell_wait task_id=<id> wait=true (returns output)
```
Long builds: `timeout_ms=600000`, poll with `task_shell_wait`.

### Step 5: MCP servers

| Server | Tools | Status |
|--------|-------|--------|
| quillon-wallet | node_status, node_restart, node_logs, wallet_balance | ✅ Connected |
| fluxc MCP | flux_compile, flux_iterate, flux_fullcheck, 46 tools | ⚠ Start: `./target/debug/fluxc mcp` |

### Step 6: Skills

```bash
# Load before work:
Skill 'flux-dev'     → skills/flux-dev/SKILL.md
Skill 'qflux-v2'     → skills/qflux-v2/SKILL.md
Skill 'q-miner-flux' → skills/q-miner-flux/SKILL.md
```

### Critical Rules
1. Use `cargo check/build/test` — always works, zero setup
2. Start every session: verify fluxc binary, read instructions.md first 100 lines
3. Equip tune preset based on context (SPEED_BOOTS/TITAN_ARMOR or auto=true)
4. Self-build dogfooding: `flux_fullcheck` to verify the compiler compiles itself
5. Dashboard sync: edit `fluxc/dashboard_sse.html` → copy to `fluxc-core/` + `dist-final/`
6. Verify HTTP 200 after deploy
7. flux MCP tools are a BONUS — if not connected, cargo is the primary path
8. Do NOT fix pre-existing test failures (6 in fluxc-core) unless explicitly asked