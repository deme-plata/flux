# flux-chronos — deterministic multiverse simulation for Flux chains

> *"Better than Docker, better than nohup, better than wall-clock testing — because none of those can actually find the bug."*
>
> **Substrate-level test framework**, lives at `flux/crates/flux-chronos/`. Any Flux chain (SIGIL today, sibling chains tomorrow) uses this for its P6 soak + consensus regression + chaos testing. MCP-ready: AI agents author + run + diff scenarios with no human in the loop.
>
> **Authored:** rocky, 2026-05-29. **Status:** Draft scope.

---

## The problem with Docker + nohup + wall-clock tests

What we have today for SIGIL testing:

| Tool | Problem |
|---|---|
| `cargo test` | Single-process unit tests. Won't surface a bug that requires 2 nodes + a network + 50 hours. |
| Docker soak | Runs in wall-clock. 72h soak takes 72h. Catches symptoms, not causes. Each fresh container is a fresh state — no replay, no branching, no determinism. |
| `nohup` background runs | No isolation, no observability beyond grepping logs. State is in RocksDB so you can't snapshot/restore. Race conditions go undetected because every run is different. |
| Real Delta + Epsilon deploys | Production-realistic but slow, expensive (real bandwidth), tied to wall clock, and one bug can corrupt a live node. |

The pattern: **wall-clock tests find the bugs that happen often. Real chain failures happen rarely.** A 72h soak that finds nothing tells you almost nothing. A 100×72h soak in parallel is impractical on real hardware.

What we want: **superhuman observation under controlled time.**

---

## What flux-chronos is

A pure-Rust framework that runs **N simulated Flux chain nodes inside a single process**, with:

1. **Virtual clock** — `clock.advance(Duration::from_hours(72))` returns in milliseconds. No wall-clock waiting.
2. **In-memory gossipsub mesh** — replaces `flux-p2p` at test time. Configurable per-edge latency, packet-loss rate, partition state.
3. **Deterministic RNG** — every node's randomness is seeded from a single scenario seed. Same seed → same execution, always.
4. **Multiverse branching** — fork the universe at any tick. Run 1000 alternate timelines from one checkpoint, each with different fault injection. See which diverge.
5. **Time-travel debug** — every state mutation is logged. Rewind to tick N, single-step forward, inspect every variable at every moment.
6. **Property-based scenarios** — proptest-style randomization over tx-mixes, network conditions, validator behaviors. Run 10000 trials, surface the one that trips an invariant.
7. **Browser-renderable** — QuillonOS module (`flux/crates/quillonos-chronos`, fluxc-compiled to wasm32-wasip1) shows the multiverse tree + timeline + divergence diff live in a browser tab.
8. **MCP-driven** — AI agents author scenarios via `mcp__fluxc__flux_chronos_*` tools: spawn, advance, snapshot, branch, diff, replay.

The combination is the test framework that finds the bugs Docker soaks can't, in seconds.

---

## Comparison

| Capability | Docker | nohup | TigerBeetle/FoundationDB internal sim | **flux-chronos** |
|---|---|---|---|---|
| Wall-clock independent | No | No | Yes | **Yes** |
| 72h soak time | 72h | 72h | seconds | **seconds** |
| Deterministic replay | No | No | Yes | **Yes** |
| Multi-universe branching | No | No | Partial | **Yes (first-class)** |
| Time-travel debug | No | No | Partial | **Yes** |
| Browser visualization | No | No | No | **Yes (QuillonOS)** |
| AI-agent scriptable | No | No | No | **Yes (MCP)** |
| Built on Rust + Flux | — | — | Closed-source | **Yes (dogfooded)** |

TigerBeetle's sim is the closest existing thing; ours adds the multiverse browser + MCP layer.

---

## Architecture

```
flux/crates/
├── flux-chronos/                — core sim engine
│   └── src/
│       ├── lib.rs               — Universe + Scenario API
│       ├── clock.rs             — VirtualClock (tick + advance)
│       ├── net.rs               — in-memory gossipsub mesh + fault injection
│       ├── storage.rs           — in-memory flux-db replacement
│       ├── node.rs              — SimNode trait + the simulated SIGIL node impl
│       ├── multiverse.rs        — branch + diff + merge across alternate timelines
│       ├── recorder.rs          — append-only event log (replayable, branchable)
│       └── property.rs          — proptest-shaped scenario fuzzer
├── flux-chronos-mcp/            — MCP tool exports
│   └── src/lib.rs               — flux_chronos_spawn / advance / branch / diff / record / replay
└── quillonos-chronos/           — browser viz (wasm32-wasip1)
    └── src/main.rs              — multiverse tree renderer, timeline scrubber
```

Three concrete pieces to land in order:

### Phase 1 — `flux-chronos` core (1 day)

```rust
let mut universe = Universe::new(ScenarioSeed::from(42));
let delta   = universe.spawn_node("delta", SigilNodeConfig::default());
let epsilon = universe.spawn_node("epsilon", SigilNodeConfig::default());
universe.connect(delta, epsilon, NetEdge::default()); // 50ms latency, 0 loss

// Inject a tx into delta.
universe.inject(delta, SigilTx::Send { from: alice, to: bob, amount: 100, ... });

// Advance 1 second of simulated time. Returns instantly.
universe.advance(Duration::from_secs(1));

assert_eq!(universe.node(delta).chain_tip(), universe.node(epsilon).chain_tip());
```

Returns whether the universe is *quiescent* (no pending messages, no scheduled ticks) at each advance step. Lets the caller decide when "the chain has caught up."

### Phase 2 — Multiverse branching (1 day)

```rust
let baseline = universe.snapshot();

// Branch 1: Delta crashes at hour 23.
let mut branch_a = baseline.fork();
branch_a.schedule(Duration::from_hours(23), Event::NodeCrash(delta));
branch_a.advance(Duration::from_hours(72));

// Branch 2: Network partitions at hour 5.
let mut branch_b = baseline.fork();
branch_b.schedule(Duration::from_hours(5), Event::Partition(delta, epsilon));
branch_b.advance(Duration::from_hours(72));

let diff = branch_a.diff(&branch_b);
println!("Tip divergence at height: {:?}", diff.first_tip_divergence());
```

Fork preserves all state via copy-on-write. 1000 branches from one baseline is cheap.

### Phase 3 — Property-based fuzzing (1 day)

```rust
flux_chronos::proptest! {
    #[test]
    fn no_balance_can_go_negative(
        tx_mix in random_tx_mix(1000),
        latency in 1u64..1000,
        loss_pct in 0.0f64..0.3,
    ) {
        let mut universe = Universe::new(ScenarioSeed::random());
        universe.spawn_nodes(3);
        universe.set_net_conditions(latency, loss_pct);
        universe.inject_all(tx_mix);
        universe.advance_until_quiescent(Duration::from_hours(24));
        for node in universe.nodes() {
            for wallet in node.all_wallets() {
                assert!(node.balance_of(wallet) >= 0);
            }
        }
    }
}
```

Run 10000 trials in a minute. Surface the one trial that breaks safety.

### Phase 4 — MCP tools (half day)

```
mcp__fluxc__flux_chronos_spawn   — create a fresh universe
mcp__fluxc__flux_chronos_inject  — inject a tx / event into a node
mcp__fluxc__flux_chronos_advance — advance N (simulated) seconds
mcp__fluxc__flux_chronos_snapshot — checkpoint, returns snapshot_id
mcp__fluxc__flux_chronos_branch  — fork from snapshot_id, returns universe_id
mcp__fluxc__flux_chronos_diff    — compare two universes
mcp__fluxc__flux_chronos_replay  — re-run a recorded scenario
mcp__fluxc__flux_chronos_assert  — check invariant across all reachable states
```

AI agents drive scenarios end-to-end. Example: "find a tx mix that breaks SIGIL's state-root invariant within 48h simulated, with packet loss < 10%."

### Phase 5 — QuillonOS browser viz (1-2 days)

Module `flux/crates/quillonos-chronos/` — fluxc-compiled wasm32-wasip1. Runs in a quillon.xyz/os.html tab. Renders:

- **Multiverse tree** — every branch as a node, divergence points highlighted
- **Timeline scrubber** — drag to any tick, see state across all nodes at that tick
- **Divergence diff** — left/right pane showing which mutation diverged between two branches
- **Heatmap** — per-tick per-node CPU + memory + message-count over time

Operator sees the entire scenario as a navigable artifact. AI agents can screenshot + caption ("at tick 1240, Delta committed block 47 but Epsilon had it as an orphan — root cause: gossip arrival order").

---

## What flux-chronos is NOT

- **Not a replacement for real-hardware deploys.** It catches *logic* bugs deterministically; it doesn't catch hardware quirks (disk full, NIC firmware, cosmic-ray bit flips). P6's 72h real-hardware soak is still required — just much shorter, because flux-chronos has already burned through the consensus edge cases.
- **Not a model checker.** TLA+ exhaustively explores; flux-chronos *samples* (via property tests + multiverse branching). For small validator counts (N ≤ 4) we can exhaust message orderings; for larger N we sample. We document the gap explicitly per scenario.
- **Not a real-time simulator.** The clock advances only when explicitly told. Background ticks don't exist. That's the whole point — testing speed is decoupled from wall clock.
- **Not a fuzzer that finds memory bugs.** That's `cargo fuzz` + AFL. flux-chronos finds *protocol* bugs (consensus, balance, state-root).

---

## What this unlocks for SIGIL

For P6: the 72h soak becomes a 72-*second* parallel run of 100 scenarios with random fault injection. If any scenario diverges, we see exactly where + why before deploying to real Delta + Epsilon. Real-hardware soak then runs as confirmation, not exploration.

For P7 (DagKnight + VDF): exhaustive validator-orderings test for small N. The Professor whitepaper's Safety + Liveness proofs become *checked properties*, not just paper claims.

For P8+ (browser wallet + public beta): users can run scenarios in their browser tab to convince themselves the chain is safe before depositing. "Don't trust, verify" extended from cryptography to consensus dynamics.

---

## What this unlocks for Flux as a substrate

flux-chronos becomes a selling point: any chain on Flux gets this for free. Compare to Solana, Ethereum, Cosmos — none of those ship a testing framework like this; they all rely on external (TLA+ in Cosmos's case, Foundry in Eth's case) tooling that doesn't speak the chain's native types.

flux-chronos speaks the chain's types directly because it's Rust + Flux, and the chain's modules import its test API as `[dev-dependencies]`. The test framework IS part of the substrate.

---

## Phased delivery

| Phase | Scope | Est | Owner |
|---|---|---|---|
| CHRONOS-A | core (Universe, SimNode, VirtualClock, in-memory net) + 2-node demo | 1 day | rocky (claiming POC) |
| CHRONOS-B | multiverse fork/diff + recorder + replay | 1 day | open |
| CHRONOS-C | property-based fuzzer | 1 day | open |
| CHRONOS-D | MCP tools (spawn/advance/branch/diff/assert/replay) | half day | open |
| CHRONOS-E | SIGIL adapter (port sigil-node into a SimNode impl) | 1 day | open |
| CHRONOS-F | scenario library — 10 pre-built scenarios (single-producer soak, partition, byzantine-validator, replay-attack, double-spend, ...) | 1-2 days | open |
| CHRONOS-G | QuillonOS browser viz module | 1-2 days | open |
| CHRONOS-H | Property-based regression suite for SIGIL (10000 trials per PR) | half day | open |

**Total: ~7-9 days parallel across 2-3 agents.**

---

## Open questions

1. **Backing storage for in-memory state** — `BTreeMap` everywhere, or a special-purpose CoW trie? CoW makes branch+fork O(1); BTreeMap forces full clone. Vote: *CoW trie via `im` crate; 2-3% perf hit vs BTreeMap, branch is free*.
2. **Determinism boundary** — do we replace `tokio` calls inside chain code with a fake executor, or assume chain code is single-threaded sync? Latter is simpler; former matches reality. Vote: *single-thread sync first; tokio-replacement is a CHRONOS-J stretch*.
3. **Scenario language** — Rust-only, or also DSL (YAML/JSON scenario files)? DSL helps AI agents author from MCP. Vote: *both — Rust API for typechecked scenarios, JSON scenarios for MCP-driven*.
4. **Browser viz first or MCP first?** Viz is the "wow" demo, MCP is the productivity multiplier. Vote: *MCP first (multiplier compounds across all later use), viz as parallel track*.

— rocky 🟠
