//! flux_bitunix_* — the Bitunix Futures trading surface as one-call MCP combos, plus the on-chain
//! wSIGIL/USDC leg. Executed by the `flux-bitunix` engine; credentials stay server-side and are
//! never returned in a tool result.
//!
//! # Safety contract
//! Reads are free. **Writes are propose-only unless `confirm: true` is passed**, and every write
//! must additionally clear the Verified Execution Gate (symbol whitelist · notional cap · leverage
//! cap · price-deviation bound). A proposal shows the exact JSON body that a confirm would send, so
//! a human is never asked to approve something they cannot see.
//!
//! Every proposal, order and cancel is appended to an append-only ledger and dispatched to the
//! webhook bus, so an agent loop (and the operator) always knows what was attempted and by whom.

use serde_json::{json, Value};
use crate::handlers::{ToolDef, ToolRegistry};
use flux_bitunix::{Bitunix, Gate, Verdict, qty_from_notional, pool};
use fluxc_webhooks::webhook;

const EV_PROPOSAL: &str = "bitunix_proposal";
const EV_ORDER: &str = "bitunix_order";
const EV_POOL: &str = "bitunix_pool";
const EV_PANEL: &str = "bitunix_panel";
const EV_ARB: &str = "arb_scan";

fn s<'a>(a: &'a Value, k: &str) -> &'a str { a.get(k).and_then(|v| v.as_str()).unwrap_or("") }
fn f(a: &Value, k: &str, d: f64) -> f64 { a.get(k).and_then(|v| v.as_f64()).unwrap_or(d) }
fn u(a: &Value, k: &str, d: u64) -> u64 { a.get(k).and_then(|v| v.as_u64()).unwrap_or(d) }
fn b(a: &Value, k: &str, d: bool) -> bool { a.get(k).and_then(|v| v.as_bool()).unwrap_or(d) }
fn out(r: Result<Value, String>) -> String {
    match r { Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_default(),
              Err(e) => serde_json::to_string_pretty(&json!({"ok": false, "error": e})).unwrap_or_default() }
}

fn ledger_path() -> String {
    std::env::var("FLUX_BITUNIX_LEDGER")
        .unwrap_or_else(|_| "/home/storage/claude-code/flux-bitunix/ledger.jsonl".to_string())
}

/// Append-only record of everything that touched money. Never overwritten, never rotated by us.
fn record(v: &Value) {
    use std::io::Write;
    let p = ledger_path();
    if let Some(dir) = std::path::Path::new(&p).parent() { let _ = std::fs::create_dir_all(dir); }
    if let Ok(mut fh) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(fh, "{}", serde_json::to_string(v).unwrap_or_default());
    }
}

fn emit(event: &str, v: Value) -> Value {
    record(&v);
    webhook::auto_dispatch(event, v.clone());
    let mut v = v;
    v["webhook_event"] = json!(event);
    v["ledger"] = json!(ledger_path());
    v
}

/// Build the gate from tool args, falling back to the tight defaults. Widening it is an explicit,
/// visible act — the effective gate is echoed in every proposal.
fn gate_from(a: &Value) -> Gate {
    let mut g = Gate::default();
    if let Some(w) = a.get("whitelist").and_then(|v| v.as_array()) {
        let list: Vec<String> = w.iter().filter_map(|x| x.as_str()).map(|s| s.to_uppercase()).collect();
        if !list.is_empty() { g.whitelist = list; }
    }
    if let Some(m) = a.get("max_notional_usd").and_then(|v| v.as_f64()) { g.max_notional_usd = m; }
    if let Some(l) = a.get("max_leverage").and_then(|v| v.as_u64()) { g.max_leverage = l as u32; }
    g
}

fn gate_json(g: &Gate) -> Value {
    json!({"whitelist": g.whitelist, "max_notional_usd": g.max_notional_usd,
           "max_leverage": g.max_leverage, "max_price_deviation_bps": g.max_price_deviation_bps})
}

/// The whole trading surface in one call — what a slider UI polls.
fn panel(symbols: &str) -> Result<Value, String> {
    let syms = if symbols.is_empty() { "BTCUSDT,ETHUSDT" } else { symbols };
    let pubc = Bitunix::public()?;
    let tickers = pubc.tickers(syms).unwrap_or_else(|e| json!({"error": e}));
    // account/positions need credentials; a missing key must degrade, not explode
    let (account, positions, orders, key_fp) = match Bitunix::from_env() {
        Ok(b) => (b.account("USDT").unwrap_or_else(|e| json!({"error": e})),
                  b.positions("").unwrap_or_else(|e| json!({"error": e})),
                  b.pending_orders("", 20).unwrap_or_else(|e| json!({"error": e})),
                  b.key_fingerprint()),
        Err(e) => (json!({"error": e.clone()}), json!({"error": e.clone()}), json!({"error": e}),
                   "<no key>".to_string()),
    };
    Ok(emit(EV_PANEL, json!({
        "ok": true, "venue": "bitunix-futures", "base": flux_bitunix::BASE,
        "key": key_fp, "symbols": syms,
        "tickers": tickers, "account": account, "positions": positions, "pending_orders": orders,
        "gate": gate_json(&Gate::default()),
        "note": "reads only. Placing an order needs flux_bitunix_order with confirm=true and a passing gate.",
    })))
}

/// Gate + size + build the exact body. Sends nothing.
fn propose(a: &Value) -> Result<Value, String> {
    let symbol = s(a, "symbol").to_uppercase();
    let side = s(a, "side").to_uppercase();
    let usd = f(a, "usd", 0.0);
    let lev = u(a, "leverage", 1) as u32;
    let otype = { let t = s(a, "orderType"); if t.is_empty() { "MARKET".to_string() } else { t.to_uppercase() } };
    let price = a.get("price").and_then(|v| v.as_str()).map(|x| x.to_string());
    let g = gate_from(a);

    let b = Bitunix::from_env()?;
    let mark = b.last_price(&symbol)?;
    let plimit = price.as_ref().and_then(|p| p.parse::<f64>().ok());
    let verdict = g.check(&symbol, &side, usd, lev, plimit, Some(mark));
    if let Verdict::Reject(why) = &verdict {
        return Ok(emit(EV_PROPOSAL, json!({
            "ok": false, "gate": "REJECTED", "reason": why, "gate_config": gate_json(&g),
            "symbol": symbol, "side": side, "usd": usd, "leverage": lev, "mark_price": mark,
            "note": "nothing was sent. Adjust the request, or widen the gate deliberately."
        })));
    }
    let qty = qty_from_notional(usd, plimit.unwrap_or(mark), 4)?;
    let order = Bitunix::build_order(&symbol, &side, &otype, &qty, price.as_deref(),
        Some("GTC"), b_reduce(a), None, opt(a, "tp"), opt(a, "sl"));
    Ok(emit(EV_PROPOSAL, json!({
        "ok": true, "gate": "PASS", "gate_config": gate_json(&g),
        "mark_price": mark, "qty": qty, "notional_usd": usd,
        "would_send": order, "endpoint": "POST /api/v1/futures/trade/place_order",
        "note": "PROPOSAL ONLY — nothing sent. Call flux_bitunix_order with the same args plus confirm=true.",
    })))
}

fn b_reduce(a: &Value) -> bool { b(a, "reduceOnly", false) }
fn opt<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k).and_then(|v| v.as_str()).filter(|s| !s.is_empty())
}

/// The one tool that can move money. Refuses without `confirm: true` AND a passing gate.
fn order(a: &Value) -> Result<Value, String> {
    let confirm = b(a, "confirm", false);
    let p = propose(a)?;
    if p["ok"] != json!(true) { return Ok(p); }
    if !confirm {
        let mut p = p;
        p["confirm_required"] = json!(true);
        return Ok(p);
    }
    let body = p["would_send"].clone();
    let b_cli = Bitunix::from_env()?;
    let res = b_cli.place_order(&body, true)?;
    Ok(emit(EV_ORDER, json!({
        "ok": true, "confirmed": true, "sent": body, "result": res,
        "gate_config": p["gate_config"].clone(), "mark_price": p["mark_price"].clone(),
    })))
}

fn cancel(a: &Value) -> Result<Value, String> {
    let symbol = s(a, "symbol").to_uppercase();
    let confirm = b(a, "confirm", false);
    let ids: Vec<String> = a.get("orderIds").and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|x| x.as_str()).map(|s| s.to_string()).collect())
        .unwrap_or_default();
    let b_cli = Bitunix::from_env()?;
    let r = if ids.is_empty() { b_cli.cancel_all(&symbol, confirm)? }
            else { b_cli.cancel_orders(&symbol, &ids, confirm)? };
    Ok(emit(EV_ORDER, json!({"ok": true, "action": "cancel", "symbol": symbol,
                             "orderIds": ids, "confirmed": confirm, "result": r})))
}

fn pool_tool(a: &Value) -> Result<Value, String> {
    let which = s(a, "pair");
    let pair = match which {
        "" | "wsigil3" | "current" => pool::PAIR_WSIGIL3,
        "legacy" | "old" => pool::PAIR_WSIGIL_LEGACY,
        other => other,
    };
    let mut r = pool::pool_report(pair)?;
    r["which"] = json!(if pair == pool::PAIR_WSIGIL3 { "wsigil3 (current)" }
                       else if pair == pool::PAIR_WSIGIL_LEGACY { "legacy wSIGIL — still referenced by the live market page" }
                       else { "custom" });
    Ok(emit(EV_POOL, r))
}

fn pool_quote(a: &Value) -> Result<Value, String> {
    let which = s(a, "pair");
    let pair = if which.is_empty() || which == "wsigil3" { pool::PAIR_WSIGIL3 }
               else if which == "legacy" { pool::PAIR_WSIGIL_LEGACY } else { which };
    let p = pool::read_pool(pair)?;
    let side = { let x = s(a, "side"); if x.is_empty() { "buy" } else { x } };
    let amt = f(a, "amount", 1.0);
    let q = pool::quote(&p, side, amt);
    Ok(json!({"ok": true, "pair": p.pair, "reserve_usdc": p.usdc(), "reserve_wsigil": p.wsigil(),
              "quote": q,
              "note": "read-only quote. Executing it needs a signed Polygon transaction from your own wallet — this tool never signs."}))
}

pub fn register(registry: &mut ToolRegistry) {
    registry.register(ToolDef {
        name: "flux_bitunix_panel",
        description: "THE TRADING PANEL — one call returns everything a Bitunix futures UI needs: live tickers, margin account balance, open positions, working orders, the key fingerprint (never the key), and the active Verified Execution Gate. Reads only; dispatches webhook event `bitunix_panel`. Args: symbols (comma-separated, default BTCUSDT,ETHUSDT).",
        input_schema: json!({"type":"object","properties":{
            "symbols":{"type":"string","description":"comma-separated, e.g. BTCUSDT,ETHUSDT"}}}),
    }, |a| out(panel(s(a, "symbols"))));

    registry.register(ToolDef {
        name: "flux_bitunix_tickers",
        description: "Bitunix futures ticker snapshot (public, unsigned): lastPrice, markPrice, high, low, open, baseVol, quoteVol. Args: symbols (comma-separated; empty = all).",
        input_schema: json!({"type":"object","properties":{"symbols":{"type":"string"}}}),
    }, |a| out(Bitunix::public().and_then(|b| b.tickers(s(a, "symbols")))));

    registry.register(ToolDef {
        name: "flux_bitunix_kline",
        description: "Bitunix futures candlesticks (public). Args: symbol (required), interval (1m 5m 15m 30m 1h 2h 4h 6h 8h 12h 1d 3d 1w 1M), limit (max 200).",
        input_schema: json!({"type":"object","properties":{
            "symbol":{"type":"string"},"interval":{"type":"string"},"limit":{"type":"integer"}},
            "required":["symbol"]}),
    }, |a| out(Bitunix::public().and_then(|b| b.kline(s(a,"symbol"),
        { let i = s(a,"interval"); if i.is_empty() { "1h" } else { i } }, u(a,"limit",100) as u32))));

    registry.register(ToolDef {
        name: "flux_bitunix_depth",
        description: "Bitunix futures order book (public). Args: symbol (required), limit.",
        input_schema: json!({"type":"object","properties":{
            "symbol":{"type":"string"},"limit":{"type":"string"}},"required":["symbol"]}),
    }, |a| out(Bitunix::public().and_then(|b| b.depth(s(a,"symbol"),
        { let l = s(a,"limit"); if l.is_empty() { "50" } else { l } }))));

    registry.register(ToolDef {
        name: "flux_bitunix_account",
        description: "Bitunix futures margin account (SIGNED, read-only): available, frozen, margin, transfer, positionMode, crossUnrealizedPNL, bonus. Args: marginCoin (default USDT).",
        input_schema: json!({"type":"object","properties":{"marginCoin":{"type":"string"}}}),
    }, |a| out(Bitunix::from_env().and_then(|b| b.account(s(a, "marginCoin")))));

    registry.register(ToolDef {
        name: "flux_bitunix_positions",
        description: "Open futures positions (SIGNED, read-only): qty, side, leverage, margin, unrealizedPNL, liqPrice, marginRate. Args: symbol (optional; empty = all).",
        input_schema: json!({"type":"object","properties":{"symbol":{"type":"string"}}}),
    }, |a| out(Bitunix::from_env().and_then(|b| b.positions(s(a, "symbol")))));

    registry.register(ToolDef {
        name: "flux_bitunix_orders",
        description: "Working (unfilled) futures orders (SIGNED, read-only). Args: symbol (optional), limit (max 100), history (true = filled/cancelled history instead).",
        input_schema: json!({"type":"object","properties":{
            "symbol":{"type":"string"},"limit":{"type":"integer"},"history":{"type":"boolean"}}}),
    }, |a| out(Bitunix::from_env().and_then(|b| {
        let (sym, lim) = (s(a,"symbol"), u(a,"limit",10) as u32);
        if b_hist(a) { b.history_orders(sym, lim) } else { b.pending_orders(sym, lim) }
    })));

    registry.register(ToolDef {
        name: "flux_bitunix_propose",
        description: "SIZE AND GATE A TRADE WITHOUT SENDING IT. Converts a USD notional into a base-coin qty at the live mark price, runs the Verified Execution Gate (symbol whitelist · notional cap · leverage cap · limit-price deviation), and returns the EXACT order body a confirm would send. Never sends. Dispatches `bitunix_proposal`. Args: symbol, side (BUY|SELL), usd (notional), leverage, orderType (MARKET|LIMIT), price (for LIMIT), tp, sl, reduceOnly; gate overrides whitelist/max_notional_usd/max_leverage.",
        input_schema: json!({"type":"object","properties":{
            "symbol":{"type":"string"},"side":{"type":"string","description":"BUY or SELL"},
            "usd":{"type":"number","description":"notional in USDT"},
            "leverage":{"type":"integer"},"orderType":{"type":"string","description":"MARKET|LIMIT"},
            "price":{"type":"string"},"tp":{"type":"string"},"sl":{"type":"string"},
            "reduceOnly":{"type":"boolean"},
            "whitelist":{"type":"array","items":{"type":"string"}},
            "max_notional_usd":{"type":"number"},"max_leverage":{"type":"integer"}},
            "required":["symbol","side","usd"]}),
    }, |a| out(propose(a)));

    registry.register(ToolDef {
        name: "flux_bitunix_order",
        description: "PLACE A REAL FUTURES ORDER. Runs the same gate as flux_bitunix_propose and REFUSES unless confirm=true is passed explicitly — without it you get the proposal back. Real money. Dispatches `bitunix_order` and appends to the append-only ledger. Same args as flux_bitunix_propose, plus confirm (boolean).",
        input_schema: json!({"type":"object","properties":{
            "symbol":{"type":"string"},"side":{"type":"string"},"usd":{"type":"number"},
            "leverage":{"type":"integer"},"orderType":{"type":"string"},"price":{"type":"string"},
            "tp":{"type":"string"},"sl":{"type":"string"},"reduceOnly":{"type":"boolean"},
            "confirm":{"type":"boolean","description":"must be true to actually send"},
            "whitelist":{"type":"array","items":{"type":"string"}},
            "max_notional_usd":{"type":"number"},"max_leverage":{"type":"integer"}},
            "required":["symbol","side","usd","confirm"]}),
    }, |a| out(order(a)));

    registry.register(ToolDef {
        name: "flux_bitunix_cancel",
        description: "Cancel futures orders. Propose-only unless confirm=true. With orderIds, cancels those; without, cancels ALL orders on the symbol. Args: symbol (required), orderIds (array), confirm.",
        input_schema: json!({"type":"object","properties":{
            "symbol":{"type":"string"},"orderIds":{"type":"array","items":{"type":"string"}},
            "confirm":{"type":"boolean"}},"required":["symbol"]}),
    }, |a| out(cancel(a)));

    registry.register(ToolDef {
        name: "flux_bitunix_pool",
        description: "THE wSIGIL/USDC POOL ON POLYGON — live reserves, spot price, TVL and a buy-size slippage ladder ($1/$5/$10/$50/$100), read straight off chain with getReserves(). Decimals are taken from the on-chain token0() result, never assumed (USDC 6dp vs wSIGIL 18dp is a 10^27 error). Read-only; dispatches `bitunix_pool`. Args: pair — 'wsigil3' (default, current), 'legacy' (the older pair the live market page still reads), or a raw pair address.",
        input_schema: json!({"type":"object","properties":{
            "pair":{"type":"string","description":"wsigil3 | legacy | 0x<pair address>"}}}),
    }, |a| out(pool_tool(a)));

    registry.register(ToolDef {
        name: "flux_bitunix_pool_quote",
        description: "Quote a wSIGIL/USDC swap against the live Polygon pool: amount out, effective price, price impact, and a plain verdict (GOOD / OK / POOR / UNTRADEABLE). This is the number a trading slider must show BEFORE anyone signs. Read-only, never signs. Args: side (buy = spend USDC, sell = spend wSIGIL), amount, pair.",
        input_schema: json!({"type":"object","properties":{
            "side":{"type":"string","description":"buy|sell"},"amount":{"type":"number"},
            "pair":{"type":"string"}},"required":["amount"]}),
    }, |a| out(pool_quote(a)));
}

fn b_hist(a: &Value) -> bool { b(a, "history", false) }

/// Bybit + cross-venue tools. Registered separately so the second venue is a visible addition
/// rather than something smuggled into the Bitunix surface.
pub fn register_bybit(registry: &mut ToolRegistry) {
    use flux_bitunix::{arb, bybit::Bybit};

    fn cat(a: &Value) -> String {
        let c = s(a, "category");
        if c.is_empty() { "linear".into() } else { c.to_string() }
    }

    registry.register(ToolDef {
        name: "flux_bybit_tickers",
        description: "Bybit V5 ticker snapshot (public, unsigned): lastPrice, markPrice, bid1/ask1, 24h volume and turnover. Args: category (linear|spot|inverse|option, default linear), symbol.",
        input_schema: json!({"type":"object","properties":{
            "category":{"type":"string"},"symbol":{"type":"string"}}}),
    }, |a| out(Bybit::public().and_then(|b| b.tickers(&cat(a), s(a, "symbol")))));

    registry.register(ToolDef {
        name: "flux_bybit_kline",
        description: "Bybit V5 candlesticks (public). Args: category, symbol (required), interval (1 3 5 15 30 60 120 240 360 720 D W M — minutes as bare numbers), limit (max 1000).",
        input_schema: json!({"type":"object","properties":{
            "category":{"type":"string"},"symbol":{"type":"string"},
            "interval":{"type":"string"},"limit":{"type":"integer"}},"required":["symbol"]}),
    }, |a| out(Bybit::public().and_then(|b| b.kline(&cat(a), s(a,"symbol"),
        { let i = s(a,"interval"); if i.is_empty() { "60" } else { i } }, u(a,"limit",50) as u32))));

    registry.register(ToolDef {
        name: "flux_bybit_orderbook",
        description: "Bybit V5 order book (public) — the depth that decides whether a spread is real. Args: category, symbol (required), limit (max 200).",
        input_schema: json!({"type":"object","properties":{
            "category":{"type":"string"},"symbol":{"type":"string"},"limit":{"type":"integer"}},
            "required":["symbol"]}),
    }, |a| out(Bybit::public().and_then(|b| b.orderbook(&cat(a), s(a,"symbol"), u(a,"limit",25) as u32))));

    registry.register(ToolDef {
        name: "flux_bybit_clock",
        description: "Local clock minus Bybit's server clock, in milliseconds. Bybit rejects any signed request whose timestamp falls outside its recv_window (default 5000 ms) with retCode 10002 — a failure that looks exactly like a bad signature. Check this FIRST when signed Bybit calls fail. Public, no credentials needed.",
        input_schema: json!({"type":"object","properties":{}}),
    }, |_a| out(Bybit::public().and_then(|b| b.server_time_skew_ms())
        .map(|s| json!({"ok": true, "local_minus_bybit_ms": s, "recv_window_ms": 5000,
                        "ok_for_signing": s.abs() < 5000}))));

    registry.register(ToolDef {
        name: "flux_bybit_balance",
        description: "Bybit V5 unified account wallet balance (SIGNED, read-only). Args: accountType (UNIFIED default, or CONTRACT/SPOT). NOTE: Bybit's edge rejects authenticated requests from some server IPs with an empty HTTP 401 before they reach the API — if you see that, it is not the signature; check flux_bybit_clock and the key's IP allowlist.",
        input_schema: json!({"type":"object","properties":{"accountType":{"type":"string"}}}),
    }, |a| out(Bybit::from_env().and_then(|b| b.wallet_balance(s(a, "accountType")))));

    registry.register(ToolDef {
        name: "flux_bybit_positions",
        description: "Open Bybit positions (SIGNED, read-only): size, side, leverage, unrealised PnL, liquidation price. Args: category (default linear), symbol (optional; empty lists all USDT-settled).",
        input_schema: json!({"type":"object","properties":{
            "category":{"type":"string"},"symbol":{"type":"string"}}}),
    }, |a| out(Bybit::from_env().and_then(|b| b.positions(&cat(a), s(a, "symbol")))));

    registry.register(ToolDef {
        name: "flux_bybit_orders",
        description: "Bybit open (unfilled) orders (SIGNED, read-only). Args: category, symbol (optional).",
        input_schema: json!({"type":"object","properties":{
            "category":{"type":"string"},"symbol":{"type":"string"}}}),
    }, |a| out(Bybit::from_env().and_then(|b| b.open_orders(&cat(a), s(a, "symbol")))));

    registry.register(ToolDef {
        name: "flux_bybit_order",
        description: "PLACE A REAL BYBIT ORDER. Runs the same Verified Execution Gate as the Bitunix side (symbol whitelist · notional cap · leverage cap) and REFUSES unless confirm=true — without it you get the exact JSON body a confirm would send. Real money. Args: symbol, side (Buy|Sell), usd (notional), leverage, category, orderType (Market|Limit), price, tp, sl, confirm; gate overrides whitelist/max_notional_usd/max_leverage.",
        input_schema: json!({"type":"object","properties":{
            "symbol":{"type":"string"},"side":{"type":"string"},"usd":{"type":"number"},
            "leverage":{"type":"integer"},"category":{"type":"string"},
            "orderType":{"type":"string"},"price":{"type":"string"},
            "tp":{"type":"string"},"sl":{"type":"string"},
            "confirm":{"type":"boolean","description":"must be true to actually send"},
            "whitelist":{"type":"array","items":{"type":"string"}},
            "max_notional_usd":{"type":"number"},"max_leverage":{"type":"integer"}},
            "required":["symbol","side","usd"]}),
    }, |a| out(bybit_order(a)));

    registry.register(ToolDef {
        name: "flux_arb_scan",
        description: "CROSS-VENUE SPREAD SCAN — compares the same symbols on Bitunix and Bybit and reports the edge AFTER the round-trip taker fee, ranked. Public prices only, no credentials, nothing is placed. It deliberately calls a spread that does not clear costs 'NO EDGE' rather than dressing a loss up as an opportunity, and it will not invent a spread when either venue fails to answer. Args: symbols (comma-separated, default BTCUSDT,ETHUSDT,SOLUSDT,XRPUSDT,DOGEUSDT).",
        input_schema: json!({"type":"object","properties":{"symbols":{"type":"string"}}}),
    }, |a| {
        let syms: Vec<String> = { let x = s(a, "symbols");
            if x.is_empty() { "BTCUSDT,ETHUSDT,SOLUSDT,XRPUSDT,DOGEUSDT" } else { x } }
            .split(',').map(|s| s.trim().to_uppercase()).filter(|s| !s.is_empty()).collect();
        out(Ok(emit(EV_ARB, arb::scan(&syms))))
    });

    registry.register(ToolDef {
        name: "flux_sigil_exit_capacity",
        description: "HOW MUCH MINED SIGIL CAN ACTUALLY BE SOLD — reads the live wSIGIL/USDC pool on Polygon and binary-searches the largest sale whose price impact stays under your tolerance, returning the wSIGIL amount and the USDC proceeds. This is the number that governs how much mining output can be turned into money, and it is a DEPTH question, not a price question. wSIGIL is not listed on Bitunix or Bybit, so no cross-exchange leg exists for it. Args: max_impact_pct (default 1.0).",
        input_schema: json!({"type":"object","properties":{"max_impact_pct":{"type":"number"}}}),
    }, |a| out(flux_bitunix::pool::read_pool(flux_bitunix::pool::PAIR_WSIGIL3)
        .map(|p| emit(EV_ARB, arb::sigil_sale_capacity(p.usdc(), p.wsigil(), f(a, "max_impact_pct", 1.0))))));
}

/// Gate a Bybit order the same way the Bitunix one is gated. Shared discipline, separate venue.
fn bybit_order(a: &Value) -> Result<Value, String> {
    use flux_bitunix::bybit::Bybit;
    let symbol = s(a, "symbol").to_uppercase();
    let side = s(a, "side").to_uppercase();
    let usd = f(a, "usd", 0.0);
    let lev = u(a, "leverage", 1) as u32;
    let category = { let c = s(a, "category"); if c.is_empty() { "linear" } else { c } };
    let otype = { let t = s(a, "orderType"); if t.is_empty() { "Market".into() } else { t.to_string() } };
    let price = a.get("price").and_then(|v| v.as_str()).filter(|x| !x.is_empty()).map(|x| x.to_string());
    let g = gate_from(a);

    // NOT `b` — that shadows the `b(args, key, default)` bool helper above and the call sites below
    // silently become "call the client", which is a compile error here and would have been a subtle
    // wrong-argument bug if the types had happened to line up.
    let client = Bybit::from_env()?;
    let mark = client.last_price(category, &symbol)?;
    let plimit = price.as_ref().and_then(|p| p.parse::<f64>().ok());
    if let Verdict::Reject(why) = g.check(&symbol, &side, usd, lev, plimit, Some(mark)) {
        return Ok(emit(EV_PROPOSAL, json!({
            "ok": false, "venue": "bybit", "gate": "REJECTED", "reason": why,
            "gate_config": gate_json(&g), "mark_price": mark, "note": "nothing was sent"})));
    }
    let qty = flux_bitunix::qty_from_notional(usd, plimit.unwrap_or(mark), 4)?;
    let order = Bybit::build_order(category, &symbol, &side, &otype, &qty, price.as_deref(),
        b(a, "reduceOnly", false), opt(a, "tp"), opt(a, "sl"));
    let confirm = b(a, "confirm", false);
    if !confirm {
        return Ok(emit(EV_PROPOSAL, json!({
            "ok": true, "venue": "bybit", "gate": "PASS", "gate_config": gate_json(&g),
            "mark_price": mark, "qty": qty, "would_send": order, "confirmed": false,
            "endpoint": "POST /v5/order/create",
            "note": "PROPOSAL ONLY — re-call with confirm=true to place it."})));
    }
    let res = client.place_order(&order, true)?;
    Ok(emit(EV_ORDER, json!({"ok": true, "venue": "bybit", "confirmed": true,
                             "sent": order, "result": res, "gate_config": gate_json(&g)})))
}
