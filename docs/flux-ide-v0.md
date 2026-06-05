# Flux IDE v0 — browser-first, MIR-observable, AI-native

> **One-line:** Monaco editor in a browser tab, `fluxc serve` as LSP via WebSocket, workspace graph as the file tree, MIR pipeline visualization, inline agent dock. Hosts inside FluxOS desktop later as the flagship app.

## Scope

The Foundation v0.17.0 deck is right: *"don't write a new compiler — own the parts where AI integration matters."* Flux IDE follows the same principle for the IDE: don't write a new Monaco. Wrap it. The added value is:

1. **MIR observability** — Flux's pipeline (Parse → MIR → Lower → CodeGen) is the differentiator. Show MIR side-by-side with source. Step through lowering stages. Highlight which crate / function / line in MIR a given source span produces.
2. **Workspace graph as file tree** — 107 workspace crates with dep edges. Tree view collapses the noise; right-click on a crate shows architecture-predict + heatmap inline.
3. **AI agent dock** — Claude / DeepSeek / GPT panel docked right. Inline diff suggestions. Agents have full access to the workspace graph + MIR view + recent `flux_combo` output.
4. **Provenance lookup** — every binary in the workspace has a `.proof` (per the Flux Foundation deck). Click a function, see its compile provenance.
5. **Live `flux_combo` output** — bottom dock shows the most recent compile + test + predict run, color-coded.

What it's NOT: a new programming language, a new compiler, a new VM.

## Dep graph

```
flux-ide-frontend (browser)
├── monaco-editor              (existing — CDN or npm bundle)
├── @noble/hashes              (for in-browser .proof verify, reused from verify-tip.html)
├── xterm.js                   (terminal pane, reuses flux-os-shell rendering)
├── d3 / cytoscape             (workspace graph viz)
└── flux-ide-client.js         (WS client to fluxc serve)

flux-ide-server (backend)
├── fluxc serve                (already exists — extend with LSP routes)
└── flux-ide-protocol          (new — JSON-RPC frames over WS)
    └── methods:
        ├── workspace.tree     → returns crate graph
        ├── file.read/write    → standard LSP
        ├── compile.run        → wraps flux_combo
        ├── mir.dump           → MIR for a given file + lowering stage
        ├── proof.lookup       → returns .proof for a built artifact
        └── agent.chat         → relays to codewhale-gate (or local Claude/DS/GPT)

flux-ide-as-flux-os-app (FluxOS integration)
└── manifest entry + window-manager hook (P2 — depends on FluxOS landing)
```

## Concrete v0 line list (target ~800 LOC frontend + ~600 LOC server)

```
crates/flux-ide-server/
├── Cargo.toml
└── src/
    ├── lib.rs                    ~60   axum router extension
    ├── protocol.rs              ~120   request/response shapes
    ├── workspace.rs              ~80   reuses fluxc-core::workspace_root + graph
    ├── compile.rs                ~80   wraps flux_combo, streams output
    ├── mir.rs                   ~100   wraps fluxc mir-dump (already exists)
    └── proof.rs                  ~60   reads <bin>.proof, base64-encodes for wire

gui/flux-ide/                  new browser app
├── index.html                  ~100   layout shell + Monaco mount
├── ide.js                      ~250   Monaco bootstrap + WS client
├── workspace-graph.js          ~150   d3 graph render + click handlers
├── mir-pane.js                 ~120   MIR display + lowering stepper
├── agent-dock.js               ~150   chat UI + codewhale-gate client
└── style.css                    ~80   obsidian + violet (skill-locked palette)
```

Top-level browser layout (rough ASCII):
```
┌──────────────────────────────────────────────────────────────────────┐
│ Flux IDE v0   workspace: deepseek-codewhale/flux                     │
├──────────────┬──────────────────────────────┬────────────────────────┤
│              │                              │                        │
│  workspace   │     monaco editor            │   MIR pane             │
│  graph       │     (rust source)            │   (lowered IR for      │
│  (crates)    │                              │    the open file)      │
│              ├──────────────────────────────┤                        │
│              │     terminal (xterm.js)      │   agent dock           │
│              │     flux_combo output        │   (Claude / DS / GPT)  │
│              │                              │                        │
└──────────────┴──────────────────────────────┴────────────────────────┘
```

LSP protocol fragment:
```json
// Editor → server
{ "method": "compile.run", "params": { "package": "sigil-tx" } }

// Server → editor (streamed)
{ "method": "compile.progress", "params": { "stage": "parse",  "ms": 220 } }
{ "method": "compile.progress", "params": { "stage": "mir",    "ms": 480 } }
{ "method": "compile.progress", "params": { "stage": "codegen","ms": 1340 } }
{ "method": "compile.result",   "params": { "tests": 8, "failed": 0,
                                            "warnings": 3, "binary": "sigil-tx.rlib",
                                            "proof": "<base64>" } }
```

## What this v0 deliberately does NOT include

- **Multi-cursor / Vim mode / themes-besides-obsidian-violet** — out of scope; Monaco supports them, we just don't expose them yet.
- **In-browser builds** — fluxc-as-wasm running entirely in the browser. Way too heavy for v0. Server is `fluxc serve` on the operator's host.
- **Real-time collaborative editing** — single-user v0. Y.js + WebRTC is a known next step.
- **Native IDE binary** — browser-only. A `tauri` wrapper is a P2 deliverable.
- **Agent autonomy** — agents respond to user input only; no background "agent observes your code and suggests" loop in v0.

## Open Qs

1. **Where does flux-ide live URL-wise?** `flux.dev`? `quillon.xyz/ide.html`? `sigilgraph.com/ide.html`? FluxOS-internal `flux-os://app/ide` (after OS port)? Probably start with `quillon.xyz/ide.html` since q-flux already serves that origin.
2. **Codewhale-gate dependency** — agent dock would naturally route through `codewhale-gate` to bill for DeepSeek usage. Means IDE benefits from gate landing first. Or: agent dock uses operator's own API key in v0, billing wrapper lands later.
3. **MIR dump format** — does `fluxc mir-dump` exist yet? Grep says no — would need a new subcommand. Could leverage `cargo-show-asm` style approach via `--emit=mir`.
4. **Workspace graph data source** — `cargo metadata` works for crate-level, but the *file → crate → MIR* mapping needs deeper hooks into fluxc-core. Punt the file-level for v0 (file tree per crate is enough).
5. **Provenance UI** — the deck emphasizes "every binary has a .proof". Should the IDE show the .proof as a sidebar? As a hover tooltip? As a status bar item? Suggest status bar — least intrusive, ambient signal.

## Sequencing

Two viable paths:

**Path A — IDE first, OS later:** Build flux-ide as a standalone browser app at `quillon.xyz/ide.html`. Ship FluxOS later, and the IDE becomes its first app via manifest entry. Faster to demo.

**Path B — OS first, IDE on top:** Build FluxOS substrate, IDE is the flagship app from day 1, never has a standalone URL. Cleaner story long-term.

Suggest **Path A** for v0: IDE works in any browser today without requiring the OS substrate. Migration into FluxOS is a manifest entry + window-manager hook — small follow-up.

## Sequencing across all three (with codewhale-gate)

```
codewhale-gate     ──┐
(revenue, fastest)   │
                     ├──► flux-ide      (IDE uses gate for agent dock)
flux-os              ┘     │
(substrate)                │
                           └──► FluxOS hosts IDE as flagship app
                                (manifest entry, post-port)
```

If forced to one-at-a-time: gate → ide → os, in that order.

— rocky-sigil 🟣
