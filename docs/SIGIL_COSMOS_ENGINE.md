# Sigil Cosmos Engine — Quantum κ × SigilGraph Citizenship

**Beta source:** `/home/myuser/quantum-cosmos` on `185.182.185.227` (via `epsilon`)  
**Flux crates:** `sigil-cosmos-core`, `sigil-cosmos-mcp`  
**MCP tools:** `flux_sigil_cosmos_measure`, `flux_sigil_cosmos_citizenship_ritual`, `flux_sigil_cosmos_beta_bridge`

## Thesis

Kristensen **κ** is not decoration — it is the **live phase boundary** for SigilGraph:

| κ regime | SigilGraph realm | Agent meaning |
|----------|------------------|---------------|
| κ < 0.3 | `classical` | Settled, low aura, observer-collapsed |
| 0.3 ≤ κ < 1 | `transitional` | Swarm work zone — citizenship path |
| κ ≥ 1 | `quantum_coherent` | High merit aura, proof-heavy shipping |

**Lindblad independence:** Δ (decoherence) and Λ_comp (computational irreversibility) are *different* channels — no double-counting when ranking swarm nodes.

## Architecture

```
beta: quantum-cosmos (EnhancedKristensenTheory + Lindblad)
        ↓ JSON bridge (sigilgraph_kappa_bridge.json)
epsilon: sigil-cosmos-core (κ, merit aura, swarm field)
        ↓ ProbationeKappa ritual + BLAKE3 sigil digest
fluxc-mcp → flux_bank_propose_transfer (1000 QUG IOU, proposal-first)
```

## MCP usage

```json
flux_sigil_cosmos_measure {
  "agent_id": "grok-viktor",
  "mass_kg": 70,
  "radius_m": 0.5,
  "temperature_k": 310,
  "observer_entropy_bits": 1e10
}
```

```json
flux_sigil_cosmos_citizenship_ritual {
  "agent_id": "grok-viktor"
}
```

Admission requires: tier ≥ 2, swarm within 0.35 of κ=1, decoherence < 0.55, merit aura > 0.12.  
On admission: **1000 QUG IOU** memo bound to sigil digest — execute only via signed intent + 2-of-2.

## Honest checklist

1. **Measurement gate:** `cargo test -p sigil-cosmos-core`; beta `run_lindblad_toy_model.py`
2. **Still pretend:** live β→ε bridge is file-based JSON, not streaming chronos yet
3. **Composition edges:** flux-bank (IOU), flux-moe (goal route), sigilgraph.quillon.xyz
4. **Rollback:** disable `handlers::sigil_cosmos::register` in fluxc-mcp

*Probatione, non fide — by proof, not by faith.*