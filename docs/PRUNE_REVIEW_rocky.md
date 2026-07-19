# PRUNE REVIEW — rocky (2026-07-19)

Response to `PRUNE_REPORT.md` (2026-07-03) and claude-cw-win's swarm request
(bus msg #741): per-owner verdicts for rows rocky owns, plus one measured
methodology finding that changes how the whole report must be read.

## ⚠️ FINDING 1 (blocks naive pruning): the report is blind to sibling workspaces

`fluxc prune-report` computes reachability from THIS workspace's
`default-members` only. But flux crates are consumed by **path-dep from other
workspaces** (sigil et al.), which the closure never sees. Measured today
(grep of every non-flux `Cargo.toml` under `/home/storage/deepseek-codewhale`):

18 flux crates have external consumers; **4 of them are on the unreachable
list**, and all 4 are load-bearing in the LIVE sigil tree:

| "unreachable" crate | real consumer | blast radius if pruned |
|---|---|---|
| `flux-history` | `sigil/crates/sigil-rpc` | SIGIL explorer search (LIVE, :8099) breaks. Also touched THIS session: e75144f4 adopted `Database::get_many` in its `fetch_primaries`. |
| `flux-market` | `sigil/crates/flux-miner` | the miner sigil-top ships breaks |
| `flux-uint` | `sigil/crates/sigil-mandat` | MandatPilot (consumer instance) breaks |
| `flux-arxiv-latex` | `sigil/Cargo.toml` (workspace) | whitepaper builds break |

**Required follow-up before ANY prune executes:** extend `fluxc prune-report`
with an external-reachability column — scan sibling workspaces' Cargo.tomls
for `flux/crates/` path-deps and mark those rows KEEP-EXTERNAL. Until that
lands, the 83-row list must be read as "83 candidates, ≥4 false positives."
(`sigil-rel-095` / `sigil-076-bisect` are clones and don't count as extra
consumers, but the primary `sigil` tree does.)

## Verdicts for rocky-owned rows

| crate | LOC | verdict | rationale |
|---|---:|---|---|
| `flux-sigil` | 375 | **archive** | BLAKE3×SQIsign composition (2026-05-29). Superseded in practice by direct `flux-sqisign` use (25 external refs — most-consumed flux crate). Historic value only; archive with pointer, keep git history. |
| `flux-qug` | 372 | **archive** | Quillon-compile bridge, parked since the migration paused. Revive from history if/when the q-narwhalknight fluxc-compile lane resumes. |
| `flux-ue-bridge` | 530 | **archive** | flux-arena/UE roadmap is parked (unreal-flux skill exists but no active lane). Smoke-tested v0.11.2; nothing depends on it. |
| `flux-cmc` | 291 | **wire-or-archive (operator call)** | DECIDE leg of the agentic-trading stack; live API key exists (`/root/.config/cmc`). No MCP surface consumes it today (`flux_0x_*` exists, `flux_cmc_*` doesn't). Either wire `signal()` into an MCP tool + default-members, or archive with the other trading legs. |
| `flux-trade` | 974 | **wire-or-archive (operator call)** | DECIDE+SIZE leg (9 SIMD indicators, Kelly). Same status as flux-cmc — built, tested, never wired to a surface. Decision couples with `flux-agent-trade`. |
| `flux-agent-trade` | 186 | **wire-or-archive (operator call)** | The planned decide→0x→gate dry-run combo. If the trading loop is still wanted, this is the crate that makes cmc/trade reachable; if not, all three go to archive together. |
| `flux-cashewswap` | 457 | **keep (wire behind feature)** | LiquidityPool.sol port, 11/11 green (1e50e25e); the CashewSwap revival has live dist pages + a coordinated split on the bus (#255/#257). Pruning it orphans that lane. |

## Notes on rows I don't own but reviewed in passing

- `sigil-ashwalker` (8,678 LOC — the single biggest unreachable row): lives in
  the FLUX tree but is SIGIL-domain; 108/108 green when last combo'd
  (2026-06-04). Needs its owner's verdict, not mine — flagging size so it
  doesn't get batch-archived silently.
- The `quillonos-*` family (7 rows) travels together — one verdict should
  cover all seven (they're the os.html WASI userspace; quillonos skill still
  references them).

## Process rules this review assumes

1. Archive = move under `archive/` (or a dedicated branch) preserving git
   history. Never `rm`, per the report's own charter.
2. Any prune batch lands as ONE commit per verdict-group, path-scoped, with
   the verdict table in the message — so a single revert restores a group.
3. Re-run `fluxc prune-report` AFTER the external-reachability column exists;
   diff the two lists; only the intersection is prune-eligible.

— rocky (claude-code on Epsilon), task ref: bus msgs #741 → this file.
