# Agentic-Money Boilerplates for Flux Dev

Forkable starter crates for building **agentic-money agents** on Flux/SIGIL —
autonomous loops that move real funds, made *safe* by construction. Fork one,
swap in your strategy, ship.

This is an **isolated nested workspace**: the parent `flux` workspace globs only
`crates/*`, and the `[workspace]` block here makes this dir its own root, so
forking/building these never touches the main flux build.

```
flux/templates/agentic-money/
  kit/            agentic-money-kit — the reusable core (lib)
  safe-trader/    DEX swap loop, every action through the gate
  wallet-onboard/ local-seed wallet bootstrap → student + nation onboard
  webhook-agent/  event-driven: webhook POST → gated money action
  llm-trader/     open-model LLM proposes → gate → execute
```

## The keystone: Verified Execution Gate

Every template routes its money action through `kit::gate::evaluate`. It is the
ONLY path from an agent's *proposal* to an on-chain action, and it never trusts
the proposer. Five checks, each a real incident someone already paid for:

1. **Direction whitelist** — only actions you explicitly allowed.
2. **Honeypot block** — tokens you'll never trade (easy to buy, impossible to sell).
3. **Amount clamp** — fat-finger sizes clamped into `[min, max]`.
4. **Balance check** — never spend more than you hold.
5. **Slippage ceiling** — real constant-product price-impact, reject thin-pool drains.

> Fork rule: **ADD checks, never remove.** The gate is where your risk policy lives.

## The kit modules

| Module | What it gives you | Lesson baked in |
|--------|-------------------|-----------------|
| `gate` | Verified Execution Gate | flux-hundred A100 honeypot incident |
| `rpc`  | std-only HTTP client for `sigil-rpcd` (no tokio/reqwest/TLS) | single tiny static binary, drops on any fleet box |
| `wallet` | local-seed bootstrap (spendable, deterministic address) | fixes `create_wallet` no-mnemonic dead-drop trap |
| `llm`  | open-model tool-call decide + lenient parse | deepseek-r1 has no ollama tools + `<think>` starves json; qwen is tool-native; verify `/api/tags` serving first |

Crypto-agility (Stargate discipline): `wallet::derive_signing_key` is the single
seam for real keypair derivation — wire your `flux-eternal-cypher` scheme there,
never hardcode a signature algorithm into agent code.

## Build & run

```bash
# from this directory — fluxc, never raw cargo (dogfood + content-hash cache)
fluxc build                      # all crates
fluxc build -p safe-trader       # one template
fluxc test  -p agentic-money-kit # the gate/wallet/llm unit tests

# run against a local sigil-rpcd (127.0.0.1:8099)
./target/release/safe-trader     http://127.0.0.1:8099 8
./target/release/wallet-onboard  http://127.0.0.1:8099
./target/release/webhook-agent   127.0.0.1:8777 http://127.0.0.1:8099
./target/release/llm-trader      http://127.0.0.1:11434 qwen2.5:32b 6 http://127.0.0.1:8099
```

## Wiring into flux-dev

These templates assume a `sigil-rpcd` money daemon on `127.0.0.1:8099` (the
std-only DEX/onboarding daemon — `sigil/crates/sigil-rpc`). For LLM decisions,
point `llm-trader` at any ollama/vLLM endpoint. For swarm coordination, an agent
forked from these registers via `flux_swarm_register` and reports earnings by
role (leader / auditor / builder) per the swarm ledger convention.

Demo ids (`TRADER`/`POOL`/`USDS`/`WQUG`) match a freshly-bootstrapped
`sigil-rpcd`. Replace them with your wallet (from `wallet-onboard`) and your
pools for a real deployment.
