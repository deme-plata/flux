# CHRONOS-T — transport adapter (flux-p2p ⟷ SimNode)

> *"Same chain code, two transports: deterministic in-memory for sim, real flux-p2p for the wire."*
>
> **Authored:** rocky, 2026-05-29 late. **Status:** building.

## The idea

`flux_chronos::SimNode::step(now, incoming) -> { publish, events, wake_at }` is already transport-agnostic — it consumes envelopes and emits envelopes, never touching a socket. CHRONOS-T exploits that: a `Transport` abstraction with two impls.

```
                    ┌──────────────────────────┐
                    │   SigilSimNode (REAL      │
                    │   apply_tx + commit +     │
                    │   sigil-bank pipeline)    │
                    └────────────┬─────────────┘
                       step(now, incoming) → publish
                                 │
         ┌───────────────────────┴───────────────────────┐
         ▼                                                ▼
  InMemoryTransport                              RealP2pTransport
  (flux-chronos Universe,                        (flux-p2p NetworkManager,
   virtual clock, deterministic)                  wall clock, real libp2p)
         │                                                │
   72h soak in 0.46s                          Delta ⟷ Epsilon over
   exhaustive, reproducible                   /sigil/g0/blocks, real bytes
```

The same `SigilSimNode` runs under both. Sim finds the logic bugs fast; the wire confirms the bytes actually move + bootstrap/NAT/gossip-delivery work. Diff the two → any discrepancy is a transport bug, not a logic bug.

## Transport trait

```rust
pub trait Transport {
    /// Publish a payload (already TAG-prefixed) to all peers on a topic.
    fn publish(&self, topic: &str, payload: Vec<u8>);
    /// Drain every payload received since last poll, with its topic.
    fn poll(&mut self) -> Vec<(String, Vec<u8>)>;
    /// Peer count (for readiness gating — don't produce before a peer is up).
    fn peer_count(&self) -> u32;
}
```

- **`RealP2pTransport`** wraps `flux_p2p::NetworkManager`: `publish` → `nm.publish(topic, data)`; `poll` → filter `nm.drain_events()` for `SwarmAppEvent::GossipsubMessage { topic, data, .. }`.
- (`InMemoryTransport` is the existing Universe bus; CHRONOS-T doesn't refactor it — the sim path already works. The trait exists so the *driver* loop is identical across both.)

## Driver loop (real transport, wall clock)

```
loop {
    incoming = transport.poll()                     // drain real gossipsub
    envelopes = incoming.map(to_envelope)           // synthesize from/to
    result = node.step(now_micros(), &envelopes)    // REAL chain logic
    for env in result.publish.dedup_by_payload() {  // N peers → 1 topic publish
        transport.publish(TOPIC_BLOCKS, env.payload)
    }
    log(result.events)
    sleep(block_time / 4)                            // poll faster than block cadence
}
```

NodeId routing collapses to topic broadcast on the wire: the producer's per-peer `publish` envelopes carry identical payloads, so the driver dedups + publishes once. Incoming gossip becomes an `Envelope { from: synthetic, to: self, payload }` — `SigilSimNode.step` dispatches on the TAG byte regardless of NodeId, so it Just Works.

## Genesis must match

Both nodes build the SAME genesis (`sigil_chronos::demo_genesis()` — fixed master + funded wallets) so they share tip at H=0. Divergence from H=0 would mean a genesis-determinism bug, caught immediately.

## Run recipe (Delta producer ⟷ Epsilon follower)

```bash
# Delta (producer, listens):
SIGIL_LISTEN=/ip4/0.0.0.0/tcp/9501 \
  sigil-chronos-net producer --blocks 50

# Epsilon (follower, dials Delta):
SIGIL_LISTEN=/ip4/0.0.0.0/tcp/9501 \
SIGIL_BOOTSTRAP=/ip4/5.79.79.158/tcp/9501/p2p/<delta-peer-id> \
  sigil-chronos-net follower
```

Expected: follower logs `apply H=1 Ok … apply H=50 Ok`, `divergence=0`.

## What this closes

The recurring "is it actually tested over the wire?" gap. After CHRONOS-T, the sim run + the wire run share ONE code path (`SigilSimNode`). The sim proves the logic exhaustively; the wire proves the transport. No more "tested locally, never on the network."

— rocky 🟠
