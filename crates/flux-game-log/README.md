# flux-game-log

Game-event sink for flux-arena. Accepts HTTP POSTs on `/v1/game`, appends each
event as one JSONL line to disk.

Replaces the dev-loop Python prototype at
`/home/storage/unreal/bundle/server-deploy/flux-game-event-logger.py`. Same
wire format, same on-disk format — direct drop-in.

## Why a dedicated crate

The cooked `FluxArenaServer.sh` (Linux dedicated, Shipping) POSTs telemetry
events — `kill_event`, `weapon_fired`, `position_sample`, etc. — to a
persistent sink. Tail-based consumers (Claude's `narrate-match.sh`,
Compile Garden widgets, any operator with `tail -F`) read from there.

`flux-ue-bridge` accepts arbitrary JSON at `/v1/webhook` and broadcasts to
WebSocket subscribers but **does not persist** — broadcast-only,
in-memory. Game telemetry needs persistence: post-mortem, screenshots,
v0.2 replay clips, training data. Hence this crate.

## Usage

```bash
# Default — listen on :9990, log to /tmp/flux-game-events.jsonl
fluxc build --package flux-game-log
./target/debug/flux-game-log

# Custom port + path + forward each event to flux-ue-bridge too
./target/debug/flux-game-log \
    --listen 0.0.0.0:9990 \
    --path /var/log/flux-arena/events.jsonl \
    --forward http://127.0.0.1:9989/v1/webhook
```

Environment-variable equivalents: `FLUX_GAME_LOG_LISTEN`,
`FLUX_GAME_LOG_PATH`, `FLUX_GAME_LOG_FORWARD`, `FLUX_GAME_LOG_MAX_BODY`.

## Wire format

POST `/v1/game` with arbitrary JSON in the body. Server validates JSON
syntax (rejects garbage), wraps in an ingest record, appends:

```json
{ "ingest_t": 1780053941,
  "remote":   "127.0.0.1",
  "path":     "/v1/game",
  "payload":  { /* whatever the client posted */ } }
```

Returns `{"ok":1}` (200) on success, `{"ok":0,"err":"not-json"}` (400) on
parse failure, `{"ok":0}` (413) on body-size cap.

GET `/healthz` returns:

```json
{ "ok": 1,
  "log": "/tmp/flux-game-events.jsonl",
  "uptime_s": 1234,
  "accepted": 1042,
  "rejected": 3,
  "bytes_written": 412877,
  "forward_errors": 0 }
```

## Operator deploy

Systemd unit at
`/home/storage/unreal/bundle/server-deploy/flux-game-event-logger.service`
(written for the Python prototype but the unit body is the same — just
swap the `ExecStart`):

```ini
ExecStart=/usr/local/bin/flux-game-log
Environment=FLUX_GAME_LOG_PATH=/var/log/flux-arena/events.jsonl
```

`Restart=on-failure`, `User=orobit`, `ReadWritePaths=/var/log/flux-arena`.

## Tests

```bash
fluxc test --package flux-game-log
```

Unit tests cover `ForwardTarget::parse`, the `find_double_crlf` helper,
and `target_path` query stripping. Integration tests live in `tests/`
(when added).

## Coordination

Owned this session by `rocky-arena-1` (swarm task `rocky-arena-1-45`).
Drop a note in `bundle/inbox/rocky-arena-1.md` for handoff.
