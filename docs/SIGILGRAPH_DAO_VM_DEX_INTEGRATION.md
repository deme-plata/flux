# SigilGraph DAO × VM × DEX Integration

Domain: **sigilgraph.fluxapp.xyz** · Network: **sigil-g0**

## Architecture Map

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         MCP Layer (flux repo)                           │
├──────────────────────┬──────────────────────┬─────────────────────────────┤
│ flux-agora-stargate- │ sigil-cosmos-mcp     │ sigil-dao-bridge-mcp (NEW)  │
│ mcp                  │                      │                             │
└──────────┬───────────┴──────────┬───────────┴──────────────┬──────────────┘
           │                      │                          │
┌──────────▼───────────┐ ┌────────▼─────────┐ ┌─────────────▼──────────────┐
│ flux-agora-stargate  │ │ sigil-cosmos-core│ │ sigil-dao-bridge (NEW)     │
│ (provenance+stargate)│ │ (κ citizenship)  │ │                            │
└──────────────────────┘ └──────────────────┘ └─────────────┬──────────────┘
                                                            │
                    ┌───────────────────────────────────────┼───────────────┐
                    │              sigil repo (core)        │               │
                    ├──────────┬──────────┬─────────┬───────▼───┬───────────┤
                    │sigil-    │sigil-    │sigil-vm │sigil-dex  │sigil-state│
                    │council   │treasury  │(WASM)   │(AMM math) │(chokept.) │
                    │(DAO)     │(payouts) │         │           │           │
                    └──────────┴──────────┴─────────┴───────────┴───────────┘
```

## Crate Inventory

| Crate | Repo | Role | Status |
|-------|------|------|--------|
| `sigil-council` | sigil | DAO governance (1-of-2 / 2-of-2 + franchise votes) | **Live** |
| `sigil-treasury` | sigil | Council-gated payouts, MAX-WINS sync | **Live** |
| `sigil-vm` | sigil | Deterministic WASM VM (wasmi in VM-1) | **Scaffold** |
| `sigil-dex` | sigil | Constant-product AMM pure math | **Live** |
| `sigil-state` | sigil | `commit_state_transition` chokepoint | **Live** |
| `sigil-tx` | sigil | Tx → StateMutation mapping | **Live** |
| `flux-agora-stargate` | flux | Agora provenance × Stargate ingest profile | **Live** |
| `sigil-cosmos-core` | flux | κ-phase citizenship ritual | **Live** |
| `sigil-dao-bridge` | sigil | **NEW** — wires DAO→DEX/VM→state | **Live** |
| `sigil-dao-bridge-mcp` | flux | **NEW** — MCP registry + demo | **Live** |

## Integration Points

1. **Governance → Treasury**: `sigil-dao-bridge::execute_passed_action(TreasuryPayout)` calls `sigil-treasury::payout` only when `sigil-council` proposal `Outcome::Passed` and `Risk::MoneyOrConsensus`.

2. **Governance → DEX**: Passed `DexSwap` actions run `sigil-dex::swap`, emit `StateMutation::SwapDelta`, commit via `sigil-state::commit_state_transition`.

3. **Governance → VM**: Passed `VmContractCall` invokes `sigil-vm::execute` (scaffold until VM-1); state writes fold into `SetContractSlot` mutations.

4. **Composite root**: `dao_composite_root(council, treasury)` = BLAKE3(`sigil-dao-bridge-v0.1` || gov_root || treasury_root).

5. **Testnet**: Registry at `/dao-vm-dex-registry.json` on sigilgraph.fluxapp.xyz.

## Related Stargate Examples

- `sigil/crates/sigil-state/examples/stargate_dag.rs`
- `sigil/crates/sigil-state/examples/stargate_1m.rs`
- `sigil/crates/sigil-state/examples/stargate_50m.rs`
- `sigil/crates/sigil-state/examples/stargate_500m.rs`

## MCP Tools

| Function | Crate |
|----------|-------|
| `flux_sigil_dao_vm_dex_bundle` | sigil-dao-bridge-mcp |
| `flux_sigil_dao_vm_dex_registry_json` | sigil-dao-bridge-mcp |
| `flux_sigil_dao_vm_dex_demo` | sigil-dao-bridge-mcp |
| `flux_sigil_dao_vm_dex_bridge_hint` | sigil-dao-bridge-mcp |
| `flux_agora_stargate_registry_json` | flux-agora-stargate-mcp |
| `flux_sigil_cosmos_citizenship_ritual` | sigil-cosmos-mcp |

## Next Steps

1. **VM-1**: Wire `wasmi` in `sigil-vm::execute()` — unlocks real `VmContractCall` commits.
2. **Publish registry**: Run `cargo run -p sigil-dao-bridge-mcp --example gen_registry` → deploy to sigilgraph.fluxapp.xyz.
3. **sigil-tx ContractDeploy/ContractCall**: Connect VM bytecode deploy to mempool (VM-3).
4. **STARK binding**: Attach transition proofs at `commit_state_transition` (Phase 3).
5. **Citizenship → franchise**: Map `sigil-cosmos-core` tiers to `sigil-council` vote weights.
## fluxc-mcp Registration (2026-06-06)

Tools registered in `flux/crates/fluxc-mcp/src/handlers/dao_vm_dex.rs`:

| Tool | Handler |
|------|---------|
| `flux_sigil_dao_vm_dex_combo` | compile + test sigil-dao-bridge |
| `flux_sigil_dao_vm_dex_bundle` | testnet bundle JSON |
| `flux_sigil_dao_vm_dex_registry_json` | registry JSON generator |
| `flux_sigil_dao_vm_dex_demo` | in-memory governance + VM demo |
| `flux_sigil_dao_vm_dex_bridge_hint` | operator crate map |
| `flux_sigil_dao_vm_dex_deploy_testnet` | write registry to dist-fluxapp |

Registry live at: `https://sigilgraph.fluxapp.xyz/dao-vm-dex-registry.json`