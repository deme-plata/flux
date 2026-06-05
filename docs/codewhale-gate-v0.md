# codewhale-gate v0 — DeepSeek API reseller with 10% markup

> **One-line:** OpenAI-compatible HTTP gateway that forwards to DeepSeek's API, charges 10% more than DeepSeek's published prices, settles in QUG/SIGIL or fiat.

## Scope

A single Rust binary (`codewhale-gate`) listens on `:8090`, accepts the OpenAI `POST /v1/chat/completions` + `POST /v1/embeddings` shape, forwards to `https://api.deepseek.com/v1/...` with the operator's upstream API key, captures the `usage` block from the response, computes `cost = tokens × deepseek_price × 1.10`, debits the user's account, and streams the response back.

Three customer-visible modes:
1. **API-key prepaid** — user pays QUG/USD up front, gateway issues an API key with a balance, debits on use.
2. **Wallet-debit (agentic money)** — user attaches a SIGIL/QUG wallet, gateway emits an on-chain settlement tx after each request.
3. **Stripe-postpay** — Stripe Customer + auto-charge monthly (covers traditional users who don't hold tokens).

## Dep graph

```
codewhale-gate (bin)
├── axum (HTTP)
├── reqwest (upstream)
├── tokio
├── serde_json
├── sqlx + sqlite (usage ledger)
├── flux-quillon-wallet-client    (mode 2 — settle on Quillon)
├── sigil-tx + sigil-net          (mode 2 — settle on SIGIL when shielded)
└── stripe-rs                     (mode 3)
```

## Concrete v0 line list (target ~600 LOC)

```
crates/codewhale-gate/
├── Cargo.toml
└── src/
    ├── main.rs                   ~80   axum router + tokio main
    ├── config.rs                 ~60   ENV: upstream key, listen addr, markup pct, mode
    ├── upstream.rs              ~120   reqwest pool, streaming response forwarder
    ├── usage.rs                  ~80   parse `usage`, compute cost, persist to sqlite
    ├── auth.rs                   ~80   API-key extraction + balance check
    ├── settle/
    │   ├── prepaid.rs            ~60   debit balance row
    │   ├── wallet.rs             ~80   mode 2: emit qnk send_token to operator
    │   └── stripe.rs             ~60   mode 3: stripe usage record
    └── pricing.rs                ~60   deepseek price table + markup math
```

Database schema (sqlite, single file):
```sql
CREATE TABLE api_keys     (key TEXT PRIMARY KEY, owner TEXT, mode TEXT, balance_usd REAL);
CREATE TABLE usage_log    (id INTEGER PRIMARY KEY, key TEXT, ts INTEGER, model TEXT,
                           input_tok INTEGER, output_tok INTEGER, cost_usd REAL, billed INTEGER);
CREATE TABLE settlements  (id INTEGER PRIMARY KEY, key TEXT, mode TEXT, amount_usd REAL,
                           tx_hash TEXT, stripe_id TEXT, settled_at INTEGER);
```

Markup math (`pricing.rs`):
```rust
const MARKUP: f64 = 1.10;
pub fn cost_usd(model: &str, in_tok: u64, out_tok: u64) -> f64 {
    let (in_per_m, out_per_m) = match model {
        "deepseek-chat"     => (0.27, 1.10),  // 2026 published — confirm at deploy time
        "deepseek-reasoner" => (0.55, 2.19),
        _                   => (0.27, 1.10),
    };
    let base = (in_tok as f64 / 1_000_000.0) * in_per_m
             + (out_tok as f64 / 1_000_000.0) * out_per_m;
    base * MARKUP
}
```

## What this v0 deliberately does NOT include

- **Streaming usage accounting** — the OpenAI streaming responses don't include `usage` in every chunk; we count after the stream completes. Fine for v0 since DeepSeek's SSE endpoint returns a final `usage` chunk.
- **Rate limiting** — punted to nginx/Caddy layer or a Phase 2 token-bucket.
- **Multi-tenant admin UI** — operator manages API keys via `codewhale-gate admin add-key` CLI subcommand.
- **Caching/dedupe** — every request hits upstream.

## Open Qs

1. **Settlement currency in mode 2** — QUG, SIGIL native, or operator-deployed CW token? CLAUDE.md memory shows operator wallet on Quillon; defaulting to QUG until SIGIL has a fee bucket.
2. **Public hosting** — `codewhale.quillon.xyz` (q-flux subdomain) or its own VM? Per-request CPU is negligible; a single $5 droplet handles 100 req/s.
3. **Anthropic-compatible endpoint** — also expose `/v1/messages`? DeepSeek doesn't speak the Anthropic shape natively; we'd have to transform. Skip in v0.
4. **Pricing for DeepSeek-V3 release** — if/when DS releases a v3, table needs update; consider a YAML config file instead of compiled-in constants.
5. **Sigil-flavored variant** — would Viktor want a parallel `codewhale-gate-sigil` that bills shielded txs by default? Architecturally identical to mode 2; just changes the settle target.

## Sequencing

Independent of FluxOS + flux-ide. Can ship anytime. Suggest first because it's the only one of the three that brings revenue this week.

— rocky-sigil 🟣
