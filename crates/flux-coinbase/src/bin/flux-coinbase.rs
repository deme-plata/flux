//! flux-coinbase CLI — setup, market data, account, GATED order placement, leveraged DCA, and the
//! trading TUI. Every write needs `--confirm`; without it you get the exact JSON that would be sent.

use flux_coinbase::dca::{self, DcaConfig, DcaState};
use flux_coinbase::{base_from_quote, build_order, coinbase_spec, install_key, is_perp, key_file_path, order_json,
                    position_json, product_json, Coinbase, Credentials, Gate, OrderKind, Verdict};
use serde_json::{json, Value};

const HELP: &str = r#"flux-coinbase — Coinbase Advanced Trade, the flux way (propose-only unless --confirm)

SETUP
  setup [--file key.json]      install a CDP API key (validates, chmod 600, verifies with Coinbase)
  setup --howto                 where to create the key and which permissions to give it
  status                        key fingerprint + what it may do (view/trade/transfer) + portfolios

MARKET (public, no key needed)
  products [--perps]            spot products, or perpetual futures with max leverage + funding
  ticker <PRODUCT>              price, 24h change, volume, increments   e.g. BTC-USD, BTC-PERP-INTX
  book <PRODUCT> [--limit N]    order book with spread + imbalance
  candles <PRODUCT> [--gran 1h] [--count N]   1m 5m 15m 30m 1h 2h 6h 1d
  trades <PRODUCT>              recent trades

ACCOUNT (signed, read-only)
  accounts                      balances
  positions                     perp positions with liquidation distance + risk report
  orders [--product P]          open orders
  fills [--product P]           recent fills

TRADING (signed, GATED — nothing is sent without --confirm)
  propose <buy|sell> <PRODUCT> --usd N [--leverage L] [--limit PRICE] [--post-only]
  order   <buy|sell> <PRODUCT> --usd N [...] --confirm
  cancel  [--ids a,b] [--product P] [--confirm]        (no ids = cancel all)
  gate overrides on any of the above: --whitelist A,B --max-notional N --max-leverage L

LEVERAGED DCA
  dca plan  --product P --usd N [--leverage L] [--every-hours H] [--max-position N] [--no-tilt]
  dca tick  (same args) [--confirm] [--force]          one metronome beat (force = ignore schedule)
  dca run   (same args) [--confirm] [--poll-secs 300]  loop forever; --confirm makes it real
  dca state --product P

TUI
  tui [--product BTC-USD] [--products A,B,C] [--gran 1h] [--live]
      PAPER by default — Enter shows what would be sent. --live lets Enter place orders.

  spec                          the flux-api endpoint spec (OpenAPI source)
"#;

const HOWTO: &str = r#"How to create the Coinbase key (5 minutes)

  1. https://portal.cdp.coinbase.com/  →  API keys  →  Secret API keys  →  Create
  2. Permissions: VIEW + TRADE.  Do NOT grant TRANSFER — this tool never moves funds off-exchange
     and a key that cannot withdraw is a key that cannot be drained.
  3. IP allowlist: add this box's public IP (Epsilon: 89.149.241.126). Optional but strongly advised.
  4. Signature algorithm: either is fine (ECDSA/ES256 or Ed25519) — both are supported here.
  5. Download the JSON. It looks like {"name":"organizations/…/apiKeys/…","privateKey":"-----BEGIN EC PRIVATE KEY-----…"}
  6. Copy it here and run:   flux-coinbase setup --file /path/to/cdp_api_key.json
     (or paste it on stdin:  flux-coinbase setup < cdp_api_key.json)

  The key is stored at /root/.config/coinbase/cdp_api_key.json, mode 600, and is never printed.
  Perpetual futures (leverage) need the account to have an INTX portfolio — `flux-coinbase status`
  says whether it does. Spot trading works regardless.
"#;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let cmd = a.first().map(|s| s.as_str()).unwrap_or("help");
    let get = |k: &str| -> Option<String> { a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).cloned() };
    let has = |k: &str| a.iter().any(|x| x == k);
    let confirm = has("--confirm");
    let pp = |r: Result<Value, String>| match r {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default()),
        Err(e) => { eprintln!("✗ {e}"); std::process::exit(1); }
    };
    let pos = |i: usize| a.get(i).cloned().unwrap_or_default();
    let gate_from_args = || {
        let mut g = Gate::default();
        if let Some(w) = get("--whitelist") { g.whitelist = w.split(',').map(|s| s.trim().to_uppercase()).filter(|s| !s.is_empty()).collect(); }
        if let Some(n) = get("--max-notional").and_then(|s| s.parse().ok()) { g.max_notional_usd = n; }
        if let Some(l) = get("--max-leverage").and_then(|s| s.parse().ok()) { g.max_leverage = l; }
        g
    };
    let dca_cfg = || {
        let mut v = json!({});
        if let Some(p) = get("--product") { v["product_id"] = json!(p); }
        if let Some(x) = get("--usd").and_then(|s| s.parse::<f64>().ok()) { v["base_usd"] = json!(x); }
        if let Some(x) = get("--leverage").and_then(|s| s.parse::<u64>().ok()) { v["leverage"] = json!(x); }
        if let Some(x) = get("--every-hours").and_then(|s| s.parse::<f64>().ok()) { v["every_hours"] = json!(x); }
        if let Some(x) = get("--max-position").and_then(|s| s.parse::<f64>().ok()) { v["max_position_usd"] = json!(x); }
        if let Some(x) = get("--max-multiplier").and_then(|s| s.parse::<f64>().ok()) { v["max_multiplier"] = json!(x); }
        if let Some(x) = get("--max-notional").and_then(|s| s.parse::<f64>().ok()) { v["max_notional_usd"] = json!(x); }
        if let Some(x) = get("--max-leverage").and_then(|s| s.parse::<u64>().ok()) { v["max_leverage"] = json!(x); }
        if let Some(x) = get("--gran") { v["candle_gran"] = json!(x); }
        if let Some(x) = get("--margin") { v["margin_type"] = json!(x); }
        if has("--no-tilt") { v["dip_tilt"] = json!(false); }
        DcaConfig::from_json(&v)
    };

    match cmd {
        "help" | "-h" | "--help" => print!("{HELP}"),
        "spec" => pp(Ok(serde_json::to_value(coinbase_spec()).unwrap_or(Value::Null))),

        "setup" => {
            if has("--howto") { print!("{HOWTO}"); return; }
            let text = match get("--file") {
                Some(p) => std::fs::read_to_string(&p).unwrap_or_else(|e| { eprintln!("✗ read {p}: {e}"); std::process::exit(1) }),
                None => {
                    eprintln!("paste the CDP key JSON, then Ctrl-D (or use --file):");
                    let mut s = String::new();
                    use std::io::Read;
                    let _ = std::io::stdin().read_to_string(&mut s);
                    s
                }
            };
            let fp = install_key(&text).unwrap_or_else(|e| { eprintln!("✗ {e}"); std::process::exit(1) });
            println!("✓ key installed at {} — {fp}", key_file_path());
            match Coinbase::from_env().and_then(|c| c.key_permissions()) {
                Ok(v) => { println!("✓ Coinbase accepted the key:"); pp(Ok(v)); }
                Err(e) => { eprintln!("✗ key installed but Coinbase refused it: {e}"); std::process::exit(2); }
            }
        }
        "status" => {
            let fp = Credentials::load().map(|c| c.fingerprint()).unwrap_or_else(|e| format!("<none: {e}>"));
            let c = Coinbase::from_env();
            let out = match c {
                Ok(c) => {
                    let perms = c.key_permissions().unwrap_or_else(|e| json!({"error": e}));
                    let ports = c.portfolios().unwrap_or_else(|e| json!({"error": e}));
                    let intx = c.intx_portfolio_uuid().ok().flatten();
                    json!({"key": fp, "key_file": key_file_path(), "permissions": perms, "portfolios": ports,
                           "perps_enabled": intx.is_some(), "intx_portfolio": intx,
                           "gate": Gate::default().to_json(), "ledger": flux_coinbase::ledger_path()})
                }
                Err(e) => json!({"key": fp, "error": e, "howto": "flux-coinbase setup --howto"}),
            };
            pp(Ok(out));
        }

        "products" => pp(Coinbase::public().and_then(|c| c.products(has("--perps"))).map(|ps| {
            let mut ps = ps; ps.sort_by(|a, b| b.volume_24h.partial_cmp(&a.volume_24h).unwrap_or(std::cmp::Ordering::Equal));
            let n = get("--limit").and_then(|s| s.parse().ok()).unwrap_or(40usize);
            json!({"count": ps.len(), "products": ps.iter().take(n).map(product_json).collect::<Vec<_>>()})
        })),
        "ticker" => pp(Coinbase::public().and_then(|c| c.product(&pos(1))).map(|p| product_json(&p))),
        "book" => pp(Coinbase::public().and_then(|c| c.book(&pos(1), get("--limit").and_then(|s| s.parse().ok()).unwrap_or(20))).map(|b| json!({
            "product_id": b.product_id, "best_bid": b.best_bid(), "best_ask": b.best_ask(), "mid": b.mid(),
            "spread_bps": b.spread_bps(), "imbalance_top10": b.imbalance(10),
            "bids": b.bids.iter().map(|l| json!([l.price, l.size])).collect::<Vec<_>>(),
            "asks": b.asks.iter().map(|l| json!([l.price, l.size])).collect::<Vec<_>>()}))),
        "candles" => pp(Coinbase::public().and_then(|c| c.candles(&pos(1), &get("--gran").unwrap_or_else(|| "1h".into()),
                get("--count").and_then(|s| s.parse().ok()).unwrap_or(100))).map(|cs| {
            let closes: Vec<f64> = cs.iter().map(|c| c.close).collect();
            json!({"count": cs.len(), "sma20": dca::sma(&closes, 20), "sma200": dca::sma(&closes, 200), "rsi14": dca::rsi(&closes, 14),
                   "candles": cs.iter().map(|c| json!([c.start, c.open, c.high, c.low, c.close, c.volume])).collect::<Vec<_>>()})
        })),
        "trades" => pp(Coinbase::public().and_then(|c| c.trades_raw(&pos(1), get("--limit").and_then(|s| s.parse().ok()).unwrap_or(30)))),

        "accounts" => pp(Coinbase::from_env().and_then(|c| c.accounts()).map(|a| json!(a.iter().filter(|b| b.available > 0.0 || b.hold > 0.0)
            .map(|b| json!({"currency": b.currency, "available": b.available, "hold": b.hold, "type": b.kind})).collect::<Vec<_>>()))),
        "positions" => pp(Coinbase::from_env().and_then(|c| c.positions()).map(|(p, why)| json!({
            "positions": p.iter().map(position_json).collect::<Vec<_>>(), "note": why, "risk": dca::risk_report(&p, 15.0)}))),
        "orders" => pp(Coinbase::from_env().and_then(|c| c.open_orders(&get("--product").unwrap_or_default())).map(|o| json!(o.iter().map(order_json).collect::<Vec<_>>()))),
        "fills" => pp(Coinbase::from_env().and_then(|c| c.fills(&get("--product").unwrap_or_default(), get("--limit").and_then(|s| s.parse().ok()).unwrap_or(30)))),

        "propose" | "order" => {
            let side = pos(1).to_uppercase();
            let pid = pos(2).to_uppercase();
            let usd: f64 = get("--usd").and_then(|s| s.parse().ok()).unwrap_or(0.0);
            let lev: u32 = get("--leverage").and_then(|s| s.parse().ok()).unwrap_or(1);
            let limit: Option<f64> = get("--limit").and_then(|s| s.parse().ok());
            let g = gate_from_args();
            pp((|| -> Result<Value, String> {
                let c = Coinbase::from_env()?;
                let mark = c.mark(&pid)?;
                let info = c.product(&pid)?;
                let verdict = g.check(&pid, &side, usd, lev, limit, Some(mark));
                if let Verdict::Reject(r) = verdict {
                    return Ok(json!({"ok": false, "gate": "REJECTED", "reason": r, "gate_config": g.to_json(), "mark": mark, "note": "nothing was sent"}));
                }
                let kind = if let Some(p) = limit {
                    OrderKind::Limit { base_size: base_from_quote(usd, p, &info.base_increment)?, limit_price: format!("{p:.2}"), post_only: has("--post-only") }
                } else if is_perp(&pid) || side == "SELL" {
                    OrderKind::MarketBase { base_size: base_from_quote(usd, mark, &info.base_increment)? }
                } else { OrderKind::MarketQuote { quote_size: format!("{usd:.2}") } };
                let order = build_order(&pid, &side, &kind, Some(lev), Some(&get("--margin").unwrap_or_else(|| "CROSS".into())), None);
                let preview = c.preview(&order).unwrap_or_else(|e| json!({"error": e}));
                if cmd == "propose" || !confirm {
                    return Ok(json!({"ok": true, "gate": "PASS", "gate_config": g.to_json(), "mark": mark, "would_send": order,
                                     "preview": preview, "endpoint": "POST /api/v3/brokerage/orders",
                                     "note": "PROPOSAL ONLY — nothing sent. Re-run as `order … --confirm` to place it."}));
                }
                let res = c.place_order(&order, true)?;
                Ok(json!({"ok": true, "confirmed": true, "sent": order, "preview": preview, "result": res, "gate_config": g.to_json()}))
            })());
        }
        "cancel" => pp(Coinbase::from_env().and_then(|c| match get("--ids") {
            Some(ids) => c.cancel(&ids.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect::<Vec<_>>(), confirm),
            None => c.cancel_all(&get("--product").unwrap_or_default(), confirm),
        })),

        "dca" => {
            let sub = pos(1);
            let cfg = dca_cfg();
            match sub.as_str() {
                "plan" => pp(Coinbase::best_effort().and_then(|c| {
                    let mv = dca::market_view(&c, &cfg)?;
                    let st = DcaState::load(&cfg.state_path());
                    let p = dca::plan(&cfg, &mv, &st);
                    Ok(json!({"config": cfg.to_json(), "market": {"price": mv.price, "sma": mv.sma, "rsi": mv.rsi, "candles": mv.candles, "funding_rate": mv.funding_rate},
                              "state": st.to_json(), "plan": p.to_json(), "note": "plan only — `dca tick --confirm` places it"}))
                })),
                "tick" => pp(Coinbase::from_env().and_then(|c| dca::tick(&cfg, &c, confirm, has("--force")))),
                "state" => pp(Ok(json!({"config": cfg.to_json(), "state": DcaState::load(&cfg.state_path()).to_json(), "path": cfg.state_path()}))),
                "run" => {
                    let poll: u64 = get("--poll-secs").and_then(|s| s.parse().ok()).unwrap_or(300);
                    let c = Coinbase::from_env().unwrap_or_else(|e| { eprintln!("✗ {e}"); std::process::exit(1) });
                    eprintln!("dca run · {} · ${} every {}h × {}x · {} · poll {poll}s", cfg.product_id, cfg.base_usd, cfg.every_hours, cfg.leverage,
                              if confirm { "LIVE (--confirm)" } else { "PAPER (no --confirm)" });
                    loop {
                        match dca::tick(&cfg, &c, confirm, false) {
                            Ok(v) => {
                                if v["due"] == json!(true) { println!("{}", serde_json::to_string(&v).unwrap_or_default()); }
                                else { eprintln!("{} not due, next in {}s", flux_coinbase::now_s(), v["next_in_s"]); }
                            }
                            Err(e) => eprintln!("✗ tick: {e}"),
                        }
                        std::thread::sleep(std::time::Duration::from_secs(poll.max(10)));
                    }
                }
                _ => { eprintln!("dca plan|tick|run|state"); std::process::exit(2); }
            }
        }

        "tui" => {
            let mut o = flux_coinbase::tui::TuiOpts::default();
            if let Some(p) = get("--product") { o.product = p.to_uppercase(); }
            if let Some(ps) = get("--products") { o.products = ps.split(',').map(|s| s.trim().to_uppercase()).filter(|s| !s.is_empty()).collect(); }
            if let Some(g) = get("--gran") { o.gran = g; }
            o.live = has("--live");
            o.dca = dca_cfg();
            if let Err(e) = flux_coinbase::tui::run(o) { eprintln!("✗ tui: {e}"); std::process::exit(1); }
        }
        other => { eprintln!("unknown command {other:?}\n{HELP}"); std::process::exit(2); }
    }
}
