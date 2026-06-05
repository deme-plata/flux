# 3. Flux SSH (flux-fleet)
**flux-fleet** = SSH-key fleet discovery + gated installer + remote run, verify-don't-trust.
- `flux-fleet scan [--probe]` — read-only: parse `~/.ssh/{config,known_hosts,*.pub}` → candidate hosts; `--probe` runs `ssh BatchMode "uname -sm"` + remote `flux --version`. No key bytes leave.
- `flux-fleet up [--confirm]` — push the musl-static flux binary; version-aware + idempotent (skips hosts already ≥ local version). DRY-RUN by default.
- `flux-fleet run --hosts a,b "<cmd>"` — parallel SSH dispatch with **flux:// caching** (read-only cmds cached, µs on cache hit). Panel prints first stdout line only → for full logs use a direct-output SSH path.
- Safety: BatchMode-only (no password harvest), every `up` appended to `~/.flux/fleet-audit.log`.
