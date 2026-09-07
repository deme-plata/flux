# flux-soneium — SIGIL on Soneium

Puts wrapped SIGIL (the byte-identical `SigilBridgeWrappedG3` artifact that runs as wSIGIL3 on
Polygon) on **Soneium** — Sony's OP-Stack L2, chain id **1868** — and seeds a Uniswap-V2-style
pool there, so SIGIL can be bought on two chains. Alchemy-first RPC, public fallback.

## Measured 2026-09-08 (read-only, nothing sent)

| | |
|---|---|
| chain id | 1868 (`eth_chainId` = 0x74c), Minato testnet 1946 |
| gas price | ≈ 0.001 gwei (0xf435e wei) — L2 gas is dust |
| ETH/USD | 2,488 — derived from the Sonus WETH/USDC.e pool reserves, no price API |
| router picked | **Sonus** `0xA0133D30…460D3a` → factory `0xdb5d9562…9a4c8`, 49 pairs, 0.93 ETH / 2,318 USDC.e |
| runner-up | `UniswapV2Router02` `0x273F68c2…334320`, 17 pairs, 0.42 ETH / 1,056 USDC.e |
| deploy gas estimate | 883,752 (×1.12 padded; Polygon measured 797,520 for the same bytes) |
| cost, WETH-quoted pool, 100 wSIGIL at $0.001 | **0.000044 ETH ≈ $0.11** (0.00004 of it is the seed itself) |
| operational wallet | `0x39D1D26d59eEbcf1b4b7b4863aD38a6D226F6840` — **0 ETH on Soneium** |
| Alchemy | app `1gruvm76shefbkkp` has Soneium **disabled** — enable at dashboard.alchemy.com/apps/1gruvm76shefbkkp/networks |

## Commands

```
flux-soneium check                       verify RPC, routers, wallet, ETH/USD
flux-soneium plan  --quote weth --seed 100 --seed-supply 100   funding plan, no tx
flux-soneium deploy --seed-supply 100 --yes
flux-soneium pool  --quote weth --seed 100 --yes [--to 0xLP_RECIPIENT]
flux-soneium all   … --yes               both, refuses unless funded
flux-soneium quote 0.001 --quote weth    how much wSIGIL 0.001 ETH buys
flux-soneium relay                       write /root/.config/sigil/soneium/relay.json
flux-soneium status                      live token + pool read
```

Money moves only on `deploy` / `pool` / `all`, only with `--yes`, and only after the funding
plan says the wallet can afford every step (estimate × 1.12 — a padded limit is the same as not
having the money).

## The relay descriptor

`relay.json` is the one file a relayer, wallet or game MCP reads: chain, token, router, factory,
pair, quote asset, selectors (`mint 0x156e29f6`, `burn 0xbcf64e05`, `BurnedTo` topic), the
10→18 decimal shift, and the Polygon sibling. `relay::mint_calldata` / `burn_calldata` mirror
`sigil-relayer` so the same relayer can drive both legs.

`relayer: "masked"` until the operator un-masks `sigil-bridge-relayer`. A pool with a masked
relayer is **built**, not **bridged** — the token is tradeable, not redeemable.
