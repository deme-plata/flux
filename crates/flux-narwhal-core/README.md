# flux-narwhal-core

Narwhal Knight — parallel block production engine for SIGIL. Sharded validators,
batch signature verification, adaptive block sizing, turbo-sync scaffolding.
Targets 500M TPS with 350 shards × 1.45M TPS.

Status: **architecture scaffold, builds warning-clean, 8/8 tests green.**
Verified via `fluxc build --package flux-narwhal-core` + `flux_combo` (T2,
swarm board #157, task rocky-271).

## ⚠️ Simulation vs real crypto — read before trusting any number

This crate is an architecture/throughput scaffold. The following are
**SIMULATED or ASPIRATIONAL**, not real:

| Claim | Reality in this crate |
|---|---|
| "Batch Ed25519 verification (10^6/s hot path)" | `BatchVerifier::verify_batch` does **NOT verify Ed25519**. It computes a BLAKE3 hash and **discards it**, then accepts any signature with ≥1 non-zero byte. A forged tx with `sig_ed25519 = [1u8; 64]` passes. The 10^6/s figure is unmeasured with real crypto — `ed25519_dalek` batch verify is roughly 50–100k sigs/s/core, so the hot-path budget must be re-benchmarked when real verification lands. |
| SQIsign settlement signatures | `NarwhalTx.sig_sqisign` is carried but **never verified anywhere**. The workspace already has `flux-sqisign` (Level 5, 292B) — wire it for the settlement lane. |
| Chain linkage | `ShardBlock.parent_hash` is hardcoded `[0u8; 32]` ("Simplified"). Blocks are not cryptographically chained; no fork choice is possible. |
| "Zero-copy state transitions via accumulator-based roots" | `Shard.state_root` is initialized to zero and **never updated**. No accumulator exists. |
| Validators / BFT | Validator IDs are synthetic index bytes (no keys exist). `BYZANTINE_TOLERANCE` is a constant only — there is no voting, no quorum, no slashing. Consensus is delegated to "DAGKnight" in the diagram but not integrated. |
| "PID · Kalman · Momentum" turbo sync | `compute_sync_rate` is a **P-controller**: `pid_ki` is misused (multiplied by instantaneous pressure, no integral state), `pid_kd` is dead, there is no Kalman filter, `peer_momentum` is never written. |
| 500M TPS | The TPS tests assert **arithmetic** (350 × 1.45M ≥ 500M), not measured throughput. `PER_SHARD_TPS` cites "validated at 1.45M in chronos benchmark" — that benchmark is not in this crate. |

## 🐛 Known real bug (not just simulation)

`MempoolRouter::route` shards on `tx.sender[0] as usize % queues.len()`.
`sender[0]` is a `u8` (0–255), so with the targeted 350 shards, **shards
256–349 can never receive a transaction** — a silent 27% capacity loss.
Fix when wiring real routing: hash the full sender (e.g. first 8 bytes of
BLAKE3) before the modulo.

Minor: `BatchVerifier.avg_verify_us` is never updated (always 0 in stats);
`ProductionStats.avg_block_us` reports the constant floor, not a measurement.

## What IS real

- The sharded data structures, mempool routing/draining, adaptive block
  interval under pressure, and rayon-parallel batch pipeline shape.
- Atomic counters + rolling TPS computation.
- 8 unit tests covering engine creation, routing, batch path, block
  production, and the shard-count arithmetic.

## Path to real

1. Swap the simulated check in `verify_batch` for `ed25519_dalek`
   batch verification; re-measure the per-core rate and recompute
   `PER_SHARD_TPS` / `TARGET_SHARDS` from the measurement.
2. Verify `sig_sqisign` via `flux-sqisign` on the settlement lane.
3. Chain `parent_hash` (BLAKE3 of the previous shard block) and compute a
   real `state_root`.
4. Fix the >256-shard routing bug.
5. Integrate DAGKnight consensus for cross-shard ordering.
