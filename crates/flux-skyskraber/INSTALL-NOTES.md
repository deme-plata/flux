# Quillon datacenter node

Source additions for Epsilon's `/home/storage/flux/crates/flux-skyskraber`.

The `datacenter` Cargo feature adds a runnable `quillon-datacenter` binary without changing the existing tower simulation CLI. Run from the Flux root:

```sh
bash crates/flux-skyskraber/scripts/start-datacenter.sh
```

The launcher builds through Flux, creates a config only if absent, and runs in the foreground. Stop with Ctrl+C. The default connects to Epsilon's existing SIGIL full node at `http://127.0.0.1:18181/v1/network/topology`, checks `/sigil/g2/` topic membership, and never opens or rewrites that node's chain database. It does not start another validator or claim independent block verification. On other machines, configure a reachable SIGIL node first.

HTTP listens on 127.0.0.1:19470; a separate real Flux P2P listener uses 127.0.0.1:19471. Private deployment only: no authentication or production network hardening in this pilot. Bootstrap peers must be configured explicitly. A node with zero connected peers is a working local worker, not a replicated cluster.

```sh
curl -fsS http://127.0.0.1:19470/status
curl -fsS http://127.0.0.1:19470/ready
curl -fsS -H 'Content-Type: application/json' -d '{"payload":"hello Quillon","rounds":100}' http://127.0.0.1:19470/jobs/hash
./target/debug/quillon-datacenter plan
./target/debug/quillon-datacenter audit crates/flux-skyskraber/datacenter.json
```

Jobs are bounded local BLAKE3 hash-chain diagnostics, not model inference, GPU FLOPS, or globally scheduled remote execution. Requests exceeding the concurrency budget are rejected with 429. Aether stores verified receipt shards locally within a quota; successful gossip queueing is not proof of peer delivery. No token is charged and no transaction is signed. The refactor audit is advisory; Cortex's upstream performance validation is simulated and labeled as such. The wrapper never uses it to claim measured acceleration or change consensus settings.

Physical estimates are assumption-driven and account for grid bottlenecks separately from robot-assisted construction. See the accompanying whitepaper.
