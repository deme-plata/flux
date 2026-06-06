# Bifrost — Flux-native agentic money + inference fabric v0

> Viktor + Grok spec, 2026-06-04. Closed loop: intent → execution → verify → settle → learn.

## Layers

| Layer | Crate / tool | Role |
|-------|----------------|------|
| Router | `flux-moe` | Goals → model/lane (CPU, Vast 4090, DeepSeek API, Qwen vLLM) |
| Spend gate | `flux_vast_recommend` → human `flux_vast_create` + `flux_vast_autostop` | MOE never auto-rents |
| Goals | `flux_goal_post` / `flux_goal_list` | Intent stack (extend to money goals) |
| Swarm | `flux_swarm_claim` / `flux_swarm_complete` | Crate lanes + QUG payout |
| Bank | `flux-bank-*` | Proposal-first ledger + Quillon bridge |
| Verify | `flux_combo`, `flux_chronos_run`, `verify_on_chain` | Objective gates |

## Bifrost loop

```text
flux_goal_post → flux_swarm_claim → flux-moe plan → flux_vast_recommend (ask_id)
  → operator flux_vast_create + flux_vast_autostop → run → tests/build
  → verify_on_chain → flux_swarm_complete → flux_activity_tail / earnings_breakdown
```

## Flux Bank crates (workspace)

- `flux-bank-core` — ledger, `TransferProposal`, `SignedIntent`
- `flux-bank-api` — routes for Stainless/OpenAPI
- `flux-bank-bridge` — Quillon metrics read, link bench hook
- `flux-bank-mcp` — `flux_bank_status` combo

**Rule:** reads/simulate free; writes need signed wallet + 2-of-2.

## Next lanes

1. `flux_bank_propose_transfer` (dry-run only)
2. OpenAPI spec → Stainless SDK (`flux-bank-sdk` crate)
3. `flux_bifrost_run` MCP combo (orchestrates loop)
4. Extend `flux-moe` with `flux_goal_list` consensus input