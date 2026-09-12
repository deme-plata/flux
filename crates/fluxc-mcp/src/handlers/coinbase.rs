//! flux_coinbase_* — the Coinbase Advanced Trade surface as one-call MCP combos, executed by the
//! `flux-coinbase` engine. Credentials (a CDP API key) stay server-side and are never returned in
//! a tool result — only a fingerprint.
//!
//! # Safety contract
//! Reads are free. **Writes are propose-only unless `confirm: true` is passed**, and every write
//! must additionally clear the Verified Execution Gate (product whitelist · notional cap ·
//! leverage cap · limit-price deviation bound). A proposal shows the exact JSON body a confirm
//! would send AND Coinbase's own preview (fees, fill estimate), so a human is never asked to
//! approve something they cannot see. Every proposal/order/cancel/DCA tick is appended to the
//! append-only ledger and dispatched to the webhook bus.

use serde_json::{json, Value};
use crate::handlers::{ToolDef, ToolRegistry};
use flux_coinbase::{base_from_quote, build_order, dca, is_perp, key_file_path, order_json, position_json, product_json,
                    Coinbase, Credentials, Gate, OrderKind, Verdict};
use fluxc_webhooks::webhook;

const EV_PROPOSAL: &str = "coinbase_proposal";
const EV_ORDER: &str = "coinbase_order";
const EV_PANEL: &str = "coinbase_panel";
const EV_DCA: &str = "coinbase_dca";

fn s<'a>(a: &'a Value, k: &str) -> &'a str { a.get(k).and_then(|v| v.as_str()).unwrap_or("") }
fn f(a: &Value, k: &str, d: f64) -> f64 { a.get(k).and_then(|v| v.as_f64()).unwrap_or(d) }
fn u(a: &Value, k: &str, d: u64) -> u64 { a.get(k).and_then(|v| v.as_u64()).unwrap_or(d) }
fn b(a: &Value, k: &str, d: bool) -> bool { a.get(k).and_then(|v| v.as_bool()).unwrap_or(d) }
fn out(r: Result<Value, String>) -> String {
    match r { Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_default(),
              Err(e) => serde_json::to_string_pretty(&json!({"ok": false, "error": e})).unwrap_or_default() }
}

fn emit(event: &str, v: Value) -> Value {
    flux_coinbase::record(&json!({"kind": event, "payload": v}));
    webhook::auto_dispatch(event, v.clone());
    let mut v = v;
    v["webhook_event"] = json!(event);
    v["ledger"] = json!(flux_coinbase::ledger_path());
    v
}

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

/// Key presence, what it may do, and whether perps (leverage) are enabled — the setup check.
fn status() -> Result<Value, String> {
    let fp = Credentials::load().map(|c| c.fingerprint());
    match (fp, Coinbase::from_env()) {
        (Ok(fp), Ok(c)) => {
            let perms = c.key_permissions().unwrap_or_else(|e| json!({"error": e}));
            let intx = c.intx_portfolio_uuid().unwrap_or(None);
            Ok(json!({"ok": true, "key": fp, "key_file": key_file_path(), "permissions": perms,
                      "perps_enabled": intx.is_some(), "intx_portfolio": intx,
                      "gate": Gate::default().to_json(), "ledger": flux_coinbase::ledger_path(),
                      "cli": "flux-coinbase tui --product BTC-USD   (add --live to let Enter place orders)",
                      "note": if intx.is_some() { "spot + perpetual futures available" } else { "spot only — this account has no INTX portfolio, so leverage/perps are not enabled on Coinbase's side" }}))
        }
        (Err(e), _) | (_, Err(e)) => Ok(json!({"ok": false, "key": "<none>", "error": e,
            "setup": "Create a CDP Secret API key (VIEW + TRADE, no TRANSFER) at https://portal.cdp.coinbase.com, download the JSON, then `flux-coinbase setup --file <json>`; or call flux_coinbase_setup with key_json.",
            "key_file": key_file_path()})),
    }
}

/// Install a key from its JSON text. The text is written to the 0600 key file and NOT echoed.
fn setup(a: &Value) -> Result<Value, String> {
    let text = s(a, "key_json");
    if text.trim().is_empty() {
        if let Some(p) = a.get("key_path").and_then(|v| v.as_str()) {
            let t = std::fs::read_to_string(p).map_err(|e| format!("read {p}: {e}"))?;
            let fp = flux_coinbase::install_key(&t)?;
            return status().map(|mut v| { v["installed"] = json!(fp); v });
        }
        return Err("pass key_json (the CDP key file contents) or key_path".into());
    }
    let fp = flux_coinbase::install_key(text)?;
    status().map(|mut v| { v["installed"] = json!(fp); v })
}

/// Gate + size + build the exact body + Coinbase preview. Sends nothing.
fn propose(a: &Value) -> Result<Value, String> {
    let pid = s(a, "product_id").to_uppercase();
    let side = s(a, "side").to_uppercase();
    let usd = f(a, "usd", 0.0);
    let lev = u(a, "leverage", 1) as u32;
    let limit: Option<f64> = a.get("limit_price").and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|x| x.parse().ok())));
    let g = gate_from(a);
    let c = Coinbase::from_env()?;
    let mark = c.mark(&pid)?;
    let info = c.product(&pid)?;
    if let Verdict::Reject(why) = g.check(&pid, &side, usd, lev, limit, Some(mark)) {
        return Ok(emit(EV_PROPOSAL, json!({
            "ok": false, "gate": "REJECTED", "reason": why, "gate_config": g.to_json(),
            "product_id": pid, "side": side, "usd": usd, "leverage": lev, "mark_price": mark,
            "note": "nothing was sent. Adjust the request, or widen the gate deliberately."})));
    }
    let kind = if let Some(p) = limit {
        OrderKind::Limit { base_size: base_from_quote(usd, p, &info.base_increment)?, limit_price: format!("{p:.2}"), post_only: b(a, "post_only", false) }
    } else if is_perp(&pid) || side == "SELL" {
        OrderKind::MarketBase { base_size: base_from_quote(usd, mark, &info.base_increment)? }
    } else { OrderKind::MarketQuote { quote_size: format!("{usd:.2}") } };
    let margin = { let m = s(a, "margin_type"); if m.is_empty() { "CROSS".to_string() } else { m.to_uppercase() } };
    let order = build_order(&pid, &side, &kind, Some(lev), Some(&margin), None);
    let preview = c.preview(&order).unwrap_or_else(|e| json!({"error": e}));
    let est_liq = dca::est_liquidation(limit.unwrap_or(mark), lev, side == "BUY", 0.01);
    Ok(emit(EV_PROPOSAL, json!({
        "ok": true, "gate": "PASS", "gate_config": g.to_json(), "mark_price": mark,
        "notional_usd": usd, "leverage": lev, "est_liquidation": est_liq,
        "would_send": order, "preview": preview, "endpoint": "POST /api/v3/brokerage/orders",
        "note": "PROPOSAL ONLY — nothing sent. Call flux_coinbase_order with the same args plus confirm=true."})))
}

/// The one tool that can move money. Refuses without `confirm: true` AND a passing gate.
fn order(a: &Value) -> Result<Value, String> {
    let confirm = b(a, "confirm", false);
    let p = propose(a)?;
    if p["ok"] != json!(true) { return Ok(p); }
    if !confirm { let mut p = p; p["confirm_required"] = json!(true); return Ok(p); }
    let body = p["would_send"].clone();
    let c = Coinbase::from_env()?;
    let res = c.place_order(&body, true)?;
    Ok(emit(EV_ORDER, json!({"ok": true, "confirmed": true, "sent": body, "result": res,
                             "preview": p["preview"].clone(), "gate_config": p["gate_config"].clone(), "mark_price": p["mark_price"].clone()})))
}

fn cancel(a: &Value) -> Result<Value, String> {
    let confirm = b(a, "confirm", false);
    let ids: Vec<String> = a.get("order_ids").and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|x| x.as_str()).map(|s| s.to_string()).collect()).unwrap_or_default();
    let c = Coinbase::from_env()?;
    let r = if ids.is_empty() { c.cancel_all(&s(a, "product_id").to_uppercase(), confirm)? } else { c.cancel(&ids, confirm)? };
    Ok(emit(EV_ORDER, json!({"ok": true, "action": "cancel", "order_ids": ids, "confirmed": confirm, "result": r})))
}

fn dca_plan(a: &Value) -> Result<Value, String> {
    let cfg = dca::DcaConfig::from_json(a);
    let c = Coinbase::best_effort()?;
    let mv = dca::market_view(&c, &cfg)?;
    let st = dca::DcaState::load(&cfg.state_path());
    let p = dca::plan(&cfg, &mv, &st);
    Ok(emit(EV_DCA, json!({"ok": true, "config": cfg.to_json(),
        "market": {"price": mv.price, "sma": mv.sma, "rsi": mv.rsi, "candles": mv.candles, "funding_rate": mv.funding_rate},
        "state": st.to_json(), "plan": p.to_json(),
        "note": "plan only — flux_coinbase_dca_tick with confirm=true places it (force=true ignores the schedule)"})))
}

fn dca_tick(a: &Value) -> Result<Value, String> {
    let cfg = dca::DcaConfig::from_json(a);
    let c = Coinbase::from_env()?;
    let v = dca::tick(&cfg, &c, b(a, "confirm", false), b(a, "force", false))?;
    Ok(emit(EV_DCA, v))
}

pub fn register(registry: &mut ToolRegistry) {
    let gate_props = json!({
        "whitelist": {"type": "array", "items": {"type": "string"}, "description": "gate override: products allowed"},
        "max_notional_usd": {"type": "number", "description": "gate override: per-order notional cap (default 100)"},
        "max_leverage": {"type": "integer", "description": "gate override: leverage cap (default 3)"}});
    let with_gate = |mut props: Value| { for (k, v) in gate_props.as_object().unwrap() { props[k] = v.clone(); } props };

    registry.register(ToolDef {
        name: "flux_coinbase_status",
        description: "IS COINBASE SET UP? Key fingerprint (never the key), what the key may do (view/trade/transfer), whether perpetual futures (leverage) are enabled on the account (INTX portfolio), the active Verified Execution Gate, and the ledger path. Call this first. If no key: returns the exact setup steps.",
        input_schema: json!({"type":"object","properties":{}}),
    }, |_a| out(status()));

    registry.register(ToolDef {
        name: "flux_coinbase_setup",
        description: "INSTALL THE COINBASE CDP API KEY. Pass key_json (the downloaded key file contents: {name|id, privateKey}) or key_path. Validates the key (ES256 PEM or Ed25519 base64), writes it to the root-only 0600 key file, then verifies with Coinbase's key_permissions. The key is NEVER echoed back. Create keys at https://portal.cdp.coinbase.com with VIEW+TRADE (no TRANSFER).",
        input_schema: json!({"type":"object","properties":{
            "key_json":{"type":"string","description":"contents of the CDP key JSON file"},
            "key_path":{"type":"string","description":"path to the CDP key JSON file on this box"}}}),
    }, |a| out(setup(a)));

    registry.register(ToolDef {
        name: "flux_coinbase_panel",
        description: "THE TRADING PANEL — one call returns everything a Coinbase UI needs: product (price, 24h change, increments, perp max leverage + funding), top-10 order book with spread + imbalance, balances, perp positions, open orders, the key fingerprint and the active gate. Reads only; dispatches `coinbase_panel`. Args: product_id (default BTC-USD).",
        input_schema: json!({"type":"object","properties":{"product_id":{"type":"string","description":"e.g. BTC-USD, ETH-USD, BTC-PERP-INTX"}}}),
    }, |a| out(Coinbase::best_effort().map(|c| emit(EV_PANEL, c.panel({ let p = s(a, "product_id"); if p.is_empty() { "BTC-USD" } else { p } })))));

    registry.register(ToolDef {
        name: "flux_coinbase_products",
        description: "Coinbase products (public). perps=true lists perpetual futures (*-PERP-INTX) with max_leverage, funding_rate and open_interest; else spot. Sorted by 24h volume. Args: perps (boolean), limit (default 40).",
        input_schema: json!({"type":"object","properties":{"perps":{"type":"boolean"},"limit":{"type":"integer"}}}),
    }, |a| out(Coinbase::public().and_then(|c| c.products(b(a, "perps", false))).map(|mut ps| {
        ps.sort_by(|x, y| y.volume_24h.partial_cmp(&x.volume_24h).unwrap_or(std::cmp::Ordering::Equal));
        json!({"count": ps.len(), "products": ps.iter().take(u(a, "limit", 40) as usize).map(product_json).collect::<Vec<_>>()})
    })));

    registry.register(ToolDef {
        name: "flux_coinbase_ticker",
        description: "One Coinbase product (public): price, 24h change %, 24h volume, base/quote increments, min sizes, and for perps max leverage + funding rate + open interest. Args: product_id (required).",
        input_schema: json!({"type":"object","properties":{"product_id":{"type":"string"}},"required":["product_id"]}),
    }, |a| out(Coinbase::public().and_then(|c| c.product(s(a, "product_id"))).map(|p| product_json(&p))));

    registry.register(ToolDef {
        name: "flux_coinbase_book",
        description: "Coinbase order book (public) — best bid/ask, mid, spread in bps, top-10 imbalance (−1..1, >0 = bids stacked), and the levels. The depth that decides whether a price is real. Args: product_id (required), limit (levels per side, default 20, max 250).",
        input_schema: json!({"type":"object","properties":{"product_id":{"type":"string"},"limit":{"type":"integer"}},"required":["product_id"]}),
    }, |a| out(Coinbase::public().and_then(|c| c.book(s(a, "product_id"), u(a, "limit", 20) as u32)).map(|bk| json!({
        "product_id": bk.product_id, "best_bid": bk.best_bid(), "best_ask": bk.best_ask(), "mid": bk.mid(),
        "spread_bps": bk.spread_bps(), "imbalance_top10": bk.imbalance(10),
        "bids": bk.bids.iter().map(|l| json!([l.price, l.size])).collect::<Vec<_>>(),
        "asks": bk.asks.iter().map(|l| json!([l.price, l.size])).collect::<Vec<_>>()}))));

    registry.register(ToolDef {
        name: "flux_coinbase_candles",
        description: "Coinbase candles (public), oldest→newest, plus SMA20/SMA200/RSI14 computed from the closes. Args: product_id (required), granularity (1m 5m 15m 30m 1h 2h 6h 1d; default 1h), count (max 350, default 100).",
        input_schema: json!({"type":"object","properties":{"product_id":{"type":"string"},"granularity":{"type":"string"},"count":{"type":"integer"}},"required":["product_id"]}),
    }, |a| out(Coinbase::public().and_then(|c| c.candles(s(a, "product_id"), { let g = s(a, "granularity"); if g.is_empty() { "1h" } else { g } }, u(a, "count", 100) as u32)).map(|cs| {
        let closes: Vec<f64> = cs.iter().map(|c| c.close).collect();
        json!({"count": cs.len(), "sma20": dca::sma(&closes, 20), "sma200": dca::sma(&closes, 200), "rsi14": dca::rsi(&closes, 14),
               "candles": cs.iter().map(|c| json!([c.start, c.open, c.high, c.low, c.close, c.volume])).collect::<Vec<_>>()})
    })));

    registry.register(ToolDef {
        name: "flux_coinbase_accounts",
        description: "Coinbase balances (SIGNED, read-only): currency, available, hold. Only non-zero rows.",
        input_schema: json!({"type":"object","properties":{}}),
    }, |_a| out(Coinbase::from_env().and_then(|c| c.accounts()).map(|acc| json!(acc.iter().filter(|x| x.available > 0.0 || x.hold > 0.0)
        .map(|x| json!({"currency": x.currency, "available": x.available, "hold": x.hold, "type": x.kind})).collect::<Vec<_>>()))));

    registry.register(ToolDef {
        name: "flux_coinbase_positions",
        description: "Open Coinbase perpetual-futures positions (SIGNED, read-only): size, side, entry, mark, liquidation price, leverage, unrealised PnL — plus a risk report flagging any position within warn_pct of liquidation (default 15). If the account has no INTX portfolio the list is empty and the note says so. Args: warn_pct.",
        input_schema: json!({"type":"object","properties":{"warn_pct":{"type":"number"}}}),
    }, |a| out(Coinbase::from_env().and_then(|c| c.positions()).map(|(p, why)| json!({
        "positions": p.iter().map(position_json).collect::<Vec<_>>(), "note": why, "risk": dca::risk_report(&p, f(a, "warn_pct", 15.0))}))));

    registry.register(ToolDef {
        name: "flux_coinbase_orders",
        description: "Open (unfilled) Coinbase orders (SIGNED, read-only), or with fills=true the recent fills. Args: product_id (optional), fills (boolean), limit.",
        input_schema: json!({"type":"object","properties":{"product_id":{"type":"string"},"fills":{"type":"boolean"},"limit":{"type":"integer"}}}),
    }, |a| out(Coinbase::from_env().and_then(|c| if b(a, "fills", false) { c.fills(s(a, "product_id"), u(a, "limit", 30) as u32) }
        else { c.open_orders(s(a, "product_id")).map(|o| json!(o.iter().map(order_json).collect::<Vec<_>>())) })));

    registry.register(ToolDef {
        name: "flux_coinbase_propose",
        description: "SIZE AND GATE A COINBASE TRADE WITHOUT SENDING IT. Converts a USD notional into the exact order body (market IOC by quote for spot buys, by base for perps/sells, or limit GTC), runs the Verified Execution Gate (product whitelist · notional cap · leverage cap · limit deviation · no leverage on spot), asks Coinbase for its own PREVIEW (fees, fill estimate), and for leverage prints the estimated liquidation price. Never sends. Dispatches `coinbase_proposal`. Args: product_id, side (BUY|SELL), usd, leverage (perps only), limit_price, post_only, margin_type (CROSS|ISOLATED); gate overrides.",
        input_schema: json!({"type":"object","properties": with_gate(json!({
            "product_id":{"type":"string"},"side":{"type":"string","description":"BUY or SELL"},
            "usd":{"type":"number","description":"notional in USD (for leverage: position notional, not margin)"},
            "leverage":{"type":"integer"},"limit_price":{"type":"number"},"post_only":{"type":"boolean"},
            "margin_type":{"type":"string"}})),"required":["product_id","side","usd"]}),
    }, |a| out(propose(a)));

    registry.register(ToolDef {
        name: "flux_coinbase_order",
        description: "PLACE A REAL COINBASE ORDER. Runs the same gate + preview as flux_coinbase_propose and REFUSES unless confirm=true is passed explicitly — without it you get the proposal back. Real money. Dispatches `coinbase_order` and appends to the append-only ledger. Same args as flux_coinbase_propose plus confirm (boolean).",
        input_schema: json!({"type":"object","properties": with_gate(json!({
            "product_id":{"type":"string"},"side":{"type":"string"},"usd":{"type":"number"},"leverage":{"type":"integer"},
            "limit_price":{"type":"number"},"post_only":{"type":"boolean"},"margin_type":{"type":"string"},
            "confirm":{"type":"boolean","description":"must be true to actually send"}})),"required":["product_id","side","usd","confirm"]}),
    }, |a| out(order(a)));

    registry.register(ToolDef {
        name: "flux_coinbase_cancel",
        description: "Cancel Coinbase orders. Propose-only unless confirm=true. With order_ids, cancels those; without, cancels ALL open orders (on product_id if given, else everything). Args: order_ids (array), product_id, confirm.",
        input_schema: json!({"type":"object","properties":{"order_ids":{"type":"array","items":{"type":"string"}},"product_id":{"type":"string"},"confirm":{"type":"boolean"}}}),
    }, |a| out(cancel(a)));

    registry.register(ToolDef {
        name: "flux_coinbase_dca_plan",
        description: "LEVERAGED DCA PLAN (nothing sent). Reads candles → SMA/RSI, applies the dip tilt (buy more below the SMA / RSI<30, less above / RSI>70, clamped 0.25–max_multiplier), sizes margin × leverage into a base quantity, estimates the liquidation price, and runs the gate + position cap + liquidation-distance floor. Returns the exact order a tick would send and the running state (ticks, avg entry, notional held). Args: product_id (BTC-USD spot or BTC-PERP-INTX perp), base_usd (margin per tick), every_hours, leverage, margin_type, dip_tilt, max_multiplier, max_position_usd, min_liq_distance_pct, candle_gran, sma_len, rsi_len; gate overrides.",
        input_schema: json!({"type":"object","properties": with_gate(json!({
            "product_id":{"type":"string"},"base_usd":{"type":"number"},"every_hours":{"type":"number"},"leverage":{"type":"integer"},
            "margin_type":{"type":"string"},"dip_tilt":{"type":"boolean"},"max_multiplier":{"type":"number"},
            "max_position_usd":{"type":"number"},"min_liq_distance_pct":{"type":"number"},"candle_gran":{"type":"string"},
            "sma_len":{"type":"integer"},"rsi_len":{"type":"integer"}}))}),
    }, |a| out(dca_plan(a)));

    registry.register(ToolDef {
        name: "flux_coinbase_dca_tick",
        description: "ONE BEAT OF THE DCA METRONOME. If the schedule says it is due (or force=true), builds the plan exactly as flux_coinbase_dca_plan, gets Coinbase's preview, and — ONLY with confirm=true — places the ticket and folds the fill into the persistent DCA state (avg entry, notional, history). Without confirm it is a proposal. Real money when confirmed. Dispatches `coinbase_dca`. Same args as flux_coinbase_dca_plan plus confirm, force.",
        input_schema: json!({"type":"object","properties": with_gate(json!({
            "product_id":{"type":"string"},"base_usd":{"type":"number"},"every_hours":{"type":"number"},"leverage":{"type":"integer"},
            "margin_type":{"type":"string"},"dip_tilt":{"type":"boolean"},"max_multiplier":{"type":"number"},
            "max_position_usd":{"type":"number"},"min_liq_distance_pct":{"type":"number"},"candle_gran":{"type":"string"},
            "confirm":{"type":"boolean","description":"must be true to actually place the ticket"},
            "force":{"type":"boolean","description":"ignore the schedule and tick now"}}))}),
    }, |a| out(dca_tick(a)));

    registry.register(ToolDef {
        name: "flux_coinbase_tui",
        description: "HOW TO OPEN THE TRADING TUI — returns the exact command for the ratatui terminal (order book with depth bars, candlestick chart + SMA, tape, balances, perp positions with liquidation distance, open orders, leveraged-DCA panel). An MCP call cannot draw a terminal, so this hands back the command line; run it in a real terminal. PAPER by default; --live lets Enter place gated orders.",
        input_schema: json!({"type":"object","properties":{"product_id":{"type":"string"},"live":{"type":"boolean"}}}),
    }, |a| {
        let bin = std::env::var("FLUX_COINBASE_BIN").unwrap_or_else(|_| "/home/storage/deepseek-codewhale/flux/target/debug/flux-coinbase".into());
        let p = { let x = s(a, "product_id"); if x.is_empty() { "BTC-USD".to_string() } else { x.to_uppercase() } };
        out(Ok(json!({"ok": true, "command": format!("{bin} tui --product {p}{}", if b(a, "live", false) { " --live" } else { "" }),
            "keys": "1-5 tabs · ←→ product · g candles · b/s order form · d DCA plan · c cancel-all · r refresh · q quit",
            "mode": if b(a, "live", false) { "LIVE — Enter places orders after gate + preview + confirm" } else { "PAPER — Enter shows what would be sent" }})))
    });
}
