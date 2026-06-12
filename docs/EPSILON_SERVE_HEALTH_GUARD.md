# Epsilon `fluxc serve` — health guard audit + rollback notes

T5 `epsilon-serve-health-guard` (swarm board #157, task rocky-281).
Read-only audit performed 2026-06-12 by rocky. **No restart performed**
(live-guard respected). Everything below was observed, not assumed.

## Live state (2026-06-12 ~13:11 UTC)

| Fact | Value |
|---|---|
| Process | PID 736911, `fluxc serve`, started Mon Jun 8 09:54 (uptime 4d+) |
| Bind | `127.0.0.1:8084` **only** — confirmed via `ss -ltnp`; no fluxc wildcard binds |
| Health | `GET /api/health` → `200 OK` (0.11s) · `GET /api/stats` → 200 (0.5ms, JSON) |
| ⚠️ Note | `GET /health` (no `/api` prefix) is **404** — monitors must use `/api/health` |
| ⚠️ Staleness | `/proc/736911/exe` → **(deleted)** — the running binary's inode was replaced by today's T1/T3 rebuilds (disk binary mtime Jun 12 12:32). The live process is running Jun-8 code. |

Wildcard listeners on the box (`*:9610` node, `*:8843` socat, `*:5190` node,
`0.0.0.0:80/443/9443` q-flux) are **not fluxc-core** — out of scope, listed so
nobody misattributes them.

## Code audit — bind safety (crates/fluxc-core)

- **serve.rs:1004–1019** — `FLUX_SERVE_HOST` defaults to `127.0.0.1`; an
  explicit **public-bind guard** refuses any non-localhost bind unless
  `FLUX_SERVE_TOKEN` or `FLUX_TLS_CERT/KEY` is set:
  `"Refusing to bind fluxc serve to {} without FLUX_SERVE_TOKEN"`. Good.
- **portal.rs:73** — `FLUX_PORTAL_HOST` defaults to `127.0.0.1`.
- **webhook.rs** — outbound dispatch has SSRF checks; loopback targets are
  rejected by default and only allowed for the explicit
  `127.0.0.1:8084/api/build_event` self-feed path (tested at webhook.rs:680).

Verdict: **localhost-by-default is enforced in code, not just by deployment
habit.** Env overrides (`FLUX_SERVE_HOST/PORT`) are the only escape hatch and
the public path requires a token or TLS material.

## Restart / rollback procedure (OPERATOR APPROVAL REQUIRED)

The running process holds a deleted inode. The moment it exits, the Jun-8
binary is gone forever — so **step 1 is not optional**:

```bash
# 1. PRESERVE the exact running binary BEFORE any restart
cp /proc/736911/exe /home/storage/tmp/fluxc-serve-rollback-jun08

# 2. Verify it is NOT systemd-managed before kill (it appears hand-launched):
systemctl status fluxc-serve 2>&1 | head -3   # expect "could not be found"

# 3. Restart onto the new binary (kill -9 — killall is unreliable on this box)
kill -9 736911
cd /home/storage/deepseek-codewhale/flux
nohup ./target/debug/fluxc serve > /home/storage/logs/fluxc-serve.log 2>&1 &

# 4. Post-restart verification (all three must pass)
ss -ltnp | grep 8084            # must show 127.0.0.1:8084 ONLY
curl -s http://127.0.0.1:8084/api/health    # must be: OK
curl -s http://127.0.0.1:8084/api/stats | head -c 200   # JSON, sane fields

# 5. ROLLBACK if anything is wrong
kill -9 <new pid>
nohup /home/storage/tmp/fluxc-serve-rollback-jun08 serve > /home/storage/logs/fluxc-serve.log 2>&1 &
# then re-run step 4
```

Known consumers of :8084 that a restart briefly interrupts: garden-state.json
snapshot writer (quillon.xyz/garden.html), `/api/build_event` webhook feed,
`/sse` dashboard stream. All reconnect-tolerant; none are money paths.
