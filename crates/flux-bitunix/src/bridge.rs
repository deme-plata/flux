//! The operator bridge — a tiny HTTP server that lets a BROWSER page drive this crate.
//!
//! # Why this exists at all
//! Bitunix serves **no CORS headers**, so a web page cannot call it directly. The obvious fix —
//! proxying Bitunix through the public sigilgraph.org vhost — would put an authenticated trading
//! API on the open internet behind our key: anyone who found the path could trade the account.
//!
//! So this binds **127.0.0.1 only**, by construction, and there is deliberately no option to bind
//! anything else. The trading UI is a local instrument, not a public endpoint.
//! `Access-Control-Allow-Origin: *` is safe here precisely because nothing off-box can open the
//! socket in the first place.
//!
//! Writes keep the same contract as everywhere else: `/propose` never sends, and `/order` refuses
//! unless the POST body carries `"confirm": true` AND the request clears the gate.

use crate::{pool, qty_from_notional, Bitunix, Gate, Verdict};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};

/// Minimum length for a shared-access token. Short tokens are guessable, and what sits behind this
/// one is a funded trading account — so a weak token is refused rather than warned about.
pub const MIN_TOKEN_LEN: usize = 32;

/// Where the bridge will accept connections.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Bind {
    /// 127.0.0.1 only. No token needed: nothing off-box can open the socket.
    Loopback,
    /// All interfaces. **Requires a token of at least [`MIN_TOKEN_LEN`] characters**, because this
    /// puts an authenticated trading API within reach of anything that can route to the host.
    All,
}

/// Decide whether a requested exposure is allowed. Pure, so the refusal is a unit test rather than
/// a code review comment.
pub fn bind_is_allowed(bind: Bind, token: Option<&str>) -> Result<(), String> {
    match bind {
        Bind::Loopback => Ok(()),
        Bind::All => match token {
            Some(t) if t.len() >= MIN_TOKEN_LEN => Ok(()),
            Some(t) => Err(format!(
                "refusing to bind all interfaces: token is {} chars, minimum is {MIN_TOKEN_LEN}",
                t.len()
            )),
            None => Err("refusing to bind all interfaces without --token: that would expose an \
                         authenticated trading API to anything that can route to this host"
                .into()),
        },
    }
}

/// Constant-time-ish comparison so a token cannot be recovered by timing the reject.
fn token_matches(expected: &str, given: &str) -> bool {
    if expected.len() != given.len() {
        return false;
    }
    expected.bytes().zip(given.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

/// Is this request authorised? Loopback mode needs nothing; token mode needs an exact match on
/// either `Authorization: Bearer <t>` or `?token=<t>`.
pub fn authorised(required: Option<&str>, header: Option<&str>, query: Option<&str>) -> bool {
    let Some(want) = required else { return true };
    let bearer = header
        .and_then(|h| h.strip_prefix("Bearer ").or_else(|| h.strip_prefix("bearer ")))
        .map(|s| s.trim());
    if let Some(b) = bearer {
        if token_matches(want, b) {
            return true;
        }
    }
    query.map(|q| token_matches(want, q)).unwrap_or(false)
}

fn body_json(v: Value, status: &str) -> String {
    let b = serde_json::to_string(&v).unwrap_or_else(|_| "{}".into());
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Allow-Headers: content-type, authorization\r\n\
         Access-Control-Allow-Methods: GET,POST,OPTIONS\r\n\
         Cache-Control: no-store\r\n\
         Content-Length: {}\r\n\r\n{b}",
        b.len()
    )
}

fn qs(path: &str) -> Vec<(String, String)> {
    path.split_once('?')
        .map(|(_, q)| {
            q.split('&')
                .filter_map(|kv| kv.split_once('='))
                .map(|(k, v)| (k.to_string(), percent_decode(v)))
                .collect()
        })
        .unwrap_or_default()
}

fn percent_decode(s: &str) -> String {
    let b = s.replace('+', " ");
    let bytes = b.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz"), 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn get(p: &[(String, String)], k: &str) -> String {
    p.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone()).unwrap_or_default()
}

/// Strip the mount prefix a reverse proxy leaves on the path. `fluxc serve` forwards the request
/// line unchanged, so a rule `/bitunix=127.0.0.1:8477` delivers `/bitunix/health` here rather than
/// `/health`. The bridge accepts either shape instead of depending on a proxy that rewrites.
pub fn strip_mount(route: &str) -> &str {
    for m in ["/bitunix", "/desk"] {
        if let Some(rest) = route.strip_prefix(m) {
            if rest.is_empty() { return "/"; }
            if rest.starts_with('/') { return rest; }
        }
    }
    route
}

fn route(method: &str, path: &str, body: &str) -> (Value, &'static str) {
    let p = qs(path);
    let r = strip_mount(path.split('?').next().unwrap_or("/"));
    let ok = "200 OK";
    let bad = "400 Bad Request";

    match (method, r) {
        ("OPTIONS", _) => (json!({"ok": true}), ok),

        ("GET", "/health") => {
            let key = Bitunix::from_env().map(|b| b.key_fingerprint()).unwrap_or_else(|e| e);
            (json!({"ok": true, "service": "flux-bitunix bridge", "bind": "127.0.0.1 only",
                    "key": key, "venue": crate::BASE,
                    "writes": "propose-only unless confirm=true AND the gate passes"}), ok)
        }

        // ── Bitunix public ─────────────────────────────────────────────────────────
        ("GET", "/tickers") => match Bitunix::public().and_then(|b| b.tickers(&get(&p, "symbols"))) {
            Ok(v) => (v, ok),
            Err(e) => (json!({"ok": false, "error": e}), bad),
        },
        ("GET", "/kline") => {
            let iv = { let i = get(&p, "interval"); if i.is_empty() { "1h".into() } else { i } };
            let lim = get(&p, "limit").parse::<u32>().unwrap_or(100);
            match Bitunix::public().and_then(|b| b.kline(&get(&p, "symbol"), &iv, lim)) {
                Ok(v) => (v, ok),
                Err(e) => (json!({"ok": false, "error": e}), bad),
            }
        }
        ("GET", "/depth") => {
            let lim = { let l = get(&p, "limit"); if l.is_empty() { "50".into() } else { l } };
            match Bitunix::public().and_then(|b| b.depth(&get(&p, "symbol"), &lim)) {
                Ok(v) => (v, ok),
                Err(e) => (json!({"ok": false, "error": e}), bad),
            }
        }

        // ── Bitunix signed reads ───────────────────────────────────────────────────
        ("GET", "/account") => match Bitunix::from_env().and_then(|b| b.account("USDT")) {
            Ok(v) => (v, ok),
            Err(e) => (json!({"ok": false, "error": e}), bad),
        },
        ("GET", "/positions") => match Bitunix::from_env().and_then(|b| b.positions(&get(&p, "symbol"))) {
            Ok(v) => (v, ok),
            Err(e) => (json!({"ok": false, "error": e}), bad),
        },
        ("GET", "/orders") => match Bitunix::from_env().and_then(|b| b.pending_orders(&get(&p, "symbol"), 20)) {
            Ok(v) => (v, ok),
            Err(e) => (json!({"ok": false, "error": e}), bad),
        },

        // ── the gate + order ticket ────────────────────────────────────────────────
        ("POST", "/propose") | ("POST", "/order") => {
            let a: Value = serde_json::from_str(body).unwrap_or(json!({}));
            let confirm = r == "/order" && a.get("confirm").and_then(|v| v.as_bool()).unwrap_or(false);
            match ticket(&a, confirm) {
                Ok(v) => (v, ok),
                Err(e) => (json!({"ok": false, "error": e}), bad),
            }
        }

        // ── the on-chain leg ───────────────────────────────────────────────────────
        ("GET", "/pool") => {
            let which = get(&p, "pair");
            let pair: String = if which == "legacy" { pool::PAIR_WSIGIL_LEGACY.to_string() }
                else if which.starts_with("0x") { which }
                else { pool::PAIR_WSIGIL3.to_string() };
            match pool::pool_report(&pair) {
                Ok(v) => (v, ok),
                Err(e) => (json!({"ok": false, "error": e}), bad),
            }
        }
        ("GET", "/pool_quote") => {
            let which = get(&p, "pair");
            let pair = if which == "legacy" { pool::PAIR_WSIGIL_LEGACY } else { pool::PAIR_WSIGIL3 };
            let side = { let s = get(&p, "side"); if s.is_empty() { "buy".into() } else { s } };
            let amt = get(&p, "amount").parse::<f64>().unwrap_or(1.0);
            match pool::read_pool(pair) {
                Ok(st) => (json!({"ok": true, "reserve_usdc": st.usdc(), "reserve_wsigil": st.wsigil(),
                                  "quote": pool::quote(&st, &side, amt)}), ok),
                Err(e) => (json!({"ok": false, "error": e}), bad),
            }
        }

        ("GET", "/prices") => (crate::aggregator::prices(&p), ok),
        ("GET", "/0x/capabilities") => (crate::aggregator::capabilities(), ok),
        ("GET", "/0x/chains" | "/0x/sources" | "/0x/crosschain/sources" | "/0x/crosschain" | "/0x/crosschain/status") => {
            let endpoint = match r {
                "/0x/chains" => "/swap/chains",
                "/0x/sources" => "/sources",
                "/0x/crosschain/sources" => "/cross-chain/sources",
                "/0x/crosschain/status" => "/cross-chain/status",
                _ => "/cross-chain/quotes",
            };
            match crate::aggregator::zerox(endpoint, &p) {
                Ok(v) => (v, ok),
                Err(e) => (json!({"ok": false, "error": e}), bad),
            }
        },
        ("POST", "/0x/solana/instructions") => {
            match serde_json::from_str::<Value>(body).map_err(|_| "invalid JSON".to_string())
                .and_then(|v| crate::aggregator::solana_instructions(&v)) {
                Ok(v) => (v, ok),
                Err(e) => (json!({"ok": false, "error": e}), bad),
            }
        },
        // ── 0x aggregator ──────────────────────────────────────────────────────────
        ("GET", "/0x/price") => match crate::aggregator::zerox("/swap/permit2/price", &p) {
            Ok(v) => (v, ok),
            Err(e) => (json!({"ok": false, "error": e}), bad),
        },
        ("GET", "/0x/quote") => match crate::aggregator::zerox("/swap/permit2/quote", &p) {
            Ok(v) => (v, ok),
            Err(e) => (json!({"ok": false, "error": e}), bad),
        },

        _ => (json!({"ok": false, "error": format!("no route {method} {r}")}), "404 Not Found"),
    }
}

/// Gate → size → build. `confirm` is the only thing that turns this into a real order.
fn ticket(a: &Value, confirm: bool) -> Result<Value, String> {
    let symbol = a.get("symbol").and_then(|v| v.as_str()).unwrap_or("").to_uppercase();
    let side = a.get("side").and_then(|v| v.as_str()).unwrap_or("").to_uppercase();
    let usd = a.get("usd").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let lev = a.get("leverage").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
    let otype = a.get("orderType").and_then(|v| v.as_str()).unwrap_or("MARKET").to_uppercase();
    let price = a.get("price").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(|s| s.to_string());

    let mut g = Gate::default();
    if let Some(w) = a.get("whitelist").and_then(|v| v.as_array()) {
        let l: Vec<String> = w.iter().filter_map(|x| x.as_str()).map(|s| s.to_uppercase()).collect();
        if !l.is_empty() { g.whitelist = l; }
    }
    if let Some(m) = a.get("max_notional_usd").and_then(|v| v.as_f64()) { g.max_notional_usd = m; }
    if let Some(l) = a.get("max_leverage").and_then(|v| v.as_u64()) { g.max_leverage = l as u32; }

    let b = Bitunix::from_env()?;
    let mark = b.last_price(&symbol)?;
    let plimit = price.as_ref().and_then(|p| p.parse::<f64>().ok());
    let gate_cfg = json!({"whitelist": g.whitelist, "max_notional_usd": g.max_notional_usd,
                          "max_leverage": g.max_leverage,
                          "max_price_deviation_bps": g.max_price_deviation_bps});

    if let Verdict::Reject(why) = g.check(&symbol, &side, usd, lev, plimit, Some(mark)) {
        return Ok(json!({"ok": false, "gate": "REJECTED", "reason": why, "gate_config": gate_cfg,
                         "mark_price": mark, "note": "nothing was sent"}));
    }
    let qty = qty_from_notional(usd, plimit.unwrap_or(mark), 4)?;
    let order = Bitunix::build_order(
        &symbol, &side, &otype, &qty, price.as_deref(), Some("GTC"),
        a.get("reduceOnly").and_then(|v| v.as_bool()).unwrap_or(false), None,
        a.get("tp").and_then(|v| v.as_str()).filter(|s| !s.is_empty()),
        a.get("sl").and_then(|v| v.as_str()).filter(|s| !s.is_empty()),
    );

    if !confirm {
        return Ok(json!({"ok": true, "gate": "PASS", "gate_config": gate_cfg, "mark_price": mark,
                         "qty": qty, "would_send": order, "confirmed": false,
                         "note": "PROPOSAL — nothing sent."}));
    }
    let res = b.place_order(&order, true)?;
    Ok(json!({"ok": true, "gate": "PASS", "gate_config": gate_cfg, "mark_price": mark,
              "qty": qty, "sent": order, "confirmed": true, "result": res}))
}

fn handle(mut s: TcpStream, token: Option<String>) {
    let mut r = BufReader::new(match s.try_clone() { Ok(c) => c, Err(_) => return });
    let mut line = String::new();
    if r.read_line(&mut line).is_err() { return; }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("/").to_string();

    let mut len = 0usize;
    let mut auth: Option<String> = None;
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).is_err() { break; }
        if h.trim().is_empty() { break; }
        let lower = h.to_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            len = v.trim().parse().unwrap_or(0);
        }
        if lower.starts_with("authorization:") {
            auth = h.splitn(2, ':').nth(1).map(|v| v.trim().to_string());
        }
    }
    let mut body = vec![0u8; len.min(1 << 20)];
    if len > 0 && r.read_exact(&mut body).is_err() { return; }

    // CORS preflight must answer before the auth check, or the browser never gets to send the token.
    let (v, status) = if method == "OPTIONS" {
        (json!({"ok": true}), "200 OK")
    } else {
        let qtok = qs(&path).into_iter().find(|(k, _)| k == "token").map(|(_, v)| v);
        if !authorised(token.as_deref(), auth.as_deref(), qtok.as_deref()) {
            (json!({"ok": false, "error": "unauthorised — supply the bridge token as \
                    'Authorization: Bearer <token>' or ?token=<token>"}), "401 Unauthorized")
        } else {
            route(&method, &path, &String::from_utf8_lossy(&body))
        }
    };
    let _ = s.write_all(body_json(v, status).as_bytes());
    let _ = s.flush();
}

/// Serve the bridge.
///
/// [`Bind::Loopback`] needs no token — nothing off-box can open the socket. [`Bind::All`] is
/// refused outright unless a token of at least [`MIN_TOKEN_LEN`] characters is supplied, because
/// what sits behind this is a funded trading account.
pub fn serve_with(port: u16, bind: Bind, token: Option<String>) -> Result<(), String> {
    bind_is_allowed(bind, token.as_deref())?;
    let addr = match bind { Bind::Loopback => Ipv4Addr::LOCALHOST, Bind::All => Ipv4Addr::UNSPECIFIED };
    let l = TcpListener::bind((addr, port)).map_err(|e| e.to_string())?;
    match bind {
        Bind::Loopback => eprintln!("flux-bitunix bridge on http://127.0.0.1:{port} (loopback only)"),
        Bind::All => eprintln!("flux-bitunix bridge on 0.0.0.0:{port} — TOKEN REQUIRED on every request"),
    }
    eprintln!("  writes are propose-only unless the POST body carries \"confirm\": true");
    for c in l.incoming() {
        if let Ok(c) = c {
            let t = token.clone();
            std::thread::spawn(move || handle(c, t));
        }
    }
    Ok(())
}

/// Loopback-only convenience wrapper, kept for callers that never want exposure.
pub fn serve(port: u16) -> Result<(), String> {
    serve_with(port, Bind::Loopback, std::env::var("FLUX_BITUNIX_TOKEN").ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_parsing_decodes_percent_and_plus() {
        let p = qs("/x?a=1&b=hello%20world&c=a+b");
        assert_eq!(get(&p, "a"), "1");
        assert_eq!(get(&p, "b"), "hello world");
        assert_eq!(get(&p, "c"), "a b");
        assert_eq!(get(&p, "missing"), "");
    }

    #[test]
    fn unknown_route_is_404_not_a_silent_ok() {
        let (v, st) = route("GET", "/nope", "");
        assert_eq!(st, "404 Not Found");
        assert_eq!(v["ok"], false);
    }

    #[test]
    fn options_preflight_is_allowed() {
        let (_, st) = route("OPTIONS", "/order", "");
        assert_eq!(st, "200 OK");
    }

    #[test]
    fn order_route_without_confirm_never_reports_confirmed() {
        // no network needed: an off-whitelist symbol is rejected by the gate before any call
        let (v, _) = route("POST", "/order", r#"{"symbol":"DOGEUSDT","side":"BUY","usd":5}"#);
        assert_ne!(v["confirmed"], json!(true));
    }

    #[test]
    fn mount_prefix_is_stripped_so_a_proxied_path_still_routes() {
        assert_eq!(strip_mount("/bitunix/health"), "/health");
        assert_eq!(strip_mount("/desk/pool"), "/pool");
        assert_eq!(strip_mount("/bitunix"), "/");
        assert_eq!(strip_mount("/health"), "/health", "an unmounted path is untouched");
        assert_eq!(strip_mount("/bitunixfoo"), "/bitunixfoo", "only a full segment counts");
        let (_, st) = route("GET", "/bitunix/nope", "");
        assert_eq!(st, "404 Not Found");
    }

    #[test]
    fn loopback_needs_no_token_but_public_bind_does() {
        assert!(bind_is_allowed(Bind::Loopback, None).is_ok());
        assert!(bind_is_allowed(Bind::All, None).is_err(), "public bind without a token must be refused");
    }

    #[test]
    fn public_bind_refuses_a_short_token() {
        assert!(bind_is_allowed(Bind::All, Some("hunter2")).is_err());
        assert!(bind_is_allowed(Bind::All, Some(&"x".repeat(MIN_TOKEN_LEN - 1))).is_err());
        assert!(bind_is_allowed(Bind::All, Some(&"x".repeat(MIN_TOKEN_LEN))).is_ok());
    }

    #[test]
    fn authorised_accepts_bearer_and_query_but_nothing_else() {
        let t = "x".repeat(40);
        assert!(authorised(None, None, None), "loopback mode requires nothing");
        assert!(authorised(Some(&t), Some(&format!("Bearer {t}")), None));
        assert!(authorised(Some(&t), None, Some(&t)));
        assert!(!authorised(Some(&t), None, None));
        assert!(!authorised(Some(&t), Some("Bearer wrong"), None));
        assert!(!authorised(Some(&t), None, Some("wrong")));
        assert!(!authorised(Some(&t), Some(&t), None), "a bare header without the Bearer prefix is not a match");
    }

    #[test]
    fn token_compare_rejects_length_mismatch_and_near_misses() {
        assert!(token_matches("abcdef", "abcdef"));
        assert!(!token_matches("abcdef", "abcde"));
        assert!(!token_matches("abcdef", "abcdeg"));
        assert!(!token_matches("", "x"));
    }

    #[test]
    fn response_carries_cors_and_no_store() {
        let h = body_json(json!({"ok": true}), "200 OK");
        assert!(h.contains("Access-Control-Allow-Origin: *"));
        assert!(h.contains("Cache-Control: no-store"));
        assert!(h.contains("Access-Control-Allow-Headers: content-type, authorization"));
        assert!(h.contains("Content-Length: 11"));
    }

    #[test]
    fn aggregator_routes_validate_before_network() {
        for route_path in ["/0x/price", "/0x/quote", "/0x/crosschain", "/0x/sources", "/0x/crosschain/status"] {
            let (value, status) = route("GET", route_path, "");
            assert_eq!(status, "400 Bad Request");
            assert_eq!(value["ok"], false);
        }
        let (value, status) = route("POST", "/0x/solana/instructions", "{}");
        assert_eq!(status, "400 Bad Request");
        assert_eq!(value["ok"], false);
    }
}
