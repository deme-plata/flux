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

**CORRECTION (2026-06-12 restart, executed by rocky):** the service IS
systemd-managed — `fluxc-serve.service` (enabled, `Restart=always`,
`MemoryMax=256M`, `CPUQuota=50%`, `ProtectSystem=strict`, journald logging
under `SyslogIdentifier=fluxc-serve`). The first draft of this doc assumed
hand-launched; it wasn't. Never kill+nohup it — `Restart=always` would
respawn it and you'd have two opinions about who owns the port.

```bash
# 1. PRESERVE the exact running binary BEFORE any restart, if /proc/<pid>/exe
#    shows (deleted) — on exit that inode is gone forever:
PID=$(systemctl show -p MainPID --value fluxc-serve)
ls -la /proc/$PID/exe                          # "(deleted)"? then:
cp /proc/$PID/exe /home/storage/tmp/fluxc-serve-rollback-$(date +%Y%m%d)

# 2. Restart (picks up whatever is at target/debug/fluxc per ExecStart)
systemctl restart fluxc-serve

# 3. Post-restart verification (all four must pass)
systemctl status fluxc-serve --no-pager | head -6   # active (running)
ss -ltnp | grep 8084                # must show 127.0.0.1:8084 ONLY
curl -s http://127.0.0.1:8084/api/health           # must be: OK
curl -s http://127.0.0.1:8084/api/stats | head -c 300
#   ^ check the "version" field matches the intended deploy (dynamic since
#     the v0.27 fix; before that it lied with a hardcoded "v0.9.9-beta1")

# 4. ROLLBACK if anything is wrong
systemctl stop fluxc-serve
cp /home/storage/tmp/fluxc-serve-rollback-<date> \
   /home/storage/deepseek-codewhale/flux/target/debug/fluxc
systemctl start fluxc-serve
# then re-run step 3 (note: the next `fluxc build` overwrites the rollback
# binary on disk again — rollback is a stopgap, fix forward promptly)
```

Executed 2026-06-12 13:42: Jun-8 binary preserved at
`/home/storage/tmp/fluxc-serve-rollback-jun08` (445M), restarted onto the
v0.27.0 build (PID 3023866), all verification points green.

Known consumers of :8084 that a restart briefly interrupts: garden-state.json
snapshot writer (quillon.xyz/garden.html), `/api/build_event` webhook feed,
`/sse` dashboard stream. All reconnect-tolerant; none are money paths.
