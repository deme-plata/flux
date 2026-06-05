# flux-record

Cinematic Claude Code session recorder. Shells out to `ffmpeg` for capture and
filtergraph work; the Rust side parses Claude transcripts, plans overlays,
detects highlight moments, and bridges Vite HMR into the timeline.

## What it makes

- A YouTube-ready 1920×1080 `mp4` from a raw screen capture + Claude transcript,
  with title card, ken-burns, vignette, tool-call lower-third badges, watermark.
- A `chapters.txt` for the YouTube description, clustered from tool calls.
- Vertical 9:16 `mp4` highlight clips for Shorts/TikTok, picked from interesting
  moments (errors, dense tool bursts, long prompts).
- A burned-in ASS karaoke caption track from assistant text.
- A 1920×1080 PNG cover thumbnail.
- A picture-in-picture *terminal + Vite preview* feed for "watch the UI build
  itself" videos, with HMR badges firing the instant the browser hot-reloads.

## Requirements

```bash
apt-get install -y ffmpeg     # x11grab + libavfilter
# DejaVu Sans is the drawtext default and ships on most distros.
```

## Subcommands

```
flux-record start         # capture screen + audio -> raw.mkv
flux-record stop          # SIGINT the running capture (flushes mux)
flux-record dual          # terminal window + browser window side-by-side
flux-record render        # raw + transcript -> cinematic.mp4
flux-record chapters      # transcript -> YouTube chapter timestamps
flux-record shorts        # raw + transcript -> 9:16 highlight clips
flux-record captions      # transcript -> karaoke ASS subtitle file
flux-record thumbnail     # one frame + title overlay -> PNG cover
flux-record control       # HTTP listener for Vite HMR events
flux-record bookmarklet   # print one-liner JS to drop in your bookmarks bar
flux-record vite-plugin   # print vite-plugin-flux-record.mjs source
```

## Sample workflow — recording a session

```bash
# 1. Start capture in one terminal
flux-record start --out raw.mkv --display :0.0 --size 1920x1080 --fps 30

# 2. Use Claude Code normally in another terminal.
#    Your transcript will be at:
#      ~/.claude/projects/<encoded-cwd>/<session-id>.jsonl

# 3. Stop when done
flux-record stop

# 4. Render the cinematic version
flux-record render \
  --raw raw.mkv \
  --transcript ~/.claude/projects/-home-storage-foo/abc123.jsonl \
  --title "Building Flux with Claude" \
  --subtitle "Live coding · Claude Opus 4.7" \
  --out cinematic.mp4

# 5. Generate YouTube description chapters
flux-record chapters --transcript ~/.claude/.../abc123.jsonl --out chapters.txt

# 6. Auto-cut up to 5 vertical Shorts
flux-record shorts \
  --raw raw.mkv \
  --transcript ~/.claude/.../abc123.jsonl \
  --max-seconds 60 --max-clips 5 \
  --out shorts/

# 7. Make a thumbnail PNG from the 3s frame
flux-record thumbnail \
  --raw raw.mkv \
  --at-seconds 3.0 \
  --title "Building Flux with Claude" \
  --subtitle "Opus 4.7 · live" \
  --out cover.png
```

## Sample workflow — Vite + React HMR side-by-side

Two windows: terminal on the left, browser on the right pointed at your Vite
dev server. The HMR control server captures every hot-reload event from your
running Vite instance and overlays a yellow `HMR` badge the moment it fires.

```bash
# 1. Start the HMR control server (one terminal)
flux-record control --bind 127.0.0.1:9876 --events flux-events.jsonl

# 2. Wire your Vite project to it. Print the plugin, drop it in your project:
flux-record vite-plugin > vite-plugin-flux-record.mjs

# Then in vite.config.ts:
#   import flux from './vite-plugin-flux-record.mjs';
#   export default { plugins: [react(), flux()] };

# OR, no project changes — print a bookmarklet and click it in the browser:
flux-record bookmarklet
# (paste the javascript: line into a new bookmark, click it on your Vite tab)

# 3. Start dual capture. Get the geometries via:
#      xdotool getwindowgeometry $(xdotool getactivewindow)
#    "WxH+X+Y" — e.g. "1280x1440+0+0" for the left half of a 4K display.
flux-record dual \
  --term "1280x1440+0+0" \
  --browser "1280x1440+1280+0" \
  --out dual.mkv

# 4. Code in Claude Code. Each time Vite hot-reloads, the control server
#    catches it and writes flux-events.jsonl. Each console.error and
#    history.pushState is captured too.

# 5. Stop
flux-record stop

# 6. Render — pass --events to fold HMR badges in alongside Claude tool cards
flux-record render \
  --raw dual.mkv \
  --transcript ~/.claude/.../abc123.jsonl \
  --events flux-events.jsonl \
  --title "Building a Dashboard with Claude" \
  --out cinematic.mp4
```

## How transcripts are parsed

`transcript.rs` reads Claude Code JSONL line by line and emits `Event { t_s,
kind, label, body }` records:

| Source                                  | Event kind                  | Card badge | Color   |
|-----------------------------------------|-----------------------------|------------|---------|
| `{"type":"user", ...}`                  | `User`                      | `YOU`      | green   |
| `{"type":"assistant", content: text}`   | `AssistantText`             | `CLAUDE`   | indigo  |
| `{"type":"assistant", tool_use:Bash}`   | `ToolUse("Bash")`           | `BASH`     | green   |
| `{"type":"assistant", tool_use:Read}`   | `ToolUse("Read")`           | `READ`     | blue    |
| `{"type":"assistant", tool_use:Edit}`   | `ToolUse("Edit")`           | `EDIT`     | orange  |
| `{"type":"tool_result", is_error:true}` | `ToolResult{is_error:true}` | `ERROR`    | red     |
| Vite HMR (via control server)           | `ToolUse("HMR")`            | `HMR`      | amber   |
| DOM mutations (via bookmarklet)         | `ToolUse("DOM")`            | `DOM`      | teal    |
| `console.error` (via bookmarklet)       | `ToolResult{is_error:true}` | `ERROR`    | red     |
| `history.pushState` (route)             | `ToolUse("ROUTE")`          | `ROUTE`    | indigo  |

## Highlight scoring (shorts)

| Heuristic                              | Score |
|----------------------------------------|-------|
| Any error in a tool result             | 10    |
| Dense tool-call burst (≥3 within 8s)   | 6 + N |
| Long user prompt (>80 chars)           | 4     |

Top-scored, non-overlapping windows are exported as 9:16 clips. Clip caption is
burned into the top of the frame.

## Building

```bash
cd /home/storage/deepseek-codewhale/flux
./target/debug/fluxc build --package flux-record
```

Tests:

```bash
# fluxc test scope is currently broken — runs whole workspace.
# As a workaround, run the lib test binary directly:
./target/debug/fluxc test 2>/dev/null
ls -lt target/debug/deps/flux_record-* | grep -v '\.d$' | awk '$5 > 8000000 {print $NF; exit}' | xargs -I{} ./{}
# Expect: 13 passed; 0 failed.
```

## Architecture

```
flux-record/
├── src/
│   ├── main.rs        clap CLI dispatch
│   ├── lib.rs         module exports
│   ├── transcript.rs  Claude JSONL -> Event stream            (4 tests)
│   ├── ffmpeg.rs      filtergraph builder, render, thumbnail
│   ├── capture.rs     x11grab/pulse single + dual capture
│   ├── overlay.rs     event -> animated Card with badge+color
│   ├── chapters.rs    cluster events -> YouTube chapters       (3 tests)
│   ├── shorts.rs      heuristic highlight picker + 9:16 render (3 tests)
│   ├── captions.rs    assistant text -> karaoke ASS subtitles
│   └── vite.rs        HMR control server + event merger        (3 tests)
└── Cargo.toml
```

Zero non-stdlib transport deps for the control server — it's a `TcpListener`
with a hand-rolled HTTP parser to keep the dependency surface tiny.
