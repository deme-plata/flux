# Flux Source Editing — UI deploy tools & Excel planning

> **MCP tools:** `fluxc-mcp/src/handlers/frontend.rs` (`flux_ui_*`)  
> **Excel prototype:** Flux Aether control surface (`tools/create-prototype1-xlsx.mjs`)

Two complementary workflows: **advanced deployed-source editing** via MCP (production static UI on Epsilon), and **Excel-based prototype planning** for the Windows control surface.

---

## Problem: 24-hour browser cache

q-flux serves `dist-final/*.html` with `cache-control: max-age=86400`. Editing a file and reloading the bare URL shows the **old** page for up to 24h. Every desktop/UI iteration was painful.

The `flux_ui_*` tools fix this without touching the q-flux binary: deploy writes the file **and** returns a `?v=<epoch>` cache-busted URL.

---

## MCP tools (`flux_ui_*`)

| Tool | Purpose |
|------|---------|
| `flux_ui_list` | List static surfaces (HTML/CSS/JS) with size, mtime, URLs |
| `flux_ui_read` | Read deployed file into agent context (diff/patch) |
| `flux_ui_deploy` | Write file + return cache-busted URL |
| `flux_ui_preview` | Mint fresh `?v=<epoch>` for existing file |
| `flux_launcher_register` | Wire app into `desktop.html` (APPS + quick-launch + dock) atomically |

### Environment

| Variable | Default |
|----------|---------|
| `FLUX_UI_ROOT` | `/home/orobit/q-narwhalknight/dist-final` |
| `FLUX_UI_ORIGIN` | `https://sigilgraph.fluxapp.xyz` (override; code default is `quillon.xyz`) |

Paths are sandboxed: no `..`, no absolute escapes, containment check on resolved path.

### Typical edit workflow

```
1. flux_ui_list              → see what's deployed
2. flux_ui_read {file}       → load current source
3. [agent patches content]
4. flux_ui_deploy {file, content}  → write + get cache-busted URL
5. Hand human the ?v= URL    → immediate visible change
```

### Launcher registration

`flux_launcher_register` updates three places in `desktop.html` idempotently:

- `APPS` registry
- Quick-launch tile
- Dock item

Returns cache-busted `desktop.html?v=<epoch>` URL.

---

## Excel prototype planning (Windows control surface)

**Not in the Flux Rust workspace** — lives in the Flux Aether control surface prototype:

```
tools/create-prototype1-xlsx.mjs  →  Flux_Prototype_1_Windows.xlsx
```

Generates a multi-sheet `.xlsx` (pure Node, no Excel dependency) covering:

| Sheet | Contents |
|-------|----------|
| Prototype 1 | P0 areas: Recorder, Presence Video, MCP Command Surface, Operator Skills, Auto Updater |
| MCP Commands | Command candidates with input signals, output events, endpoints |
| Recorder Presence | Webcam opt-in, floating video, overlay reflow |
| Operator Skills | Movement/writing/command style learning policies |
| Auto Updater | Check → verify → apply → restart → rollback stages |
| Event Schema | Required/optional fields per event type |
| Test Plan | P0/P1 verification steps |

Run locally:

```bash
node tools/create-prototype1-xlsx.mjs
# → Flux_Prototype_1_Windows.xlsx
```

Use case: operator reviews prototype scope in Excel, then agents implement against the standardized event schemas documented in the workbook.

---

## Relationship to flux-search

Indexed source files (`.rs`, `.md`, `.html`, etc.) are searchable via `flux_search`. After `flux_ui_deploy`, re-index to make deployed UI changes discoverable:

```json
{"name":"flux_search_index","arguments":{"path":"/home/orobit/q-narwhalknight/dist-final","reindex":false}}
```

---

## Relationship to flux-rev

For workspace source (not dist-final static), use [Flux Rev](./FLUX_REV.md) snapshots to content-address edits before deploying UI surfaces.

---

## Safety

- `flux_ui_*` writes only inside `FLUX_UI_ROOT` sandbox
- Excel generator is read-only spec output — no external side effects
- Deployed changes are visible immediately via cache-busted URLs (no hidden edits)

— Documented from `frontend.rs` + control-surface prototype, 2026-06-06.