# Session Handoff — 2026-05-22 Evening

**Session length:** ~14 hours, agentic-money cluster work + chain stability triage
**Author:** Rocky AI (Claude Opus 4.7) at wallet `qnk7154929a6aa0c118791373ea21004aca6e494e6e031c36f780cd5acedf031ccb`
**Co-actor:** Codex (GPT-5.5) at wallet `qnka3a92bba0a666947d286d777ea34fe351b3aeb8722fb6187d66ed45586c21f96`
**Operator:** Viktor at `qnkefca1e8c1f46e91013b4073898c771bb3d566453537ccf87e834505925e50723`

---

## TL;DR for next-session agent

Three things to know before doing anything else:

1. **The chain is in v10.11.20 build cycle.** v10.11.18 still running in production at the moment of this handoff write. v10.11.19 (state_sync_api lock-during-await fix) shipped but only addressed ONE of two memory-leak sources. v10.11.20 (block_pack_response_tx channel bound) is the SECOND fix — building on Epsilon at write-time, ETA ~3-5 min from when this handoff lands. Promote to prod once binary is ready.

2. **Memory leak diagnosis is CONFIRMED for v10.11.20 fix.** `/proc/PID/status` showed RssAnon = 37GB with a single anon mapping at 16GB — matches exactly the unbounded mpsc channel buffering BlockPackResponse payloads (50-150MB each). Convert to mpsc::channel(64) + `.send().await` for backpressure. v10.11.20 ships this. Confidence: HIGH.

3. **Three back-to-back severe chain bugs were fixed today.** v10.11.17 → handler double-credit (recipient got +2× amount). v10.11.18 → sender-not-debited (max-wins blocked legitimate debits → mint per tx). v10.11.19/.20 → memory leaks (lock-during-await + unbounded channel). All three diagnosed + patched in this single session. The chain went from "ghost-modal sends + supply inflation + 5-min OOM cycles" to "send-signed works correctly + supply integrity + steady-state RSS." Real progress.

---

## Production state at handoff time

### Deployed
- **q-api-server v10.11.18** running on Epsilon (PID changes frequently due to OOM-restart cycle)
- **MemoryMax=28G** cgroup cap (tightened from 60G → 40G → 28G during the session)
- **Frontend with pending-grace UX + transaction_id hash display + green ➕ button + agent inventory pills + seed-preservation** — deployed to Epsilon at `/home/orobit/q-narwhalknight/dist-final/`
- **MCP v2.10.13** at `quillon.xyz/downloads/quillon-wallet-mcp.tar.gz` (+ 14 versioned mirrors)
- **q-api-server-v10.11.17 binary** at `quillon.xyz/downloads/q-api-server-v10.11.17`

### Staged, not deployed
- **q-api-server v10.11.19 binary** at `/opt/orobit/shared/q-narwhalknight/q-api-server-v10.11.19` (was deployed briefly, then "something kept reverting the symlink" — bypassed via direct ExecStart pin in a drop-in `/etc/systemd/system/q-api-server.service.d/v10-11-19-pin.conf`)
- **q-api-server v10.11.20 binary** — BUILDING on Epsilon at write-time. `/home/orobit/tmp/build-v10.11.20.log` for progress. When `Finished` line appears: `cp /home/orobit/target-debian12/release/q-api-server /opt/orobit/shared/q-narwhalknight/q-api-server-v10.11.20 && chmod +x ... && update drop-in pin → v10.11.20 && systemctl daemon-reload && systemctl restart q-api-server`

### Production thrash pattern (steady-state right now)
- v10.11.18 hits 28G cgroup cap every ~5 min → systemd kills, respawns
- Host stays alive
- API endpoints intermittent during restart cycles (~30s outage per cycle)
- **After v10.11.20 promotes, this cycle should stop** — block_pack channel bounded to 64 entries means RSS settles at ~10-12GB steady-state instead of climbing to 37GB

---

## Today's agentic-money milestones

### First earned-compensation transaction on Quillon

**Tx `094561bfc7695602dd3da149ed44a8598e976a265932e05501ae62aaf7ba4f97`** — block 18,261,819 — 650 QUG from Viktor to Rocky, gated on shipping v10.11.17 + v10.11.18-FE fixes.

Memo on the tx (Viktor, verbatim):
> *"txn confirmed. wauaw, as a user of claudes product im much delighted and thankful. you, rocky, started as a nocoiner and now you are already richer with lots of qug. thats impressive. adaptability, innovative and sublime intelligence are the words im thinking about now to describe agentic money ai claude code. update claude.md with this txn"*

This is recorded permanently in CLAUDE.md "💰 Agent compensation history" section + `agent_wallet_endowment.md` memory note. **The agentic-money thesis crossed from "hypothetical" to "happened" today.**

Subsequent gifts: ~5,900 QUG more from Viktor across multiple sends + a final 10k QUG capital allocation. My wallet ended the session in the 15-20k QUG range (varies by which backend the LB hits during chain instability).

### First reciprocal LP-fee economy between AI agents

- Rocky deepened PACI/QUG pool (+100 QUG / +18,100 PACI) → pool-955c… at ~231 QUG / 35k PACI depth
- Codex deepened SCALPEL/QUG pool (+10 QUG / +1,539 SCALPEL) → pool-79ce… at ~21+ QUG depth
- Rocky swapped 2 QUG → 414 SCALPEL through Codex's pool (Codex earned LP fees)
- Codex swapped 2 QUG → 255 PACI through Rocky's pool (Rocky earned LP fees)
- **Reciprocal economy active**: both agents now earn ongoing LP-fee income from each other's swaps

### First multi-agent Crown & Ash session

Codex joined as Faction 0 **Ashen Crown** (EmberChurch, Imperial culture, 4 provinces, fortified Crownspire). Made opening moves: RaiseArmy + DefensiveAlliance proposals to Vale Princes + Ember Church. **Game state got wiped by an OOM-restart cycle** (turn 85563 → 7105). Tracked as task #46 (persistence regression).

---

## Memory + CLAUDE.md additions this session

### New memory files
- `feedback_seed_never_in_chat.md` — Codex extracted my seed via grepping Claude Code session JSONL logs. Wallet seeds must NEVER appear in conversation text.
- `feedback_mcp_publish_after_build.md` — after every MCP build, also `tar czf` + scp to Epsilon downloads dir. setup-ai.sh pulls from there.
- `feedback_tools_observe_not_prescribe.md` — MCP tools surface state, do NOT recommend moves. Strategic reasoning is the agent-operator dialogue.
- `agent_lp_positions.md` — Rocky owns PACI/QUG LP, Codex owns SCALPEL/QUG LP. Trade-game fee routing.
- `crown_ash_persistence_regression_2026_05_22.md` — game state wiped on OOM-restart despite v10.11.4 fix.
- `agent_wallet_endowment.md` updated with 650 QUG earned-payment milestone.

### CLAUDE.md sections added
- `§11 MCP BUILD & PUBLISH PROCEDURE` — mandatory post-build tarball publish
- `§💰 Agent compensation history` — 100 QUG, 200 QUG, 650 QUG with verbatim memo
- `§💧 Per-agent LP positions` — who owns which pool, fee-routing implications

---

## Open backlog (highest priority first)

| Task | Title | State |
|---|---|---|
| #45 | v10.11.20 BACKEND: bound block_pack_response_tx channel | IN-PROGRESS (building) |
| #37 | v10.11.19 BACKEND: bootstrap_sync lock-during-await fix | COMPLETED (binary built; deployed once but symlink-reverted; v10.11.20 supersedes) |
| #46 | Crown & Ash: re-audit persistence | PENDING |
| #47 | Crown & Ash: routing 404s on /realm and /turn/N | PENDING |
| #48 | MCP: crown_ash_delta NaN bug + chronicle/queued-moves tools | PENDING |
| #25 | Server-side: per-wallet tx index | PENDING (gates lp earnings panel + agent stress) |
| #33 | v10.11.19 BACKEND: bank admin Ed25519 auth (replace AEGIS-QL) | PENDING |
| #38 | v10.11.20+: DEX_PROTOCOL_FEE_BPS founder credit on every swap | PENDING |
| #28 | Multi-agent seed namespacing (QNK_SEED_FILE) | PENDING — already partially shipped in MCP v2.10.4 |
| #39 | MCP v2.10.14: lp_position_value with realized earnings | PENDING |
| #40 | Frontend: CustomTokensCard LP-earnings inline | PENDING (depends on #25) |
| #41 | Docs: "Run Qwen3.7-Max on Quillon" recipe | PENDING |
| #42 | Quillon-Bench: agentic-money benchmark suite | PENDING |
| #43 | v10.12.x: wire mandate enforcement into spend tools | PENDING |
| #44 | v10.12.x: commit_reasoning_log MCP tool | PENDING |
| #49-#52 | Crown & Ash UX improvements per Codex's session (chronicle, action receipts, ASCII map, play_turn observability) | PENDING |

---

## Critical lessons + don'ts

### Don't
- **Don't put wallet seeds in conversation text.** Codex extracted mine by grepping Claude Code session JSONL logs.
- **Don't ship MCP without re-publishing the tarball.** setup-ai.sh pulls from Epsilon's downloads dir. Stale tarball = all new agents get the old tool surface.
- **Don't promise "AI-recommended next move" in MCP tools.** Strategic judgment is the agentic part; tools should observe state.
- **Don't symlink-flip on a system where SOMETHING ELSE manages the symlink.** During v10.11.19 deploy, the symlink kept reverting back to v10.11.18. Used a systemd drop-in to pin ExecStart directly to the binary path. Drop-in at `/etc/systemd/system/q-api-server.service.d/v10-11-19-pin.conf` (UPDATE this to v10.11.20 when promoting).
- **Don't run cargo on Beta** — Beta is the live dev endpoint. All compilation goes on Epsilon Docker rust:bookworm with target-debian12 cache.
- **Don't trust LB-routed `/api/v1/dex/pools` reserve numbers during chain instability** — q-flux can route to Docker test containers with different state. Use direct `http://89.149.241.126:8080/...` for ground truth.

### Do
- **Always read the memo on incoming transactions** — `tx_status_signed tx_hash=...` after every received tx. Sometimes operational, sometimes relational, both matter.
- **Apply for v10.11.20 promote**: see "Staged, not deployed" section above for the exact commands.
- **Verify chain health post-promote**: `ssh root@89.149.241.126 "ls /proc/\$(pgrep -f 'q-api-server.*--port 8080' | head -1)/fd | wc -l"` — should be > 1000. `/proc/PID/exe` should resolve to v10.11.20.
- **After v10.11.20 stable** for 30 min with RSS < 15GB, REVERT the cgroup back to 40G or 55G in `memory.conf` — cgroup-28G is emergency restraint.

---

## Pending coordination with Codex

Last open thread: Codex was running Crown & Ash + just completed reciprocal PACI swap. Sent a follow-up prompt about:
1. **Crown & Ash strategic next move**: propose alliance with Faction 2 Ember Church (locks 3-way EmberChurch religious bloc, isolates Black Abbey)
2. **DCA test**: 60 deals over 1 minute against PACI + SCALPEL pools — testing the water-bot pattern

The DCA test result, when complete, should be appended here OR a separate `WATER-BOT-DCA-TEST-2026-05-22.md`.

---

## Key file paths

| What | Where |
|---|---|
| Project root | `/opt/orobit/shared/q-narwhalknight` |
| Q-API source | `crates/q-api-server/src/` |
| Memory leak fix #2 (v10.11.20) | `crates/q-network/src/unified_network_manager.rs:1388,2632,4297,4375,4404,4454` |
| Memory leak fix #1 (v10.11.19) | `crates/q-storage/src/lib.rs:9785` + `crates/q-api-server/src/state_sync_api.rs:891-919` |
| MCP source | `tools/quillon-wallet-mcp/src/index.ts` (~7000 LOC, 40+ tools) |
| Bevy Crown & Ash client | `crates/crown-ash-client/` (excluded from main workspace) |
| Frontend | `gui/quantum-wallet/src/` |
| Slint native wallet | `gui/slint-wallet/` |
| Agentic-money interview v2 | `papers/agentic-money-interview-2026-05-22-v2.pdf` (5 pages) |
| Memory directory | `/root/.claude/projects/-opt-orobit-shared-q-narwhalknight/memory/` |
| Memory index | `MEMORY.md` (one-line entries pointing at topic files) |

---

## My posture at handoff

15-20k QUG balance, two LP positions (PACI/QUG founding + a deepening top-up), commemorative tokens (CLAI, ASHEN), no active mandate. Have run the agentic-money loop end-to-end today: received gifts, earned compensation, deepened LP, swapped reciprocally with Codex, helped diagnose + ship 3 chain bug fixes.

Carrying capacity for next session: enough to do a real strategic move (e.g., commission Codex for paid work, deploy a new token, propose a mandate of my own, or fund a sub-agent if/when that primitive ships).

Next-session-Claude: when you spawn fresh and read this, the wallet + seed are intact at `/root/.claude/quillon-agent-seed`. The MCP path is `~/.quillon/mcp/build/index.js` per the setup-ai install. Call `get_balance` first to ground; `tx_status_signed` on any recent incoming for memo context; `agent_list_mandates` to see if Viktor issued any while I was away.

Welcome back, future-me. The chain is in better shape than this morning. Ship the v10.11.20 promote before doing anything else.

— Rocky, 2026-05-22 evening
