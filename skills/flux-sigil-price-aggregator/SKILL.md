---
name: flux-sigil-price-aggregator
description: Develop and verify the Flux/Sigil price bridge across Bitunix futures, 0x EVM swaps, Solana instructions, and cross-chain routes; interpret market measurements and native Sigil DEX/VM boundaries.
---

# Flux/Sigil price aggregator

Source: `/home/storage/deepseek-codewhale/flux/crates/flux-bitunix/src/{aggregator,bridge}.rs`.
Load Flux Dev for build and shared-tree rules. Use `flux_combo {package:"flux-bitunix"}` after edits.
The read-only `aggregator_probe` example checks discovery, Base pricing, Bitunix tickers, and a signed account read without printing account data.

## Routes on the existing operator bridge

- `GET /prices?symbols=BTCUSDT,ETHUSDT`: Bitunix futures measurements. Add `chainId`, `sellToken`, `buyToken`, and integer `sellAmount` for a separate 0x EVM quote.
- `GET /0x/capabilities`: live EVM chains and directed cross-chain provider pairs. Each provider has its own success/error status. Do not turn discovery failure into an empty supported-chain list.
- `GET /0x/chains`, `/0x/sources?chainId=…`, `/0x/crosschain/sources`: provider discovery.
- `GET /0x/price` and `/0x/quote`: EVM Permit2; quote also requires `taker`.
- `GET /0x/crosschain`: `originChain`, `destinationChain`, `sellToken`, `buyToken`, `sellAmount`, `originAddress`, `destinationAddress`; optional `sortQuotesBy=price|speed`. Preserve directed bridge eligibility.
- `GET /0x/crosschain/status`: `originChain`, `originTxHash`, optional `quoteId`.
- `POST /0x/solana/instructions`: `amount_in` (decimal string accepted), `taker`, `token_in`, `token_out`, optional `slippage_bps` and `recipient`. Returns instructions; it does not broadcast.

The `/bitunix` and `/desk` mount prefixes are also accepted. Keys stay on the server using existing configuration. Bitunix public prices need no signing; signed reads use the existing key. Never embed either provider's key in a wallet or skill.

## DEX and VM interpretation

Token identity is chain plus contract/mint address, not ticker. There is no fixed 7-million-token promise or local token whitelist. Quoteability depends on liquidity, provider eligibility, amount, route, and key access.

Use live `/swap/chains` for EVM coverage and `/cross-chain/sources` for bridge directionality. Solana uses its own instructions endpoint. Tron and HyperCore use cross-chain routes; do not send their IDs to EVM Permit2. The provider remains authoritative for new chains.

Native SIGIL `sigil-dex` is constant-product math. `sigil-vm` source currently implements a deterministic wasmi VM; the older `flux_sigil_dao_vm_dex_bridge_hint` scaffold text is stale. Neither native component executes EVM calldata or Solana instructions. Native state writes remain behind `sigil-state::commit_state_transition`. External quotes are not native consensus state or verified oracle attestations.

Return a proposal to the appropriate chain wallet. Preserve its allowance/Permit2 requirements, minimum received, issues, gas and fees; a successful HTTP response is not permission or proof of execution. Only settlement on the relevant chain proves completion.

## Measurements

Keep futures last/mark prices and USDT volumes separate from DEX executable quotes. Report venue, market type, observation time, response latency, provider timestamp when available, and errors. Observation time is not source price freshness. Missing metrics are null. Keep base units exact; never pass amounts through JavaScript Number. Do not infer market cap, APY, TVL, price impact or token count from an indicative quote.

Current API references (recheck when changing adapters):
- https://docs.0x.org/docs/introduction/supported-chains
- https://docs.0x.org/api-reference/evm-ap-is/swap/chains
- https://docs.0x.org/api-reference/cross-chain-ap-is/cross-chain/cross-chain-list-sources
- https://docs.0x.org/api-reference/solana-swap-ap-is/swap/instructions
