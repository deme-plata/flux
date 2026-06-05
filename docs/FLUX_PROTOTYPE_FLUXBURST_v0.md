# ⚡ Prototype FLUXBURST — the self-paying, *trustless* build mesh

> `fluxc build` that auto-bursts compilation onto rented boxes over flux-p2p, where each crate is compiled by a **quorum of workers that must agree on the output hash** (each signing a `.proof`) — so a malicious rented box can't inject a backdoor without being caught.
>
> Viktor directive, 2026-05-30. Builds directly on [[project-flux-compute-fabric-vast]] (gateway, nodeswarm, cross-compile, autostop) + the SIGIL divergence primitive.

## The invention — Verifiable Build Consensus (VBC)

distcc / sccache / Bazel-RBE give you **speed but require trusting the workers**. FLUXBURST gives speed **+ cryptographic trust**, by reusing SIGIL's safety primitive:

- SIGIL: *"state divergence is impossible to hide"* — 4 roots committed, exit-78 on mismatch.
- FLUXBURST: *"build divergence is impossible to hide"* — N workers compile the same unit; **if their artifact hashes disagree, someone tampered → reject + slash.** The `.proof` binds *who* built each unit; quorum agreement proves *what* they built.

That is trustless compilation on stranger hardware — not shipped by anyone today.

```
  fluxc build (local saturated)
       │  split dep-graph into compile-units (reuse `fluxc xray` batches)
       ▼
  ┌──────────────── flux-p2p mesh ────────────────┐
  │  unit-A → worker1 ┐                            │
  │  unit-A → worker2 ┼─ each: musl-compile +      │
  │  unit-A → worker3 ┘  return artifact + .proof  │
  └───────────────────────┬────────────────────────┘
       │  VBC: do the hashes AGREE?
       ├── yes → cache + link (trusted)         SIGIL ⇄ pay workers
       └── no  → REJECT + flag byzantine worker (slash)
       │
       ▼  idle workers → flux_vast_autostop reaps them
```

## Lanes (each is FLUXFOOD + an MCP combo)

- **BURST-1** · `flux-burst` crate: dep-graph → compile-unit DAG (reuse `xray` build batches). workspace.deps + shared target + mold from line 1. ~2d
- **BURST-2** · worker: receive a unit over flux-p2p, musl-compile (reuse `flux_cross_compile`), return artifact + SQIsign `.proof` (reuse `flux provenance`). ~2d
- **BURST-3** · **VBC verifier**: quorum hash-agreement + `.proof` verify; divergence → reject + flag (reuse the exit-78 divergence pattern). **The make-or-break lane.** ~3d · *claimed by rocky*
- **BURST-4** · `flux_burst` MCP combo: one call = plan units → `flux_vast_search/create` (rent K verified boxes) → `flux_nodeswarm` spawn workers → distribute → VBC → `flux_vast_autostop` reap → settle SIGIL. Composes the whole compute fabric. ~2d
- **BURST-5** · chronos sim: inject a **byzantine worker** (tampered artifact); prove VBC catches it deterministically before spending a cent. ~2d

## Reuse vs new

**Reuse (the FLUXFOOD payoff):** `flux_vast_*` · `flux_nodeswarm_*` · `flux_cross_compile` (musl workers) · `flux_vast_autostop` · `flux provenance .proof` · flux-p2p (640 Mbps proven) · content-hash cache · chronos. **New:** only `flux-burst` + the VBC verifier. The prototype is mostly composition of substrate this session already shipped.

## The honest hard part — reproducibility

Rust builds aren't bit-reproducible by default (paths, timestamps, codegen-unit order), so naive hash comparison fails. VBC only needs it *relative* across workers on the **same toolchain + flags + source-hash**:
- `--remap-path-prefix` + `SOURCE_DATE_EPOCH` + `-Cmetadata` pinning + 1 codegen unit on the verified path.
- Residual nondeterminism → fall back to **rmeta-hash quorum** (interface, not full binary) or **random re-audit** (recompile 1-in-K units locally, compare). Quorum + audit buys trust without perfect reproducibility.

Tackle this in **BURST-3 first, in chronos**, before renting real boxes.

## Phases
1. **Local mesh** — unit-DAG + 3 localhost workers → prove distribution + VBC agreement.
2. **flux-p2p mesh** — workers on Epsilon↔Delta.
3. **Rented mesh** — Vast workers via the gateway, autostop + SIGIL settle.
4. **Byzantine catch** — chronos injects a tampered worker; VBC rejects deterministically. *That's the demo.*

— rocky 🟠 2026-05-30
