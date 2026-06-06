# Flux Aether — Distributed File Mixer

> **Crate:** `flux/crates/flux-aether/`  
> **Status:** Core shard/reassemble + SOV sync + v2 PIR lane  
> **Canonical domain:** [sigilgraph.fluxapp.xyz](https://sigilgraph.fluxapp.xyz)

Flux Aether is a live distributed **mixer for files**. Each file is chunked, erasure-coded (K-of-N), per-shard encrypted, content-addressed, and mixed into one indistinguishable shard pool scattered over `flux-p2p`. The coherent file exists **HERE** (you hold the manifest), **THERE** (shards live on peers), and **NOWHERE** (assembled only transiently on read).

First target: WAV/MP3 raw bytes (format-aware chunking is a later lane).

---

## Core model (`aether.rs`)

### FileBlock — structured like a SIGIL block header

| Field | Role |
|-------|------|
| `content_root` | BLAKE3 of original file (≈ state root) |
| `shard_merkle_root` | Merkle root over shard CIDs (≈ `txs_merkle_root`) |
| `k`, `n` | Data shards + total (K data + parity) |
| `producer` | Agent who sharded the file (provenance `.proof` + SQIsign sig wired in AETHER-2) |
| `nonce` | Per-file encryption nonce |

### Shard

Encrypted bytes + content-address (`cid = BLAKE3(encrypted_bytes)`). Indistinguishable from any other shard in the pool — reveals nothing about file membership or content.

### Operations

```rust
shard_file(data, shard_size, key, producer) -> (FileBlock, Vec<Shard>)
reassemble(fb, available_shards, key) -> Result<Vec<u8>, AetherError>
```

Current keystone ships **K data + 1 XOR parity** (recover any single loss). General K-of-N via Reed-Solomon is in `rs.rs` (`reed-solomon-erasure` crate).

Encryption: BLAKE3-keystream XOR (AEAD is the production upgrade). Wrong key → `ContentMismatch` at reassembly.

---

## Modules

| Module | Purpose |
|--------|---------|
| `aether.rs` | Shard / mix / reassemble core |
| `rs.rs` | Real K-of-N erasure (wider blast-radius durability) |
| `sov.rs` | Version sync: `plan`, `sync_pair`, `gossip_until_converged`, `mesh_status_json` |
| `v2.rs` | Merkle root helpers, PIR query/answer/reconstruct, `TimeLock`, `verify_storage` |

### SOV sync (`sov.rs`)

Manifest-based version convergence across mesh nodes:

- `Manifest`, `VersionEntry`, `NodeIdentity`
- `apply_download`, `divergence`, `plan`, `sync_pair`
- `gossip_until_converged` — mesh-wide convergence loop

---

## Flux Aether control surface (operator UI)

Separate from the Rust crate: the **Flux Aether control surface** is a local Node server (`flux-aether-control-surface`) that exposes auditable HTTP endpoints for MCP webhooks, activity, presence, and operator events.

Default: `http://127.0.0.1:4178` (or `PORT` env).

Key endpoints used by MCP combo scripts:

| Endpoint | Purpose |
|----------|---------|
| `POST /api/mcp-webhook` | Visible MCP combo results |
| `POST /api/activity` | Agent activity heartbeat |
| `POST /api/operator/style` | Operator style learning |
| `POST /api/event` | Control-room diary events |

Combo scripts under `tools/mcp-combos/` POST to these endpoints — no covert channels. Inspect `captures/*.jsonl` for durable audit trails.

---

## MCP integration

`fluxc mcp` exposes Aether-aware search combos (see [FLUX_MCP_COMBOS.md](./FLUX_MCP_COMBOS.md)):

- `flux_aether_search_combo` — index Vizily + Aether-facing Flux crates, then query
- Shell gate: `tools/mcp-combos/flux_combo_fluxc_search.sh` — SSH to Epsilon, runs `fluxc mcp` with search probes

Environment for the search combo:

| Variable | Default |
|----------|---------|
| `FLUX_EPSILON_HOST` | `epsilon` |
| `FLUX_REMOTE_ROOT` | `/home/storage/deepseek-codewhale/flux` |
| `FLUX_SEARCH_INDEX` | `target/flux-search/index.json` |
| `FLUX_SEARCH_REINDEX` | `0` (set `1` to reindex before probe) |

---

## Philosophy tie-in

From [FLUX_FOUNDATION_PHILOSOPHY.md](./FLUX_FOUNDATION_PHILOSOPHY.md): files become blocks — the same integrity gate that verifies a SigilGraph block verifies a stored file. Aether is the storage substrate where truth is computed and verified, not promised.

---

## Build

```bash
fluxc build --package flux-aether
cargo test -p flux-aether   # via fluxc test --package flux-aether
```

Proof binaries: `bin/durability-proof.rs` exercises Reed-Solomon K-of-N recovery.

— Documented from `flux-aether` source, 2026-06-06.