# Flux Release Pipeline — flux-sigil-releases & fluxc release

> **SIGIL ledger:** `flux/crates/flux-sigil-releases/`  
> **Publisher:** `fluxc release` → `fluxc-core/p2p_worker.rs::publish_release_product`  
> **MCP:** `flux_release_publish`, `flux_release_check`  
> **Deploy doc:** `AUTO_UPDATE_DEPLOY.md`

Two release channels serve different purposes:

1. **flux-sigil-releases** — append-only JSONL ledger for the 100-iteration SIGIL wallet plan (static page polls it)
2. **fluxc release** — binary publish channel for auto-update consumers (Delta, arena clients, etc.)

---

## flux-sigil-releases

### Purpose

Append-only ledger for SIGIL wallet development phases A..J (100 slots: `0.1.0`..`1.0.9`). The public site (`sigil-releases.html` on sigilgraph.fluxapp.xyz) polls the JSONL every 5s and fires Web Notifications on new entries. **No backend process** — the ledger IS the substrate.

### Data model

```rust
Release {
  phase: char,           // A..J
  version: String,       // e.g. "0.1.0"
  title: String,
  status: Shipped | InFlight | Pending | Aborted,
  settled_qug: Option<f64>,
  ts_ms: u64,
  notes, url, agent
}
```

Phases:

| Phase | Name | Version range |
|-------|------|---------------|
| A | Bootstrap & Inventory | 0.1.0–0.1.9 |
| B | Visual Identity | 0.2.0–0.2.9 |
| ... | ... | ... |
| J | Launch + Post-Launch | 1.0.0–1.0.9 |

### CLI (`sigil-releases`)

```bash
sigil-releases ship PHASE VERSION TITLE [--qug N] [--notes ...] [--url ...] [--agent NAME]
sigil-releases in-flight PHASE VERSION TITLE [--notes ...]
sigil-releases list [--phase A]
sigil-releases backfill    # writes known Phase A entries
sigil-releases dump        # full 100-grid as JSON
```

Ledger path: `$SIGIL_RELEASES_PATH` or default `/home/orobit/q-narwhalknight/dist-final/sigil-releases.jsonl`

### Build

```bash
fluxc build --package flux-sigil-releases
```

---

## fluxc release (binary publish)

### CLI

```bash
fluxc release [VERSION] [--product NAME] [--binary PATH]
```

Defaults:

- `product` = `fluxc`
- `binary` = current executable
- `version` = workspace `CARGO_PKG_VERSION`

### What it does (`publish_release_product`)

1. Read binary bytes (source exe or `--binary` path)
2. Compute `sha256_hex` + `blake3_hex`
3. Write `${product}-v${version}-${arch}` to downloads dir (atomic tmp → rename)
4. Write `${product}-latest.json` manifest:

```json
{
  "product": "fluxc",
  "version": "0.18.0",
  "url": "https://sigilgraph.fluxapp.xyz/downloads/fluxc-v0.18.0-linux-x86_64",
  "sha256_hex": "...",
  "blake3_hex": "...",
  "size_bytes": 12345678,
  "released_at_us": 1780000000000000,
  "publisher": "root@epsilon",
  "publisher_wallet_hex": "...",
  "notes": ""
}
```

### Environment overrides

| Variable | Default |
|----------|---------|
| `FLUX_DOWNLOADS_DIR` | `/home/orobit/q-narwhalknight/dist-final/downloads` |
| `FLUX_RELEASE_URL_BASE` | `https://sigilgraph.fluxapp.xyz/downloads` |
| `FLUX_RELEASE_PUBLISHER` | `$USER@$(hostname -s)` |
| `FLUX_AGENT_WALLET` | qnk wallet hex |
| `FLUX_RELEASE_NOTES` | free-form string |

### Multi-product channel

```bash
# flux-arena client zip
fluxc release --product flux-arena --binary /path/to/flux-arena-v0.1.0.zip 0.1.0

# UE5 dedicated server binary
fluxc release --product flux-arena-server --binary /path/to/server-bin 0.1.0
```

Each product gets its own `*-latest.json` — no collision.

---

## MCP wrappers

```json
{"name":"flux_release_publish","arguments":{"product":"fluxc","version":"0.18.0"}}
{"name":"flux_release_check","arguments":{"product":"fluxc"}}
```

`flux_release_check` fetches manifest, reports version + URL + size — no download.

---

## Auto-update consumer (Delta)

From `AUTO_UPDATE_DEPLOY.md`:

| Server | Role | Update |
|--------|------|--------|
| Epsilon (89.149.241.126) | Publisher | Manual: `fluxc release` after `fluxc build` |
| Delta (5.79.79.158) | Consumer | `fluxc auto-update --apply --interval 60` (systemd) |

Publisher recipe:

```bash
cd /home/storage/deepseek-codewhale/flux
./target/debug/fluxc build --package fluxc
./target/debug/fluxc release
```

Consumer (non-fluxc products):

```bash
FLUX_MANIFEST_URL=https://sigilgraph.fluxapp.xyz/downloads/flux-arena-server-latest.json \
  fluxc auto-update --apply --interval 60
```

Verification: HTTPS pull + sha256 check + atomic binary swap. Optional libp2p gossip notification is stubbed in `p2p_worker.rs`.

---

## End-to-end release flow

```
Agent ships SIGIL wallet milestone
  → sigil-releases ship A 0.1.3 "API URL swap" --qug 0.5 --url https://sigilgraph.fluxapp.xyz/...
  → sigil-releases.jsonl appended
  → sigil-releases.html polls → Web Notification

Agent ships fluxc binary
  → fluxc build --package fluxc
  → fluxc release 0.18.0
  → fluxc-latest.json + binary in downloads/
  → Delta auto-update pulls on next tick
```

---

## Tests

`flux-sigil-releases` tests: canonical slot validation, 100-grid build, latest-per-slot selection, grid stats, JSONL roundtrip.

— Documented from `flux-sigil-releases`, `p2p_worker.rs`, `AUTO_UPDATE_DEPLOY.md`, 2026-06-06.