# flux-coinbase — Coinbase Advanced Trade, the flux way

One crate, four faces, one safety model:

| face | what | how |
|---|---|---|
| **engine** (`lib.rs`) | CDP-JWT client (ES256 **and** Ed25519 keys), typed market data, order builders, the Verified Execution Gate, append-only ledger, flux-api spec | `Coinbase::from_env()` / `::public()` |
| **CLI** (`flux-coinbase`) | setup · market · account · gated trading · leveraged DCA | `flux-coinbase help` |
| **TUI** (`flux-coinbase tui`) | ratatui trading terminal: order book with depth bars, candlestick chart + SMA20, tape, balances, perp positions with liquidation distance, open orders, DCA panel | `flux-coinbase tui --product BTC-USD [--live]` |
| **MCP** (`flux_coinbase_*` in fluxc-mcp) | 16 tools for Claude Code / any MCP client | `flux_coinbase_status` first |

## The safety model (read before trading)

1. **Nothing is sent without `confirm=true` / `--confirm`.** Every write returns the exact JSON it
   *would* send, plus Coinbase's own **preview** (fees, fill estimate), so what you approve is
   what goes out.
2. **The Gate** (`Gate::default()`): whitelist `BTC-USD ETH-USD BTC-USDC ETH-USDC BTC-PERP-INTX
   ETH-PERP-INTX` · **$100 per order** · **3× max leverage** · limit price within 500 bps of mark
   · **leverage on a spot product is refused**. Widening it is an explicit argument on every call
   (`--max-notional`, `--max-leverage`, `--whitelist`) and is echoed back in the proposal.
3. **Leverage = position notional, not margin.** The gate sees `$margin × leverage`. A $30 ticket
   at 3× is a $90 order and is gated as $90.
4. **Liquidation is printed on every leveraged plan** (`entry × (1 − 1/lev + 1 %)`), and the DCA
   engine refuses a ticket whose liquidation is closer than `min_liq_distance_pct` (default 15 %).
5. **The key never leaves the process.** Only a fingerprint (`organizations/…abc (ES256)`) is ever
   returned. The key file is `/root/.config/coinbase/cdp_api_key.json`, mode 0600.
6. **Ledger**: `/home/storage/claude-code/flux-coinbase/ledger.jsonl` — every proposal, order,
   cancel and DCA tick, append-only.

## Setup (5 minutes)

```
flux-coinbase setup --howto        # where to create the key + which permissions
flux-coinbase setup --file ~/cdp_api_key.json
flux-coinbase status               # fingerprint · can_trade · perps enabled?
```

Create the key at <https://portal.cdp.coinbase.com> → API keys → **Secret API key** with
**View + Trade** and **NOT Transfer** (a key that cannot withdraw cannot be drained), IP-allowlisted
to this box. Either signature algorithm (ECDSA or Ed25519) works.

**Perpetual futures / leverage need an INTX portfolio on the Coinbase account** (Coinbase's own
eligibility, per jurisdiction). `status` reports `perps_enabled`. Spot works regardless.

## Trading

```
flux-coinbase propose buy BTC-USD --usd 25                      # gate + preview, nothing sent
flux-coinbase order   buy BTC-USD --usd 25 --confirm            # REAL
flux-coinbase propose buy BTC-PERP-INTX --usd 60 --leverage 3   # perp, $60 notional = $20 margin
flux-coinbase order   sell BTC-USD --usd 25 --limit 80000 --post-only --confirm
flux-coinbase cancel --confirm                                   # cancel all open
```

## Leveraged DCA

```
flux-coinbase dca plan --product BTC-PERP-INTX --usd 20 --leverage 3 --every-hours 24 --max-position 500
flux-coinbase dca tick  ... --force            # propose one beat now
flux-coinbase dca tick  ... --force --confirm  # place it
flux-coinbase dca run   ... --confirm          # the metronome, forever (systemd it)
flux-coinbase dca state --product BTC-PERP-INTX
```

The **dip tilt**: ticket × (1 + 10·drawdown below SMA200) when under the average, × (1 − 5·premium)
when over; RSI14 < 30 → ×1.5, > 70 → ×0.5; clamped to [0.25, `max_multiplier`]. Never zero — a DCA
that stops buying is a market-timer. State (ticks, avg entry, notional held, history) persists in
`/home/storage/claude-code/flux-coinbase/dca-<product>.json`.

## TUI keys

`1-5`/Tab tabs · `←/→` product · `g` candle granularity · `b`/`s` order form · `d` DCA plan ·
`c` cancel-all · `r` refresh · `q` quit. Form: digits amount · `+`/`-` leverage · `m` market/limit
· `Tab` field · `Enter` gate+preview → `Enter` again places (LIVE only) · `Esc`.

PAPER mode is the default: Enter logs what would be sent. `--live` is the only way Enter can trade.

## MCP tools

`flux_coinbase_status` · `_setup` · `_panel` · `_products` · `_ticker` · `_book` · `_candles` ·
`_accounts` · `_positions` · `_orders` · `_propose` · `_order` (confirm) · `_cancel` (confirm) ·
`_dca_plan` · `_dca_tick` (confirm) · `_tui`.

## Auth details (for the next person who has to debug a 401)

JWT per request: header `{alg: ES256|EdDSA, kid: <key name>, nonce: <16 random bytes hex>, typ: JWT}`,
payload `{sub: <key name>, iss: "cdp", nbf: now, exp: now+120, uri: "GET api.coinbase.com/api/v3/brokerage/accounts"}`
— the `uri` is METHOD + host + path **without the query string**. ES256 signature is raw `r‖s`
(64 bytes), not DER. Both are pinned by unit tests that verify the signature against the public key.
