# Flux SSH — Fleet, Distributed Build & Epsilon Hardening

> **Fleet crate:** `flux-fleet` (see `docs/skill-report/03-flux-ssh.md`)  
> **Distributed build:** `fluxc-core/src/distributed.rs`  
> **Serve hardening:** `fluxc-core/src/serve.rs`, `portal.rs`

Flux SSH covers three layers: fleet discovery/install, multi-machine distributed builds, and Epsilon service hardening (localhost-only binds + token auth).

---

## flux-fleet (SSH-key fleet driver)

From `docs/skill-report/03-flux-ssh.md`:

| Command | Purpose |
|---------|---------|
| `flux-fleet scan [--probe]` | Parse `~/.ssh/{config,known_hosts,*.pub}` → candidate hosts; `--probe` runs `ssh BatchMode "uname -sm"` + remote `flux --version` |
| `flux-fleet up [--confirm]` | Push musl-static flux binary; version-aware, idempotent. **DRY-RUN by default** |
| `flux-fleet run --hosts a,b "<cmd>"` | Parallel SSH with **flux:// caching** (read-only cmds cached) |

Safety:

- **BatchMode-only** — no password harvest
- Every `up` appended to `~/.flux/fleet-audit.log`
- No key bytes leave the scan

Epsilon SSH config (operator machine):

```
Host epsilon
    HostName 89.149.241.126
    User root
    IdentityFile ~/.ssh/id_ed25519
```

---

## Distributed build (`distributed.rs`)

`fluxc` distributed build round-robins crates across the supercluster via SSH:

| Peer | Host |
|------|------|
| local | Epsilon workspace |
| delta | `root@5.79.79.158` |
| beta | `root@185.182.185.227` |

Flow:

1. `flux_graph::resolve_workspace` → crate batches
2. `rsync -azq --exclude=target --exclude=.git` workspace to `/tmp/flux-dist/flux` on each peer
3. Parallel `ssh` + `cargo check/build --package <crate>` per batch
4. SEC-1 guard: crate names validated `[A-Za-z0-9_-]` before shell interpolation (prevents RCE via malicious `Cargo.toml` name)

---

## Epsilon hardening (2026-05-31)

Documented in operator audit `epsilon-hardening-status-2026-05-31.md`:

### Completed

| Service | Before | After |
|---------|--------|-------|
| Flux Eye (`eye-server.mjs`) | `0.0.0.0:9789` | `127.0.0.1:9789` |
| `fluxc serve` | public `:8084` | `127.0.0.1:8084` |
| `/root/.flux` perms | loose | `700` |
| `webhooks.json` perms | loose | `600` |

External checks to `89.149.241.126:9789/health` and `:8084/api/health` **fail** after hardening (expected). Local `127.0.0.1` checks pass.

### Serve configuration

```bash
FLUX_SERVE_HOST=127.0.0.1 ./target/debug/fluxc serve
```

Optional token auth:

```bash
FLUX_SERVE_TOKEN=<secret> ./target/debug/fluxc serve
```

Accepts `Authorization: Bearer <token>` or `X-Flux-Token: <token>` on mutating endpoints (`POST /api/tune`, `POST /api/build_event`).

Default bind: `127.0.0.1` (via `FLUX_SERVE_HOST`, falls back from `0.0.0.0`).

### Portal multiplexer (`portal.rs`)

Single-port routing alternative:

- Master port `:8084` routes `/api/*`, `/sse`, `/webhook`, `/p2p/*`, `/dashboard`
- Internal services via Unix sockets in `/tmp/flux-portal/`
- Default bind: `FLUX_PORTAL_HOST=127.0.0.1`

---

## MCP combo SSH pattern

Shell combos use guarded SSH to Epsilon:

```bash
ssh -o BatchMode=yes -o ConnectTimeout=10 epsilon \
  "bash -lc 'cd /home/storage/deepseek-codewhale/flux && exec target/debug/fluxc mcp'"
```

Environment:

| Variable | Default |
|----------|---------|
| `FLUX_EPSILON_HOST` | `epsilon` |
| `FLUX_REMOTE_ROOT` | `/home/storage/deepseek-codewhale/flux` |

Brace templates (`docs/flux-brace-templates.md`) document the voice→SSH grammar:

```
{target:epsilon, transport:ssh, repo:{repo}, command:fluxc {action}, verify:{verify}}
```

Templates are visible command grammar — they do not execute SSH by themselves.

---

## Reverse proxy access

Public surfaces terminate on Epsilon via `flux-flux` reverse proxy (same pattern as `sigilgraph.fluxapp.xyz`). Localhost binds + reverse proxy = services not directly exposed on public IP.

---

## Parked / pending

- Review and split large `serve.rs` diff into clean PR
- Decide on `FLUX_SERVE_TOKEN` enablement for production
- Re-run read-only git status when SSH approval budget available

---

## Quick verification

```bash
ssh epsilon "ss -tlnp | grep -E '8084|9789'"
# Expect 127.0.0.1:8084 (fluxc) and 127.0.0.1:9789 (eye-server)

ssh epsilon "curl -s http://127.0.0.1:8084/api/health"
# Expect: OK
```

— Documented from fleet skill-report, distributed.rs, serve.rs, hardening audit, 2026-06-06.