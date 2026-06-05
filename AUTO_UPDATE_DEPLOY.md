# fluxc auto-update — deployment recipe

> First deployed: 2026-05-29 evening (rocky), v0.15.0.
> Architecture: HTTPS pull from quillon.xyz/downloads/fluxc-latest.json + sha256 verification + atomic binary swap. Optional libp2p gossip notification layer is stubbed out (see `p2p_worker.rs`); not required for the demo.

## Server topology

| Server | Role | Binary path | Update strategy |
|---|---|---|---|
| Epsilon (89.149.241.126) | Publisher + bootstrap | `/home/storage/deepseek-codewhale/flux/target/debug/fluxc` | Manual: `fluxc release` after each `fluxc build` |
| Delta (5.79.79.158) | Consumer | `/usr/local/bin/fluxc` (planned; currently `/tmp/fluxc-deb12`) | Auto: `fluxc auto-update --apply --interval 60` as systemd unit |

## Publisher recipe (Epsilon)

### fluxc itself

```bash
cd /home/storage/deepseek-codewhale/flux
./target/debug/fluxc build --package fluxc       # ~25s incremental
./target/debug/fluxc release                     # publishes self's binary as v$(CARGO_PKG_VERSION)
# Or: ./target/debug/fluxc release 0.15.1        # publish with explicit version override
```

### Multi-product channel (v0.16.0+)

The release tool now accepts `--product NAME` and `--binary PATH` so any artifact
goes through the same channel. The on-disk manifest is `${product}-latest.json`
so multiple products coexist without collision.

```bash
# Publish flux-arena client (brother's download)
fluxc release --product flux-arena --binary /path/to/flux-arena-v0.1.0.zip 0.1.0
# → writes flux-arena-latest.json and flux-arena-v0.1.0-linux-x86_64
# → reachable at https://quillon.xyz/downloads/flux-arena-latest.json

# Publish flux-arena UE5 dedicated server
fluxc release --product flux-arena-server --binary /path/to/server-bin 0.1.0
# → writes flux-arena-server-latest.json and flux-arena-server-v0.1.0-linux-x86_64
```

### Consumer recipe for non-fluxc products

`fluxc auto-update` defaults to product `fluxc`. To consume a different product,
override the manifest URL. Multiple systemd units can coexist.

```bash
# Pull flux-arena-server updates onto a UE5 dedicated server host
FLUX_MANIFEST_URL=https://quillon.xyz/downloads/flux-arena-server-latest.json \
fluxc auto-update --apply --interval 60
```

A natural extension is `fluxc auto-update --product NAME` — see the [Pending
items](#whats-still-pending) below.

### MCP wrappers (callable from any agent)

```jsonc
mcp__fluxc__flux_release_publish
  product: "flux-arena"
  version: "0.1.0"
  binary_path: "/path/to/binary"   // optional; default: current fluxc exe

mcp__fluxc__flux_release_check
  product: "flux-arena"            // fetches flux-arena-latest.json
```

Side effects:
- Copies the running binary to `/home/orobit/q-narwhalknight/dist-final/downloads/fluxc-v$VER-musl`
- Writes `/home/orobit/q-narwhalknight/dist-final/downloads/fluxc-latest.json` with `{version, url, sha256_hex, blake3_hex, size_bytes, released_at_us, publisher, publisher_wallet_hex, notes}`
- q-flux serves both at `https://quillon.xyz/downloads/...` immediately (no restart needed)

Env overrides:
- `FLUX_DOWNLOADS_DIR` — change destination dir (default: dist-final/downloads)
- `FLUX_RELEASE_URL_BASE` — base URL in manifest (default: `https://quillon.xyz/downloads`)
- `FLUX_RELEASE_PUBLISHER` — publisher name in manifest (default: `$USER@$(hostname -s)`)
- `FLUX_AGENT_WALLET` — qnk wallet in manifest (links release to a swarm agent)
- `FLUX_RELEASE_NOTES` — free-form notes string

## Consumer recipe (Delta)

### Bootstrap (one-time)

```bash
# Build a Delta-compatible fluxc binary. Two paths:
# A) Build on Delta directly (preferred; no GLIBC mismatch):
ssh root@5.79.79.158
cd /root/flux                         # rsync'd source from Epsilon
cargo build --release --package fluxc # ~15-25 min cold, <2 min warm
install -m 755 target/release/fluxc /usr/local/bin/fluxc
# B) Or grab the latest manifest binary (works only if Epsilon builds for Debian12):
# curl -fsSL https://quillon.xyz/downloads/fluxc-latest.json | jq -r .url | xargs curl -fsSL -o /usr/local/bin/fluxc
# chmod +x /usr/local/bin/fluxc
```

### Daemon (steady-state)

```bash
# Systemd unit (creates the loop that pulls every minute and applies on version bump)
cat >/etc/systemd/system/fluxc-auto-update.service <<'EOF'
[Unit]
Description=fluxc auto-updater — pulls new releases from quillon.xyz
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/local/bin/fluxc auto-update --apply --interval 60
Restart=on-failure
RestartSec=10
StandardOutput=append:/var/log/fluxc-auto-update.log
StandardError=append:/var/log/fluxc-auto-update.log
# Optional: link release events to a swarm wallet for provenance
# Environment=FLUX_AGENT_WALLET=qnk...
# Environment=FLUX_MANIFEST_URL=https://quillon.xyz/downloads/fluxc-latest.json

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable --now fluxc-auto-update
journalctl -u fluxc-auto-update -f   # watch the loop
```

### Manual (no systemd)

```bash
# One-shot check: print state, exit
fluxc auto-update --interval 60        # downloads but doesn't apply (no --apply)
fluxc auto-update --apply --interval 60 # apply on version bump; runs forever

# Env-var equivalents (useful for containers):
FLUX_AUTOUPDATE_INTERVAL_SECS=30 FLUX_AUTOUPDATE_APPLY=1 fluxc auto-update
```

## End-to-end test loop

```bash
# Epsilon: bump version
cd /home/storage/deepseek-codewhale/flux
# Edit Cargo.toml: workspace.package.version = "0.15.1"
./target/debug/fluxc build --package fluxc       # recompile binary embedding new version
./target/debug/fluxc release                     # publish

# Delta: watch the pickup
journalctl -u fluxc-auto-update -f               # within 60s expect:
#   🔔 new release available: v0.15.0 → v0.15.1
#   ✓ sha256 matches (177942304 bytes)
#   🚀 12:45:00 — applied v0.15.1 → /usr/local/bin/fluxc
#   Restart the process / systemd unit to start using the new binary.

# Verify swap
ssh root@5.79.79.158 "/usr/local/bin/fluxc version"   # → fluxc 0.15.1
```

## Safety properties

- **sha256 verified** — manifest carries the hex digest; auto-update fails closed if mismatch.
- **Atomic rename** — `std::fs::rename` (or fallback `copy` on cross-filesystem) ensures the binary is never half-replaced.
- **Restart required to USE the new binary** — auto-update itself keeps running on the OLD code path; systemd `Restart=on-failure` doesn't pick up the new exec unless something kills the process. Either add `Restart=always` + `KillMode=process` to roll on a timer, or just `systemctl restart fluxc-auto-update` after each release.
- **No new deps** — uses curl subprocess for HTTP (same pattern as `webhook.rs`).

## What's still pending

1. **SQIsign signature in manifest** — currently `notes` and `blake3_hex` populated, but no signed bundle. Compose with `fluxc-core::provenance` to sign the manifest with the agent's SQIsign Level 5 key on release. Consumer verifies before downloading.
2. **libp2p gossip notification** — publish to `/flux/1/release` topic so Delta picks up new releases in <1s instead of 60s. Stub is in `p2p_worker.rs::run_p2p_worker`.
3. **Rollback** — keep N previous binaries on disk so `fluxc auto-update --rollback` swaps back. Today it's a one-way ratchet.
4. **Differential updates** — for 177 MB debug binaries, ship bsdiff patches instead of the full binary. Probably not worth it until we ship release-mode musl-static at ~10-20 MB.
