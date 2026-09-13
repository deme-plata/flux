# What SIGIL taught the compiler — Flux retro, 2026-09-13

Branch `v0.5.2-dev`, fluxc 0.41.0. Two weeks of SIGIL work (g2 cutover → shielded sends →
100 blk/s) used Flux as the build, serve, release and coordination layer for every step.
This is the list of places where Flux itself was the thing that cost time, ranked by what
it cost, each one traced to a dated incident, and each one either fixed in this commit,
already fixed (verified, not assumed), or deferred with a reason.

The pattern across all of them: **a tool that answers "fine" when the truth is "absent" is
worse than one that fails.** Every expensive incident below is a 200 where a 404 belonged,
a green where an UNVERIFIED belonged, or a panic where a quiet exit belonged.

## Ranked

| # | What cost time | Incident | Cost | Status |
|---|---|---|---|---|
| 1 | serde attributes bincode can encode but never decode | sigil-header ×2 (08-15), sigil-tx + sigil-state + block backfill wire (09-11) — a follower sat **6 M blocks** behind on a payload that arrived complete | days of a stalled node, 5 occurrences | ✅ **`fluxc wire-audit` + `flux_wire_audit`** (this commit) |
| 2 | `flux_release_check` could not read a SIGIL manifest | `missing field sha256_hex` (09-02); manifests looked up on quillon.xyz for a sigilgraph.org product | the one tool that should answer "is the live release genuine" answered nothing | ✅ tolerant schema, sigil-* → sigilgraph.org, **Ed25519 verify against the pinned key** (this commit) |
| 3 | SPA fallback answered POSTs and JSON clients with the landing page | 4 chronos step events into `/webhooks/rocky-dev` = black hole; `flux_webhook_list` counted 42 "live" receivers that were all the fallback (09-09) | silent data loss, false "live" | ✅ fallback = GET/HEAD + `Accept: text/html` only (this commit) |
| 4 | `fluxc … \| grep -q` → `panicked: Broken pipe` → "verify failed" on a VALID signature | sigil-top v8.0.7 release aborted after a 10-min build (09-06) | one release attempt, two scripts patched | ✅ SIGPIPE → default for one-shot subcommands (this commit) |
| 5 | Build timings on a saturated box read as cache behaviour | "`--tests` evicts the cache" rule written from 13 s → 48 s measured under a release build; refuted 09-06 | a wrong rule in CLAUDE.md for four days | ✅ `flux_combo` prints `LOAD 1m x / N cores` + verdict (this commit) |
| 6 | `flux_combo` reported `0/0 ✓ green` when the test binary failed to compile | multiple sessions shipped on it | — | ✅ already fixed: three-way verdict, `UNVERIFIED` (verified in `test_combo.rs` today) |
| 7 | `fluxc serve` read one 8 KiB buffer → HTTP 413 on every 210 KB shielded proof | no phone or web wallet could pay through sigilgraph.org (09-05) | days of "send doesn't work" | ✅ already fixed `58bc38a5` (8 MiB, deployed 09-06, byte-identical error vs `:18181`) |
| 8 | fluxc-mcp read SIGIL API bodies through a 10 MB `into_string` cap | every `flux_sigil_shielded_*` tool died on the 18 MB `/v1/shielded/leaves` (09-12) | MCP money tools blind | ✅ already fixed `b003228a` (256 MB reader) — needs an MCP restart |
| 9 | `RUSTC_WRAPPER` identity differed per checkout → agents evicted each other's cache | 33 % hit rate over 112 k units | every build slower for everyone | ✅ already fixed `b6ab1262` (`~/.flux/bin/fluxc` default identity) |
| 10 | flux-p2p routed consensus topics through `entangled_publish`, which skipped the publish | block/vote gossip silently dropped (09-12) | the 100 blk/s lane's first wall | ✅ already fixed `c4422b23` |
| 11 | flux-graph refused the whole SIGIL workspace: dev-dependency cycles (cargo-legal) were build edges | `fluxc xray` → "Cycle detected: crate involves dependency loop" on the sigil tree (found today while running #1 there) | xray / agility / api_generate / wire-audit blind on SIGIL | ✅ `Dependency.dev` excluded from the DAG; the error names the loop (this commit) |

## Measured today, so nobody re-chases it

- **The public "200 text/html for a missing `.json`" on sigilgraph.org is q-flux, not fluxc.**
  `fluxc serve` on `:8459` answers `GET /nope-2026.json` → **404** and `POST /webhooks/x` → **404**
  already; the 200 appears only after the q-flux vhost (`/home/orobit/q-narwhalknight/q-flux.toml`,
  `static_root` + `proxy_paths=["/","/v1"]`) — Quillon-side source, out of scope for a SIGIL/Flux
  session. What fluxc serve DID still do wrong: `GET /webhooks/sigil-x` (no extension, no Accept)
  → 200 landing page. Fixed here for non-navigations; a bare `curl` with no `Accept` still gets the
  page, because refusing `*/*` would break every browser landing on a deep link.
- `fluxc --help` exits **0** and `fluxc version | head -1` exits 0 — the `--help exits non-zero`
  trap in CLAUDE.md is sigil-top's, not fluxc's.
- The live sigil-top manifest carries `blake3_hex`, `channel`, `flux_rev`, `source_tag`, `targets`
  and no `sha256_hex` — that exact body is now a unit test in `p2p_worker.rs`.
- Box state during this retro: load 1m **54** on 48 cores (xmrig 23 cores, sigil-miner 4,
  q-api-server 3). Every timing in this session is a queue measurement.

## The audit, run on the SIGIL tree (2026-09-13, after fix #11)

`fluxc wire-audit --root /home/storage/deepseek-codewhale/sigil` → 95 crates, 42 with
bincode in scope, 367 files: **4 direct, 20 review, 0 allowed.** Each direct one was read:

| finding | verdict |
|---|---|
| `sigil-tx/src/lib.rs:102` `SigilTx` `#[serde(tag = "kind")]` | known — a test pins that bincode CANNOT decode it, it never travels on that wire → needs `// flux-wire: allow` |
| `sigil-top/src/block_sync/mod.rs:69` `SyncMsg` `#[serde(tag = "t")]` | JSON topic message → allow marker |
| `sigil-node/src/main.rs:84,89` `BackfillReq` `skip_serializing_if` ×2 | JSON request (`serde_json::from_slice`, "old servers ignore the field") → allow marker; the second was added TODAY by another lane, uncommitted |
| review: `sigil-events/src/lib.rs:56` `SigilEvent` `#[serde(tag = "kind")]` | **the 09-11 incident type**, reachable via sigil-node/sigil-top bincode — now on MessagePack behind `SIGILM1`, still needs the marker so the next reader knows why it is safe |

So on SIGIL the lint's first run is: no new bug, four markers to write, and the one type that
did break the chain is on the list. That is the intended shape — the audit is a reading
list with a reason on every line, not a verdict.

## What is still pretend

- `fluxc wire-audit` is a **source scan**, not a type check. It flags the attribute wherever a
  codec is in scope; it cannot prove the type travels on that wire. That is what the
  `// flux-wire: allow` marker is for — the sigil tree will need it on `SigilTx` (internally
  tagged, travels only on MessagePack behind `SIGILM1`, pinned by test). Those markers belong to
  a sigil session; this one does not touch claimed sigil crates.
- `flux_release_check` verifies the **manifest** signature. It does not download the artifact,
  so it does not check the artifact's `blake3_hex` — `fluxc verify-proof` is that step.
- The SIGPIPE change makes fluxc die quietly (exit 141) instead of panicking (exit 101). Under
  `set -o pipefail` a pipeline still fails either way — scripts must keep capturing output first
  (`out=$(fluxc …) || fail`). The change removes the backtrace and the wrong error text, not the
  shell semantics.

## Deferred (needs a design or the operator)

1. **"Committed but never called."** `spend_full_v5`, `with_next_index`, `balance_smt.rs`,
   `TokenDeploy` — four fixes that shipped with zero callers. A `fluxc callers <fn>` / "pub fn
   with no in-workspace caller" audit would catch the class. Needs a symbol index, not a grep.
2. **`flux_sigil_node_deploy` should compare `systemctl show -p ExecStart` with the running
   `/proc/PID/exe`** before calling anything deployed — `systemctl cat` lied on 09-10 and a 7-min
   build went to the wrong target dir.
3. **Release version collision.** Two agents released sigil-top v8.0.6 within minutes (09-06).
   A release lock keyed on the LIVE manifest version (refuse ≤ live) belongs in
   `flux_release_publish`, but the SIGIL script owns that ceremony today.
4. **A producer cannot be rolled back out of its own blocks** (09-10). Not a Flux bug; the
   compiler's contribution would be a `chronos` gate that runs header-vs-body agreement before a
   consensus-affecting binary is allowed near the live producer.

## Commit

Path-scoped (`git add <file>` only — the tree carries 2,466 dirty files from other lanes):
`crates/flux-graph/src/{wire_safety.rs,lib.rs,graph.rs,manifest.rs,resolver.rs,build_order.rs,agility.rs}`, `crates/fluxc/src/main.rs`,
`crates/fluxc-serve/src/serve_router.rs`, `crates/fluxc-core/src/p2p_worker.rs`,
`crates/fluxc-mcp/src/handlers/{ops.rs,test_combo.rs}`, this file, CHANGELOG.
Verified with `flux_combo` per crate; provenance stamped with `flux-rev snapshot` per crate.
