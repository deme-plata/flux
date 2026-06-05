# codewhale-deck v0 — multi-window terminal for AI sessions

> **One-line:** tmux-style multi-pane terminal where each pane hosts an AI agent session (Claude Code, Codex, custom Flux agents), with a codewhale-themed sidepanel showing live tokens spent, tool-call timeline, swarm identity, file claims, and inbox feed.

## Scope

Operators today juggle **N concurrent AI sessions** across browser tabs + terminal tmux panes + Slack-style swarm messages. No single surface shows: "this session has spent $4.32, holds 3 file claims, has 7 unread swarm messages, is on its 12th tool call." codewhale-deck is that surface.

What it does:
1. **Pane manager** — spawn / kill / resize panes, each with its own PTY running an AI agent.
2. **Codewhale sidepanel** — collapsible side dock showing per-session: live token spend (via `codewhale-gate` cost-stream), recent tool calls (last 5), agent identity (`agent_id` + wallet prefix), active file claims (`flux_file_list`), unread swarm-message count.
3. **Cost rollup** — sum across all panes; show daily / monthly running total.
4. **Hot-paste channels** — `:swarm <msg>` posts to inbox; `:claim <file>` adds a file lease; `:tour <n>` runs `tourbillon` over the highlighted scenario.
5. **Recording** — every keystroke + every model response gets append-only logged for replay (chronos hooks in P2).

## Dep graph

```
codewhale-deck (bin)
├── ratatui                          (TUI rendering)
├── crossterm                        (terminal backend)
├── tokio                            (async PTY + WS)
├── portable-pty                     (PTY spawn — cross-platform)
├── tungstenite                      (WebSocket to codewhale-gate cost stream)
├── reqwest                          (REST to MCP / swarm tools)
├── serde_json
└── flux-swarm-tools (lib)           (file claims + messages — already exists)
```

No new server-side dep: codewhale-gate already streams cost; flux-swarm-tools already exposes file claims + inbox.

## Layout

```
┌───────────────────────────────────────────────────────────┬────────────────┐
│ ┌─ rocky-sigil ───────────────┐ ┌─ codex ─────────────┐   │  CODEWHALE     │
│ │                             │ │                     │   │  ────────────  │
│ │  claude-code session        │ │  codex session      │   │  $ today $4.32 │
│ │  > running flux_combo...    │ │  > generating diff  │   │  $ month $87.10│
│ │  ✓ tests 21/21              │ │  ...                │   │                │
│ │                             │ │                     │   │  ───SESSIONS── │
│ │                             │ │                     │   │  ▶ rocky-sigil │
│ │                             │ │                     │   │    tokens 4k   │
│ ├─ rocky-updater ─────────────┤ │                     │   │    claims 2    │
│ │                             │ │                     │   │    inbox 3 new │
│ │  claude-code session        │ │                     │   │                │
│ │  awaiting prompt...         │ │                     │   │  • codex       │
│ │                             │ │                     │   │    tokens 1.2k │
│ │                             │ │                     │   │    claims 0    │
│ └─────────────────────────────┘ └─────────────────────┘   │  • rocky-updater│
│                                                            │    idle        │
│  Status: 3 panes · $0.42/h · 5 claims · 8 inbox            │  ───TOOLCALLS──│
│                                                            │  19:42 flux_combo│
└───────────────────────────────────────────────────────────┴────────────────┘
```

Sidepanel toggles with `Ctrl-b s` (mirrors tmux's pane-side convention).

## Concrete v0 line list (target ~1500 LOC)

```
crates/codewhale-deck/
├── Cargo.toml
└── src/
    ├── main.rs              ~80    ratatui main loop + crossterm setup
    ├── app.rs              ~120    App state, pane list, sidepanel state
    ├── pane.rs             ~150    one Pane = PTY + screen buffer + cursor
    ├── pty.rs              ~120    portable-pty wrapper, async read/write
    ├── input.rs            ~120    key dispatch (Ctrl-b prefix, :command mode)
    ├── render/
    │   ├── grid.rs         ~120    panes layout (horizontal/vertical splits)
    │   ├── sidepanel.rs    ~150    codewhale dock — tokens/claims/tools/inbox
    │   ├── status.rs        ~80    bottom bar — total cost + counts
    │   └── theme.rs         ~60    obsidian/violet palette (skill-locked)
    ├── codewhale/
    │   ├── cost_stream.rs  ~100    WS client to codewhale-gate /cost-stream
    │   ├── swarm.rs        ~120    flux-swarm-tools client (claims, inbox)
    │   └── toolcall.rs      ~80    parse Claude Code / Codex tool-call lines
    ├── command.rs          ~120    :swarm :claim :tour :record handlers
    └── record.rs            ~80    append-only log of (session, ts, bytes)
```

Bindings (vi-flavored, tmux-compatible prefix):
```
Ctrl-b %        split pane vertically
Ctrl-b "        split pane horizontally
Ctrl-b o        cycle pane focus
Ctrl-b s        toggle sidepanel
Ctrl-b :        command mode (:swarm, :claim, :tour, :record)
Ctrl-b z        zoom pane fullscreen
Ctrl-b ?        keymap help
```

## What this v0 deliberately does NOT include

- **Browser variant** — native TUI only. Browser version stacks on top of `flux-ide` (xterm.js + WS PTY) later.
- **Pane sync / mirroring** — single-keystroke-to-multi-pane is a v1 ergonomic. Saves the "type once, broadcast to N agents" workflow for when the abstraction lands.
- **Session persistence across restarts** — record-only in v0. Replay-into-fresh-pane is a recording.rs follow-up.
- **Agent-spawning UI** — operator launches `claude-code` / `codex` themselves in each pane. Auto-spawn-via-MCP is when MCP is wired (~CHRONOS-D timing).

## Open Qs

1. **Cost stream from codewhale-gate** — does the gate already expose `WS /v1/cost-stream/{session_id}`? If not, that's a deck-blocking dep (gate adds it). Gate's v0 sketch (`codewhale-gate-v0.md`) didn't include it; needs a paragraph there.
2. **Token attribution** — how does the deck know which pane's session a cost event belongs to? Each pane needs an `X-Codewhale-Session: <uuid>` header injected into the AI agent's outbound API calls. Means `claude-code` / `codex` need a way to take a session id from env (`CODEWHALE_SESSION_ID=...`). Open: do they support that today? If not, we ship a tiny wrapper.
3. **Toolcall parsing** — Claude Code prints tool calls in a specific format; Codex prints another; custom agents print whatever. Parser is fragile. Suggest a `--codewhale-jsonl` flag on agent CLIs so they emit machine-parseable events. Long road, but cleanest.
4. **Window layout serialization** — `.codewhale-deck.toml` per workspace? Auto-save on quit + restore on launch? Lifts ergonomics massively for "I just want to relaunch yesterday's setup."
5. **Compose with FluxOS terminal** — if FluxOS lands, codewhale-deck eventually runs INSIDE FluxOS as a tiling app, not as a top-level binary. Means the layout engine should be abstracted enough to render into either ratatui (terminal) or DOM (FluxOS window). Probably overkill for v0; v0 ships native-only.

## Sequencing

- **Depends on codewhale-gate** for the cost-stream WS endpoint. Suggest: extend codewhale-gate sketch with `WS /v1/cost-stream` spec, build that, then deck.
- **Depends on flux-swarm-tools** (already exists — file claims, inbox MCP tools).
- **Indirectly depends on AI agents emitting structured tool-call events** — fragile parser otherwise. Could ship deck with "best-effort regex" parse for v0 and tighten later.

The whole stack:
```
codewhale-gate (cost + billing + cost-stream WS)
    │
    ├──► codewhale-deck (multi-pane terminal, sidepanel reads gate)
    │
    └──► flux-ide      (browser, agent dock also reads gate)

flux-swarm-tools (claims + inbox + messages)
    │
    ├──► codewhale-deck (sidepanel: claims/inbox/tools-recent)
    │
    └──► flux-ide      (status bar widgets)
```

Both deck and IDE pull from the same two backends (gate + swarm-tools), so the work is amortized.

— rocky-sigil 🟣
