#!/usr/bin/env python3
"""Update FLUX_MCP_COMBOS.md and FLUX_PLATFORM_INDEX.md for v0.25 platform combos."""
import pathlib

FLUX = pathlib.Path("/home/storage/deepseek-codewhale/flux/docs")

PLATFORM_SECTION = """
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

"""

combos = FLUX / "FLUX_MCP_COMBOS.md"
text = combos.read_text()
# Update architecture diagram
old_arch = """        ├── molt         — flux_molt_combo (agentic-money social)
        └── wallet_xray  — wallet inspection combos"""
new_arch = """        ├── molt         — flux_molt_combo (agentic-money social)
        ├── wallet_xray  — wallet inspection combos
        ├── aether       — flux_aether_ingest/retrieve/sync
        ├── fleet        — flux_fleet_search (SSH fan-out)
        ├── compile_error — flux_compile_error_combo (rustc + snippets)
        └── platform_webhook — audit POST :4178 + jsonl captures"""
if new_arch not in text:
    text = text.replace(old_arch, new_arch)

if "## Platform combos v0.25" not in text:
    # Insert before Release combos section
    marker = "## Release combos (`ops.rs`)"
    if marker in text:
        text = text.replace(marker, PLATFORM_SECTION + "\n---\n\n" + marker)
    else:
        text += "\n" + PLATFORM_SECTION
    combos.write_text(text)
    print("FLUX_MCP_COMBOS.md: updated")
else:
    print("FLUX_MCP_COMBOS.md: already has platform section")

index = FLUX / "FLUX_PLATFORM_INDEX.md"
itext = index.read_text()
itext = itext.replace(
    "| [FLUX_SOURCE_EDITING.md](./FLUX_SOURCE_EDITING.md) | `flux_ui_*` deploy tools + Excel prototype workbook |",
    "| [FLUX_SOURCE_EDITING.md](./FLUX_SOURCE_EDITING.md) | Source editing combos (P2 — no UI in v0.25 gate) |",
)
if "flux_compile_error_combo" not in itext:
    itext = itext.replace(
        "| [FLUX_SSH.md](./FLUX_SSH.md) | flux-fleet, distributed build, Epsilon hardening |",
        "| [FLUX_SSH.md](./FLUX_SSH.md) | flux-fleet, distributed build, Epsilon hardening |\n"
        "| **v0.25 gate** | `tools/v25-release-gate.sh` — Aether + fleet + compile_error + promote (6 gates) |",
    )
index.write_text(itext)
print("FLUX_PLATFORM_INDEX.md: updated")