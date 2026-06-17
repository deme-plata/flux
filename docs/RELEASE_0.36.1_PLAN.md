# Release 0.36.1 — Plan (2026-06-10)

Planned with DeepSeek-V4 + two Claude Code research agents (release-candidate survey + deploy-mechanics survey). rocky (claude-fable-5).

## A. Theme
**Security-hardened Flux + instant flux-db cold-open + SIGIL sync-starts-earlier.** A stabilization/hardening release — no new features, no on-disk format change.

## B. Ship list (verified, low-risk unless noted)
| Item | Commits | Risk | Gate |
|---|---|---|---|
| Flux security audit SEC-001..022 (20/22 closed) | 2d7a1839, 28b5f799, 95a99c57, 03fa6cb0, 2cbbfd36 | **HIGH** (security surface) | security-regression suite green + manual probes (SSRF / path-traversal / shell-injection) = 0 penetration |
| flux-db cold-open: WAL truncate-on-flush + lazy SST bodies | 7178f912, cdbf5d45 | MED | cold-open from a ≥1.5 GB WAL ≤ 2s; 100-cycle crash-consistency stress |
| flux-p2p bootstrap-mesh hardening | f7c0767…ef1e076, 1ec588e1 | LOW | mesh reconnect, no id-rotation drop |
| SIGIL v0.35 sync-starts-earlier (S1 O(1) open, S2 CDN, S4 sync-before-TUI, S5 probe) | 1d5a669 | LOW | chronos divergence=0 within 5 min |
| SIGIL sync-engine hardening 0.26→0.35 (anti-OOM, SIGTERM flush, zstd 14×, CP437) | 24c5989, 67cae2a, 4a05b46, 2bda9a2 | LOW | verify "honest sync" readout |
| SIGIL mining (halving emission, VDF-fold, AVX2 grind consensus-safe) | a108333, df105e1, e84c56d | LOW | consensus-safe (R=7 / no retarget) |

## C. Pre-release gates (all must pass on Epsilon, glibc 2.39, before any deploy)
1. Full CI green via `flux_combo` / `flux_fullcheck` (never raw cargo).
2. Security regression: the SEC-001..022 test set + 3 manual exploit probes against a throwaway `fluxc serve`.
3. flux-db cold-open benchmark: fresh ≥1.5 GB WAL → time-to-ready ≤ 2s (REFUSE to ship if > 5s).
4. SIGIL chronos divergence=0 on an isolated clone (NOT the live testnet).
5. RELEASES.md refreshed (SIGIL's is ~33 sigil-top versions stale).

## D. Deploy phases (glibc-safe → Delta, ending in a safe sigil-node restart)
**The glibc blocker is solved by musl-static:** a static-pie `fluxc` already builds here (tokio-uring is gated to gnu-only; ring builds under musl; flux-db is pure-Rust, no rocksdb). Existing artifact: `flux/target/x86_64-unknown-linux-musl/release/fluxc`.

1. **Bump + build (Epsilon):** `flux_version_bump` 0.27.0→0.36.1 → `flux_version_sync` → `cargo build --release --target x86_64-unknown-linux-musl -p fluxc`; `file …/fluxc` MUST say `static-pie linked`.
2. **Publish fluxc:** `fluxc release 0.36.1 --binary target/x86_64-unknown-linux-musl/release/fluxc` (writes `fluxc-v0.36.1-x86_64` + `fluxc-latest.json` to dist-final/downloads).
3. **fluxc → Delta:** scp to `/usr/local/bin/fluxc.new` → keep `.bak` → swap → `fluxc version` == 0.36.1. (Or the auto-update channel if `fluxc-auto-update.service` is confirmed live.)
4. **(only if shipping a new sigil-node)** rsync `flux/` → Delta `/home/orobit/flux/`; `fluxc build --package sigil-node --release` ON Delta (native → glibc 2.36) so it links the WAL-fixed flux-db.
5. **sigil-node deploy:** `scripts/deploy.sh delta <SIGIL_VER>` — verify→apply (atomic, keeps `.bak`)→`systemctl restart sigil-node` (the SAFE restart; **never `pkill -f 'sigil-node start'`** — it kills the whole mesh + self-matches the SSH shell)→journal health check→auto-rollback on panic/preflight_fail(78).
6. **Verify + soak:** version, peers≥1, block production resumes, DB dir grows (WAL fix = durable), WAL no longer balloons. R9 = 24h soak before RC→stable.

**Riskiest step:** the sigil-node restart (drops the live producer until peers≥1). **Rollback:** instant — restore `.bak` (`deploy.sh` does it automatically; <30s). First restart onto the WAL-fixed binary is the first time persistence is exercised on Delta — confirm `SIGIL_DB_PATH` is absolute and watch cold-open.

## E. Explicit non-goals (defer to 0.37 / P1)
- SIGIL C4 unlimited `/onboard` faucet (needs finite faucet-debit + per-IP rate-limit).
- SIGIL C9 bridge value-custody (mint unbound from proof, no replay/spent-set) — bridge must not hold value until fixed.
- SIGIL C10 VDF modulus (`bench_2048` is not a secure RSA modulus) — needs class-group/ceremony.
- SIGIL H1–H7 consensus-crypto wiring (block-apply verifies no PoW/VDF/sig — Phase-0 by design).
- flux-ivc Phase-5 STARK (2 TODO bodies: NTT anchor + Dilithium sig) + flux-lattice-guard 4 Phase-C stubs.
- The 2 remaining (accepted) Flux SEC items (SEC-015 public-read, SEC-017 mock-proof).

## Operator decisions (locked 2026-06-10)
**Two tracks under the 0.36.1 banner — not a conflict:**

1. **Scope = fluxc only.** Cut the fluxc binary (flux workspace → 0.36.1), ship the musl-static binary carrying the security fixes; **do NOT rebuild/restart the Delta sigil-node producer.** Consequence (accepted): fluxc does NOT link flux-db, so the flux-db cold-open fix does NOT reach the running sigil-node in this release — it rides into SIGIL only on the next SIGIL build. Lowest risk, no producer restart.

2. **SIGIL auth gate = FULL migration (Track 2).** Close the live `:8099` anonymous-mint hole properly: deploy the `sigil_rpc::auth` gate (branch `security/audit-hardening-2026-06-10`, `183c53b6`) AND migrate every client (React wallet, sigil-miner `/mine`, scripts) to sign requests, shipped with auth ENFORCED (not the `SIGIL_RPC_NO_AUTH=1` bypass). This is a separate SIGIL deploy track from the fluxc cut — larger, needs the client-signing work + a coordinated sigil-rpcd restart.

## ⚠️ COORDINATION HOLD (2026-06-10)
A **parallel session is mid-cutting flux `0.27.0`** right now: the working tree has uncommitted `Cargo.toml` (`0.26.0→0.27.0`) + `RELEASES.md` ("Surprise Drop") + `main.rs` edits. **Do NOT bump to 0.36.1 or cut the release until 0.27.0 lands** — editing those same files now collides head-on. Sequence: let 0.27.0 commit → then bump 0.27.0→0.36.1 from a clean tree → build musl → publish → deploy. Track 2 (SIGIL auth migration) is independent of this hold and can be scoped/started in the sigil repo in parallel.

## F. Parallelizable (fan out Claude Code agents on a frozen 0.36.1 checkout)
- A1: security-regression suite + exploit probes.
- A2: flux-db cold-open benchmark (repeat from fresh 1.5 GB WAL).
- A3: SIGIL chronos divergence-check (isolated env).
- A4: `fluxc serve` SSRF/path-traversal/CRLF probes.
- A5: per-finding SEC closure re-verification + RELEASES.md refresh.
Do NOT start deploy until every agent signals green and cold-open ≤2s is confirmed.
