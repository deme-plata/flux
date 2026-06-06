# Flux Rev — Content-Addressed Version Control

> **Crate:** `flux/crates/flux-rev/`  
> **Status:** Working core + P2P sync daemon  
> **Philosophy:** Aligns with [FLUX_FOUNDATION_PHILOSOPHY.md](./FLUX_FOUNDATION_PHILOSOPHY.md) — *probatione, non fide*: revisions are keyed by BLAKE3 hashes, not branch conventions.

Flux Rev is Flux-native, content-addressed version control — the git replacement for the swarm. It exists because mutable branch-centric git has been fragile on this cluster (stale `main`, diverged branches, deployed artifacts with no pushed branch). Flux Rev is the opposite by construction.

---

## Design principles

| Git model | Flux Rev model |
|-----------|----------------|
| Mutable branches, merge conflicts | Content-addressed objects; identical content ⇒ identical hash everywhere |
| Push/pull over remotes | Object exchange over `flux-p2p` gossipsub |
| Provenance via branch history | Genesis stamp + revision body hash on every snapshot |
| 3-way merge | No merge — peers either hold object X or request it |

### Object types

- **Blob** — raw file bytes, keyed by BLAKE3 hash
- **Manifest** — sorted `path → {hash, mode}` snapshot; keyed by canonical JSON hash
- **Genesis** — one-time import stamp (`imported_from`, `workspace_version`, `author`, `ts_unix`, `note`)
- **Revision** — `{parent, manifest, genesis, author, ts, message}` hashed into `id`; stored at `id` (body hash, not full struct hash)

Store layout: `<workdir>/.flux-rev/objects/<blake3hex>`. HEAD is a plain file pointing at the current revision id.

Skipped directories: `.git`, `.flux-rev`, `target`, `node_modules`, `dist`, `build`, `.cargo`, `__pycache__`, `.venv`, `venv`, `.next`, `out`, `.target-shared`.

---

## CLI (`flux-rev`)

```bash
flux-rev genesis <dir> [--from <src>] [--version <v>] [--note <s>]
flux-rev snapshot <dir> [-m <msg>]
flux-rev checkout <dir> <revid> [--into <dest>]
flux-rev log <dir>
flux-rev diff <dir> <a> <b>
flux-rev head <dir>
```

Environment:

- `FLUX_REV_AUTHOR` — author string on new revisions (default: `claude-desktop-viktor`)

`genesis` stamps the canonical import and creates the first revision. `snapshot` walks the working tree, stores blobs + manifest + revision, updates HEAD. If nothing changed, HEAD is unchanged.

---

## Mesh sync (`flux-rev-sync`)

Binary: `flux-rev-sync` — propagates revisions over **real** `flux-p2p` gossipsub.

```bash
flux-rev-sync <dir> --port <P> [--peer <multiaddr>] [--seconds <N>] [--node <id>] [--watch] [--watch-secs <N>]
```

Wire protocol on topic `/flux-rev/sync/v1`:

1. **Announce** — HEAD + object closure (transitive hashes the revision needs)
2. **Want** — missing hashes
3. **Have** — `{hash, hex-bytes}` — receiver verifies `verify_object(hash, bytes)` before storing

Integrity gate: `store_verified()` rejects tampered objects. A peer cannot inject bytes that do not hash to the claimed address.

Flags:

- `--watch` — auto-snapshot working dir on change (default interval 3s)
- `--seconds 0` — run forever (daemon mode)

Two nodes on two ports with `--peer` pointing at each other = the 2-node propagation proof.

---

## Relationship to flux-vcs

`docs/flux-vcs-spec.md` covers GitHub-shaped hosting (repos, invites, OAuth2) for `code.sigilgraph.fluxapp.xyz`. Flux Rev is the **substrate VCS** for workspace snapshots and mesh propagation. Future: flux-vcs can crawl repo HEADs into `flux-search`; Flux Rev can feed canonical source snapshots into that index.

---

## Quick start (Epsilon)

```bash
cd /home/storage/deepseek-codewhale/flux
fluxc build --package flux-rev

# Import canonical workspace
./target/debug/flux-rev genesis . --from epsilon --version 0.18.0

# Cut a revision after edits
./target/debug/flux-rev snapshot . -m "docs: FLUX_REV.md"

# Propagate over P2P (30s demo)
./target/debug/flux-rev-sync . --port 9100 --seconds 30
```

---

## Tests

Unit tests in `lib.rs`, `sync.rs`, and `flux-rev-sync` integration cover: manifest hashing, `verify_object`, message roundtrips, `missing()` closure, tamper rejection, and watch/snapshot behavior.

— Documented from `flux-rev` source, 2026-06-06.