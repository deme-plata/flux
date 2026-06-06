# Flux MCP Combos — fluxc combo & fluxc mcp tools

> **Crate:** `flux/crates/fluxc-mcp/`  
> **Entry:** `fluxc mcp` (stdio JSON-RPC)  
> **Registry:** 49+ tools across handler modules

MCP combos are multi-step Flux operations exposed as single MCP tool calls. They save token round-trips, emit webhooks, and push real-time feed events to the `fluxc serve` dashboard SSE stream.

---

## Architecture

```
fluxc mcp
  └── ToolRegistry (handlers/mod.rs)
        ├── build        — flux_compile, flux_version, flux_clean, ...
        ├── test_combo   — flux_combo, flux_test, flux_quickcast, flux_ult
        ├── stats        — flux_stats, flux_heatmap, ...
        ├── predict      — flux_predict, flux_tune, ...
        ├── webhook      — flux_webhook_*, auto_dispatch
        ├── session      — session management
        ├── ops          — flux_search_*, flux_release_*, flux_zk_combo, ...
        ├── chronos      — network simulation tools
        ├── frontend     — flux_ui_* static deploy tools
        ├── nodeswarm    — parallel node dispatch
        ├── sigil_combos — SIGIL wallet/chain combos
        ├── molt         — flux_molt_combo (agentic-money social)
        ├── wallet_xray  — wallet inspection combos
        ├── aether       — flux_aether_ingest/retrieve/sync
        ├── fleet        — flux_fleet_search (SSH fan-out)
        ├── compile_error — flux_compile_error_combo (rustc + snippets)
        └── platform_webhook — audit POST :4178 + jsonl captures
```

Transport: stdio JSON-RPC (`initialize`, `tools/list`, `tools/call`). Protocol version: `2024-11-05`.

Every `tools/call` pushes a feed event via `fluxc_core::serve::push_feed_event(agent, msg, tool_name)`.

---

## Core combos (`test_combo.rs`)

| Tool | Pipeline | Token savings |
|------|----------|---------------|
| `flux_combo` | compile + test + predict | ~67% vs separate calls |
| `flux_quickcast` | tune + check + predict | fast iteration |
| `flux_ult` | check + heatmap + predict | max insight |
| `flux_test` | scoped test run (failures only) | token-efficient |

### `flux_combo` usage

```json
{
  "name": "flux_combo",
  "arguments": { "package": "flux-search", "release": false }
}
```

On completion: fires `combo_complete` webhook via `webhook::auto_dispatch`, renders visual ECONOMICS panel in output.

---

## Search combos (`ops.rs`)

| Tool | One-liner |
|------|-----------|
| `flux_search` | Query persisted index |
| `flux_search_combo` | Optional reindex + search |
| `flux_aether_search_combo` | Reindex Vizily + Flux Aether paths + search |
| `flux_zk_combo` | `pq_status` + `verify_10ms` in one call |

See [FLUX_SEARCH.md](./FLUX_SEARCH.md) for index details.

---


## Platform combos v0.25 (Aether + fleet + compile errors)

> **No UI** — MCP combos + audit webhooks only. Control surface `:4178` receives `POST /api/mcp-webhook`.

| Tool | Module | One-liner |
|------|--------|-----------|
| `flux_aether_ingest` | `aether.rs` | base64 → shard → `content_root` + local store |
| `flux_aether_retrieve` | `aether.rs` | 64-hex `content_root` → reassemble + BLAKE3 verify |
| `flux_aether_sync` | `aether.rs` | mesh divergence report; `sync=true` required to mutate |
| `flux_fleet_search` | `fleet.rs` | local + SSH fan-out search, dedup by URL |
| `flux_compile_error_combo` | `compile_error.rs` | `cargo check` → parse rustc errors → **source snippets at fault lines** |

### `flux_compile_error_combo` — agent-fast error fixing

On compile failure the combo:
1. Parses every `--> file:line:col` rustc diagnostic
2. Reads `context_lines` (default 6) around each error site
3. Returns highlighted snippets (`>>>  NNN | code`) in MCP output
4. Writes `target/flux-compile-errors/latest.json` (full structured payload)
5. Fires `compile_error` + `build_failed` swarm webhooks
6. POSTs audit envelope to `http://127.0.0.1:4178/api/mcp-webhook` (override: `FLUX_PLATFORM_WEBHOOK_URL`)
7. Appends `target/flux-platform-captures/compile_error.jsonl`

```json
{
  "name": "flux_compile_error_combo",
  "arguments": {
    "package": "fluxc-mcp",
    "context_lines": 8
  }
}
```

### Security (`platform_security.rs`)

- Query sanitization, strict JSON schema, 64-hex `content_root`
- Fleet SSH: query via base64 env — no user string in shell
- `flux_aether_sync`: `sync` defaults **false**

### Gate

```bash
bash tools/v25-release-gate.sh   # 6/6 platform gates (no Excel, no flux_ui_*)
```


---

## Release combos (`ops.rs`)

| Tool | Description |
|------|-------------|
| `flux_release_publish` | Publish any product binary + write `<product>-latest.json` |
| `flux_release_check` | Fetch manifest, report version/URL/size (no download) |

CLI equivalent: `fluxc release [VERSION] [--product NAME] [--binary PATH]`

See [FLUX_RELEASE_PIPELINE.md](./FLUX_RELEASE_PIPELINE.md).

---

## Domain-specific combos

| Module | Example tools |
|--------|---------------|
| `sigil_combos` | SIGIL wallet/chain verification combos |
| `sigil_cosmos` | Quantum Cosmos × SigilGraph κ-phase |
| `agora_stargate` | `flux_agora_stargate_combo` — compile + test |
| `molt` | `flux_molt_combo` — swarm inspiration → compose → post |
| `wallet_xray` | Wallet inspection for AI agents |
| `nodeswarm` | Cross-call stateful node swarm dispatch |

---

## Shell combo scripts (allowlisted buttons)

Local operator scripts in `tools/mcp-combos/` (Flux Aether control surface):

| Script | Combo name | Lane |
|--------|------------|------|
| `flux_combo_fluxc_search.sh` | `fluxc_aether_search_gate` | SSH → Epsilon `fluxc mcp` |
| `flux_combo_fluxc_verify.sh` | verify gate | build verification |
| `flux_combo_aether_emit.sh` | `aether_pulse_visible_combo` | visible localhost events |

Pattern:

1. Source `_flux_root.sh` for Aether URL + webhook helpers
2. Run guarded operation (SSH, curl, dry-run by default)
3. `POST /api/mcp-webhook` with `{source, combo, status, score, detail}`
4. Append to `captures/webhooks.jsonl`

Guardrails: `FLUXC_BUILD=0` default (dry-run), `FLUX_SEARCH_REINDEX=0` default (read-only index).

---

## Running MCP on Epsilon

```bash
ssh epsilon
cd /home/storage/deepseek-codewhale/flux
./target/debug/fluxc mcp
```

From local agent (combo script pattern):

```bash
ssh -o BatchMode=yes epsilon \
  "bash -lc 'cd /home/storage/deepseek-codewhale/flux && exec target/debug/fluxc mcp'" \
  < requests.jsonl
```

Configure in Cursor/Claude: point MCP server to `fluxc mcp` on Epsilon (or local build).

---

## Tool naming convention

All tools prefixed `flux_` (enforced by registry test). Unknown tools return `None` from `registry.execute()`.

---

## Tests

`fluxc-mcp/src/lib.rs` tests verify: registry size ≥49, dispatch for version/stats/tune/search, all names prefixed, `flux_combo` dispatches.

— Documented from `fluxc-mcp` source, 2026-06-06.
## Wickes CMS ? Sigil Bank combos (`handlers/bank.rs`)

| Tool | Description |
|------|-------------|
| `flux_wickes_site_propose` | Proposal-only CMS skeleton (flux-cck + flux-pagebuilder patterns) |
| `flux_company_launch_combo` | CMS + treasury seed dry-run + bank status |
| `flux_bank_status` | Read-only Quillon/Sigil bank metrics |
| `flux_bank_propose_transfer` | Dry-run transfer (never executes) |
| `flux_bifrost_run` | Goal post + MoE route + bank status |

Core logic: `flux-bank-mcp/src/company_launch.rs`. Proposal-first spend gate per `flux-dev-gate`.

Example:

```json
{"name":"flux_company_launch_combo","arguments":{"company_name":"Acme Corp","slug":"acme","founder_wallet":"f","treasury_wallet":"t","seed_capital_uqug":1000000}}
```
