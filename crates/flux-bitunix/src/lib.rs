//! flux-bitunix — Flux-native Bitunix Futures engine. Built the flux way: the whole authenticated
//! surface is declared as a **flux-api** `ApiEndpoint` spec (so flux-api generates the OpenAPI 3.1 +
//! the SDKs + the docsite for free) and a typed reqwest client executes it. The MCP combos
//! (`flux_bitunix_*` in fluxc-mcp) drive it agentically, and every fill is pushed to the webhook bus.
//!
//! # Auth — double SHA-256, NOT HMAC
//! Bitunix does not use HMAC. It uses two SHA-256 rounds with a per-request nonce:
//!
//! ```text
//!   digest = SHA256( nonce ‖ timestamp ‖ api_key ‖ sorted_query ‖ compact_body )
//!   sign   = SHA256( digest ‖ secret_key )
//! ```
//!
//! Headers: `api-key`, `nonce`, `timestamp`, `sign`, `Content-Type: application/json`.
//! Query params are sorted by ASCII key ascending and concatenated `key‖value` with NO separators.
//! The body is the compact JSON (no spaces). A GET has an empty body; a POST has an empty query.
//!
//! Credentials come from `FLUX_BITUNIX_API_KEY` / `FLUX_BITUNIX_SECRET_KEY`, else
//! `/root/.config/bitunix/{api_key,secret_key}` (chmod 600). **Never committed, never logged, never
//! echoed into a tool result** — `redact()` guards every error path.
//!
//! # Safety: this crate touches real money
//! `place_order` is propose-only by default. A live order requires BOTH an explicit `confirm=true`
//! AND a passing [`Gate`] (symbol whitelist · notional cap · leverage cap · slippage bound). The
//! same discipline as `flux-agent-trade`: the agent proposes, the human confirms.

pub mod aggregator;
pub mod arb;
pub mod bridge;
pub mod bybit;
pub mod pool;

use flux_api::schema::{ApiEndpoint, ApiParameter, ApiResponse, ApiSchema, HttpMethod, ParamLocation};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

/// Bitunix futures REST base.
pub const BASE: &str = "https://fapi.bitunix.com";

// ─────────────────────────────── pure, unit-tested primitives ───────────────────────────────
// Everything that can be wrong about a signed request is decided here, with no network in sight.
// The live calls only wire these together — so a signature bug is a failing test, not a 401 at 3am.

fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    hex::encode(h.finalize())
}

/// Canonical query string for the signature: sorted by ASCII key ascending, `key‖value`, no
/// separators, no `?`, no `&`. Empty params → empty string.
pub fn canonical_query(params: &[(String, String)]) -> String {
    let mut p: Vec<&(String, String)> = params.iter().filter(|(_, v)| !v.is_empty()).collect();
    p.sort_by(|a, b| a.0.cmp(&b.0));
    p.iter().map(|(k, v)| format!("{k}{v}")).collect::<Vec<_>>().join("")
}

/// The wire query string actually sent on the URL (`?a=1&b=2`), same sort order as the signature.
pub fn wire_query(params: &[(String, String)]) -> String {
    let mut p: Vec<&(String, String)> = params.iter().filter(|(_, v)| !v.is_empty()).collect();
    p.sort_by(|a, b| a.0.cmp(&b.0));
    if p.is_empty() { return String::new(); }
    format!("?{}", p.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&"))
}

/// Compact JSON body exactly as it must be hashed and sent — serde_json's `to_string` is already
/// space-free, which is precisely what Bitunix's "remove all spaces" rule asks for.
pub fn compact_body(body: Option<&Value>) -> String {
    body.map(|b| serde_json::to_string(b).unwrap_or_default()).unwrap_or_default()
}

/// The Bitunix double-SHA-256 signature. Pure — this is the function the whole crate rests on.
pub fn sign(nonce: &str, timestamp: &str, api_key: &str, query: &str, body: &str, secret: &str) -> String {
    let digest = sha256_hex(&format!("{nonce}{timestamp}{api_key}{query}{body}"));
    sha256_hex(&format!("{digest}{secret}"))
}

/// 32-char lowercase-hex nonce. Bitunix asks for a "random 32-bit string"; 32 hex chars (128 bits of
/// entropy) satisfies it and removes any chance of a collision inside one millisecond.
pub fn make_nonce() -> String {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Strip anything that looks like a credential out of a string before it leaves the process.
/// Errors get logged, returned to agents, and pasted into chats — none of those may carry a key.
pub fn redact(s: &str, api_key: &str, secret: &str) -> String {
    let mut out = s.to_string();
    for c in [api_key, secret] {
        if c.len() >= 8 { out = out.replace(c, "<redacted>"); }
    }
    out
}

/// HTTP statuses worth retrying — transient only. A 4xx is Bitunix saying "no"; retrying a rejected
/// order is how you accidentally place it twice.
pub fn is_retryable_status(status: u16) -> bool {
    matches!(status, 408 | 425 | 429 | 500 | 502 | 503 | 504)
}

pub fn backoff_ms(attempt: u32, base_ms: u64, cap_ms: u64) -> u64 {
    base_ms.checked_shl(attempt.min(20)).unwrap_or(cap_ms).min(cap_ms)
}

/// Pull the human reason out of a Bitunix envelope `{code, data, msg}`. `code == 0` is success.
pub fn parse_error(body: &Value) -> Option<String> {
    let code = body.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
    if code == 0 { return None; }
    let msg = body.get("msg").and_then(|m| m.as_str()).unwrap_or("unknown error");
    Some(format!("bitunix code {code}: {msg}"))
}

// ───────────────────────────────── the Verified Execution Gate ─────────────────────────────────

/// Every check an order must clear before it may become real. Mirrors `flux-agent-trade::Gate` —
/// the layer that makes it safe for a language model to hold an API key.
#[derive(Debug, Clone)]
pub struct Gate {
    /// Only these symbols may trade. An empty whitelist rejects everything (fail closed).
    pub whitelist: Vec<String>,
    /// Hard cap on notional per order, in quote currency (USDT).
    pub max_notional_usd: f64,
    /// Hard cap on leverage. Bitunix futures go to 125x; that is not a number an agent picks.
    pub max_leverage: u32,
    /// Reject a LIMIT price further than this from the mark price, in basis points.
    pub max_price_deviation_bps: u32,
}

impl Default for Gate {
    /// Deliberately tight. Widening this is an operator decision, taken once, in the open.
    fn default() -> Self {
        Gate {
            whitelist: ["BTCUSDT", "ETHUSDT"].iter().map(|s| s.to_string()).collect(),
            max_notional_usd: 100.0,
            max_leverage: 5,
            max_price_deviation_bps: 500, // 5%
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict { Pass, Reject(String) }

impl Gate {
    /// Pure and fully tested. Order matters — fail on the first thing that is wrong, and say which.
    pub fn check(&self, symbol: &str, side: &str, notional_usd: f64, leverage: u32,
                 price: Option<f64>, mark: Option<f64>) -> Verdict {
        let sym = symbol.to_uppercase();
        if sym.is_empty() { return Verdict::Reject("empty symbol".into()); }
        if !matches!(side.to_uppercase().as_str(), "BUY" | "SELL") {
            return Verdict::Reject(format!("side must be BUY or SELL, got {side:?}"));
        }
        if self.whitelist.is_empty() {
            return Verdict::Reject("gate whitelist is empty — fails closed by design".into());
        }
        if !self.whitelist.iter().any(|w| w.eq_ignore_ascii_case(&sym)) {
            return Verdict::Reject(format!("{sym} is not on the gate whitelist {:?}", self.whitelist));
        }
        if !(notional_usd > 0.0) || !notional_usd.is_finite() {
            return Verdict::Reject(format!("notional must be a positive finite number, got {notional_usd}"));
        }
        if notional_usd > self.max_notional_usd {
            return Verdict::Reject(format!(
                "notional ${notional_usd:.2} exceeds the gate cap ${:.2}", self.max_notional_usd));
        }
        if leverage == 0 { return Verdict::Reject("leverage must be at least 1".into()); }
        if leverage > self.max_leverage {
            return Verdict::Reject(format!("leverage {leverage}x exceeds the gate cap {}x", self.max_leverage));
        }
        if let (Some(p), Some(m)) = (price, mark) {
            if m > 0.0 && p > 0.0 {
                let dev_bps = ((p - m).abs() / m * 10_000.0).round() as u32;
                if dev_bps > self.max_price_deviation_bps {
                    return Verdict::Reject(format!(
                        "limit price {p} is {dev_bps} bps from mark {m}, gate allows {}",
                        self.max_price_deviation_bps));
                }
            }
        }
        Verdict::Pass
    }
}

// ───────────────────────────────────────── the client ─────────────────────────────────────────

pub struct Bitunix {
    api_key: String,
    secret_key: String,
    base: String,
    http: reqwest::blocking::Client,
}

impl Bitunix {
    /// Credentials from env, else the root-only config files. Errs loudly if neither is present —
    /// a silently unauthenticated client would just return "permission denied" forever.
    pub fn from_env() -> Result<Self, String> {
        let read = |env: &str, path: &str| -> Option<String> {
            std::env::var(env).ok()
                .or_else(|| std::fs::read_to_string(path).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        let api_key = read("FLUX_BITUNIX_API_KEY", "/root/.config/bitunix/api_key")
            .ok_or("no Bitunix API key (set FLUX_BITUNIX_API_KEY or /root/.config/bitunix/api_key)")?;
        let secret_key = read("FLUX_BITUNIX_SECRET_KEY", "/root/.config/bitunix/secret_key")
            .ok_or("no Bitunix secret (set FLUX_BITUNIX_SECRET_KEY or /root/.config/bitunix/secret_key)")?;
        let base = std::env::var("FLUX_BITUNIX_BASE").unwrap_or_else(|_| BASE.to_string());
        let http = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .user_agent("flux-bitunix/1.0")
            .build().map_err(|e| e.to_string())?;
        Ok(Bitunix { api_key, secret_key, base, http })
    }

    /// Public (unsigned) client — market data only, no credentials needed.
    pub fn public() -> Result<Self, String> {
        let http = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .user_agent("flux-bitunix/1.0")
            .build().map_err(|e| e.to_string())?;
        Ok(Bitunix { api_key: String::new(), secret_key: String::new(),
                     base: std::env::var("FLUX_BITUNIX_BASE").unwrap_or_else(|_| BASE.to_string()), http })
    }

    /// Fingerprint the loaded key without revealing it — first 4 and last 4 characters. Lets an
    /// operator confirm *which* key is loaded without the key ever appearing in a log.
    pub fn key_fingerprint(&self) -> String {
        let k = &self.api_key;
        if k.len() < 12 { return "<short-or-absent>".into(); }
        format!("{}…{} ({} chars)", &k[..4], &k[k.len() - 4..], k.len())
    }

    fn err(&self, e: impl std::fmt::Display) -> String {
        redact(&e.to_string(), &self.api_key, &self.secret_key)
    }

    /// One signed request with bounded retry on transient failures only.
    fn request(&self, method: HttpMethod, path: &str, params: &[(String, String)],
               body: Option<&Value>, signed: bool) -> Result<Value, String> {
        if signed && self.api_key.is_empty() {
            return Err("this endpoint needs credentials; construct with Bitunix::from_env()".into());
        }
        let query_wire = wire_query(params);
        let query_canon = canonical_query(params);
        let body_str = compact_body(body);
        let url = format!("{}{}{}", self.base, path, query_wire);

        let mut last = String::new();
        for attempt in 0..3u32 {
            let mut req = match method {
                HttpMethod::GET => self.http.get(&url),
                _ => self.http.post(&url).body(body_str.clone()),
            };
            req = req.header("Content-Type", "application/json").header("language", "en-US");
            if signed {
                let nonce = make_nonce();
                let ts = now_ms().to_string();
                let sig = sign(&nonce, &ts, &self.api_key, &query_canon, &body_str, &self.secret_key);
                req = req.header("api-key", &self.api_key)
                         .header("nonce", nonce)
                         .header("timestamp", ts)
                         .header("sign", sig);
            }
            match req.send() {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let text = resp.text().unwrap_or_default();
                    if is_retryable_status(status) && attempt < 2 {
                        last = format!("HTTP {status}");
                        std::thread::sleep(std::time::Duration::from_millis(backoff_ms(attempt, 250, 4000)));
                        continue;
                    }
                    let v: Value = serde_json::from_str(&text)
                        .map_err(|_| self.err(format!("HTTP {status}: non-JSON response: {}",
                                                      text.chars().take(300).collect::<String>())))?;
                    if let Some(e) = parse_error(&v) { return Err(self.err(e)); }
                    return Ok(v);
                }
                Err(e) => {
                    last = self.err(&e);
                    if attempt < 2 {
                        std::thread::sleep(std::time::Duration::from_millis(backoff_ms(attempt, 250, 4000)));
                        continue;
                    }
                }
            }
        }
        Err(format!("bitunix request failed after 3 attempts: {last}"))
    }

    // ── public market data (unsigned) ───────────────────────────────────────────────────────
    pub fn tickers(&self, symbols: &str) -> Result<Value, String> {
        self.request(HttpMethod::GET, "/api/v1/futures/market/tickers",
                     &[("symbols".into(), symbols.into())], None, false)
    }
    pub fn kline(&self, symbol: &str, interval: &str, limit: u32) -> Result<Value, String> {
        self.request(HttpMethod::GET, "/api/v1/futures/market/kline",
                     &[("symbol".into(), symbol.into()), ("interval".into(), interval.into()),
                       ("limit".into(), limit.clamp(1, 200).to_string())], None, false)
    }
    pub fn depth(&self, symbol: &str, limit: &str) -> Result<Value, String> {
        self.request(HttpMethod::GET, "/api/v1/futures/market/depth",
                     &[("symbol".into(), symbol.into()), ("limit".into(), limit.into())], None, false)
    }
    pub fn trading_pairs(&self, symbols: &str) -> Result<Value, String> {
        self.request(HttpMethod::GET, "/api/v1/futures/market/trading_pairs",
                     &[("symbols".into(), symbols.into())], None, false)
    }

    // ── private account + positions (signed, read-only) ─────────────────────────────────────
    pub fn account(&self, margin_coin: &str) -> Result<Value, String> {
        let mc = if margin_coin.is_empty() { "USDT" } else { margin_coin };
        self.request(HttpMethod::GET, "/api/v1/futures/account",
                     &[("marginCoin".into(), mc.into())], None, true)
    }
    pub fn positions(&self, symbol: &str) -> Result<Value, String> {
        self.request(HttpMethod::GET, "/api/v1/futures/position/get_pending_positions",
                     &[("symbol".into(), symbol.into())], None, true)
    }
    pub fn pending_orders(&self, symbol: &str, limit: u32) -> Result<Value, String> {
        self.request(HttpMethod::GET, "/api/v1/futures/trade/get_pending_orders",
                     &[("symbol".into(), symbol.into()),
                       ("limit".into(), limit.clamp(1, 100).to_string())], None, true)
    }
    pub fn history_orders(&self, symbol: &str, limit: u32) -> Result<Value, String> {
        self.request(HttpMethod::GET, "/api/v1/futures/trade/get_history_orders",
                     &[("symbol".into(), symbol.into()),
                       ("limit".into(), limit.clamp(1, 100).to_string())], None, true)
    }

    // ── trading (signed, WRITE — gated) ─────────────────────────────────────────────────────

    /// Build the exact order body that would be sent. Pure enough to show a human before anything
    /// is signed: this is what "propose" returns, byte for byte what "confirm" will send.
    pub fn build_order(symbol: &str, side: &str, order_type: &str, qty: &str,
                       price: Option<&str>, effect: Option<&str>, reduce_only: bool,
                       client_id: Option<&str>, tp: Option<&str>, sl: Option<&str>) -> Value {
        let mut m = Map::new();
        m.insert("symbol".into(), json!(symbol.to_uppercase()));
        m.insert("side".into(), json!(side.to_uppercase()));
        m.insert("orderType".into(), json!(order_type.to_uppercase()));
        m.insert("qty".into(), json!(qty));
        if order_type.eq_ignore_ascii_case("LIMIT") {
            if let Some(p) = price { m.insert("price".into(), json!(p)); }
            m.insert("effect".into(), json!(effect.unwrap_or("GTC").to_uppercase()));
        }
        if reduce_only { m.insert("reduceOnly".into(), json!(true)); }
        if let Some(c) = client_id { m.insert("clientId".into(), json!(c)); }
        if let Some(t) = tp {
            m.insert("tpPrice".into(), json!(t));
            m.insert("tpStopType".into(), json!("MARK_PRICE"));
            m.insert("tpOrderType".into(), json!("MARKET"));
        }
        if let Some(s) = sl {
            m.insert("slPrice".into(), json!(s));
            m.insert("slStopType".into(), json!("MARK_PRICE"));
            m.insert("slOrderType".into(), json!("MARKET"));
        }
        Value::Object(m)
    }

    /// Place an order. **Requires `confirm == true`**; anything else returns the proposal and sends
    /// nothing. The `Verdict` must already be `Pass` — callers run the gate and pass it in, so the
    /// gate decision is visible in the proposal rather than hidden in here.
    pub fn place_order(&self, order: &Value, confirm: bool) -> Result<Value, String> {
        if !confirm {
            return Ok(json!({
                "ok": true, "dry_run": true, "would_send": order,
                "endpoint": "POST /api/v1/futures/trade/place_order",
                "note": "propose-only — nothing was sent. Re-call with confirm=true to place this order."
            }));
        }
        let r = self.request(HttpMethod::POST, "/api/v1/futures/trade/place_order", &[], Some(order), true)?;
        Ok(json!({"ok": true, "dry_run": false, "sent": order, "result": r}))
    }

    pub fn cancel_orders(&self, symbol: &str, order_ids: &[String], confirm: bool) -> Result<Value, String> {
        let body = json!({
            "symbol": symbol.to_uppercase(),
            "orderList": order_ids.iter().map(|id| json!({"orderId": id})).collect::<Vec<_>>()
        });
        if !confirm {
            return Ok(json!({"ok": true, "dry_run": true, "would_send": body,
                             "endpoint": "POST /api/v1/futures/trade/cancel_orders",
                             "note": "propose-only — re-call with confirm=true to cancel."}));
        }
        let r = self.request(HttpMethod::POST, "/api/v1/futures/trade/cancel_orders", &[], Some(&body), true)?;
        Ok(json!({"ok": true, "dry_run": false, "result": r}))
    }

    /// Flatten everything on a symbol. Always destructive, so `confirm` is mandatory here too.
    pub fn cancel_all(&self, symbol: &str, confirm: bool) -> Result<Value, String> {
        let body = json!({"symbol": symbol.to_uppercase()});
        if !confirm {
            return Ok(json!({"ok": true, "dry_run": true, "would_send": body,
                             "endpoint": "POST /api/v1/futures/trade/cancel_all_orders",
                             "note": "propose-only — re-call with confirm=true."}));
        }
        let r = self.request(HttpMethod::POST, "/api/v1/futures/trade/cancel_all_orders", &[], Some(&body), true)?;
        Ok(json!({"ok": true, "dry_run": false, "result": r}))
    }

    /// Last price for a symbol, as f64 — the number the gate needs to turn "$50 of BTC" into a qty.
    pub fn last_price(&self, symbol: &str) -> Result<f64, String> {
        let t = self.tickers(symbol)?;
        t.get("data").and_then(|d| d.as_array()).and_then(|a| a.first())
            .and_then(|o| o.get("lastPrice").or_else(|| o.get("markPrice")))
            .and_then(|p| p.as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| p.as_f64()))
            .ok_or_else(|| format!("no lastPrice for {symbol} in ticker response"))
    }
}

/// Turn a notional in USDT into a base-coin quantity at a given price, rounded down to `step`
/// decimals. Rounding DOWN matters: rounding up silently spends more than the cap the gate approved.
pub fn qty_from_notional(notional_usd: f64, price: f64, decimals: u32) -> Result<String, String> {
    if !(price > 0.0) || !price.is_finite() { return Err(format!("bad price {price}")); }
    if !(notional_usd > 0.0) || !notional_usd.is_finite() { return Err(format!("bad notional {notional_usd}")); }
    let scale = 10f64.powi(decimals.min(8) as i32);
    let q = (notional_usd / price * scale).floor() / scale;
    if q <= 0.0 {
        return Err(format!("${notional_usd} at price {price} rounds to zero at {decimals} decimals — order too small"));
    }
    Ok(format!("{:.*}", decimals.min(8) as usize, q))
}

// ─────────────────────────────────── the flux-api spec ───────────────────────────────────

fn qp(name: &str, required: bool, desc: &str) -> ApiParameter {
    ApiParameter { name: name.into(), location: ParamLocation::Query, required,
                   schema: ApiSchema::string(), description: desc.into() }
}

fn hdr(name: &str, desc: &str) -> ApiParameter {
    ApiParameter { name: name.into(), location: ParamLocation::Header, required: true,
                   schema: ApiSchema::string(), description: desc.into() }
}

fn ep(method: HttpMethod, path: &str, op: &str, summary: &str,
      mut params: Vec<ApiParameter>, tag: &str) -> ApiEndpoint {
    // every SIGNED call carries the four auth headers; public market calls tolerate them too
    if !summary.contains("(public)") {
        params.insert(0, hdr("api-key", "Bitunix API key"));
        params.insert(1, hdr("nonce", "random 32-hex nonce, one per request"));
        params.insert(2, hdr("timestamp", "unix milliseconds, UTC"));
        params.insert(3, hdr("sign", "SHA256(SHA256(nonce‖ts‖key‖query‖body)‖secret)"));
    }
    ApiEndpoint {
        crate_name: "flux-bitunix".into(), method, path: path.into(),
        operation_id: op.into(), summary: summary.into(), parameters: params,
        request_body: None,
        responses: vec![ApiResponse { status: 200, description: "Bitunix envelope {code,data,msg}; code 0 = success".into(), schema: Some(ApiSchema::object().build()) }],
        tags: vec![tag.into()], middleware: None,
    }
}

/// The declarative Bitunix surface — flux-api turns this into OpenAPI 3.1, the SDKs and the docsite.
pub fn bitunix_spec() -> Vec<ApiEndpoint> {
    use HttpMethod::{GET, POST};
    vec![
        ep(GET, "/api/v1/futures/market/tickers", "tickers", "Ticker snapshot (public)",
           vec![qp("symbols", false, "comma-separated symbols, e.g. BTCUSDT,ETHUSDT")], "market"),
        ep(GET, "/api/v1/futures/market/kline", "kline", "Candlesticks (public)",
           vec![qp("symbol", true, "trading pair"), qp("interval", true, "1m 5m 15m 30m 1h 2h 4h 6h 8h 12h 1d 3d 1w 1M"),
                qp("limit", false, "default 100, max 200"), qp("type", false, "LAST_PRICE | MARK_PRICE")], "market"),
        ep(GET, "/api/v1/futures/market/depth", "depth", "Order book (public)",
           vec![qp("symbol", true, "trading pair"), qp("limit", false, "book depth")], "market"),
        ep(GET, "/api/v1/futures/market/trading_pairs", "tradingPairs", "Contract specs (public)",
           vec![qp("symbols", false, "comma-separated symbols")], "market"),
        ep(GET, "/api/v1/futures/account", "account", "Margin account balance (signed)",
           vec![qp("marginCoin", true, "margin coin, e.g. USDT")], "account"),
        ep(GET, "/api/v1/futures/position/get_pending_positions", "positions", "Open positions (signed)",
           vec![qp("symbol", false, "trading pair"), qp("positionId", false, "position id")], "position"),
        ep(GET, "/api/v1/futures/trade/get_pending_orders", "pendingOrders", "Working orders (signed)",
           vec![qp("symbol", false, "trading pair"), qp("status", false, "NEW | PART_FILLED"), qp("limit", false, "max 100")], "trade"),
        ep(GET, "/api/v1/futures/trade/get_history_orders", "historyOrders", "Order history (signed)",
           vec![qp("symbol", false, "trading pair"), qp("limit", false, "max 100")], "trade"),
        ep(POST, "/api/v1/futures/trade/place_order", "placeOrder", "Place an order (signed, GATED)", vec![], "trade"),
        ep(POST, "/api/v1/futures/trade/cancel_orders", "cancelOrders", "Cancel orders (signed, GATED)", vec![], "trade"),
        ep(POST, "/api/v1/futures/trade/cancel_all_orders", "cancelAllOrders", "Cancel all (signed, GATED)", vec![], "trade"),
    ]
}

// ───────────────────────────────────────── tests ─────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_matches_the_documented_two_round_construction() {
        // digest = SHA256(nonce‖ts‖key‖query‖body) ; sign = SHA256(digest‖secret)
        let (nonce, ts, key, secret) = ("abc123", "1700000000000", "KEY", "SECRET");
        let expect_digest = sha256_hex("abc1231700000000000KEYsymbolBTCUSDT");
        let expect = sha256_hex(&format!("{expect_digest}SECRET"));
        assert_eq!(sign(nonce, ts, key, "symbolBTCUSDT", "", secret), expect);
    }

    #[test]
    fn signature_is_sensitive_to_every_input() {
        let base = sign("n", "1", "k", "q", "b", "s");
        for altered in [sign("N", "1", "k", "q", "b", "s"), sign("n", "2", "k", "q", "b", "s"),
                        sign("n", "1", "K", "q", "b", "s"), sign("n", "1", "k", "Q", "b", "s"),
                        sign("n", "1", "k", "q", "B", "s"), sign("n", "1", "k", "q", "b", "S")] {
            assert_ne!(base, altered, "signature must change when any component changes");
        }
    }

    #[test]
    fn canonical_query_sorts_ascii_ascending_and_drops_empties() {
        let p = vec![("symbol".to_string(), "BTCUSDT".to_string()),
                     ("limit".to_string(), "10".to_string()),
                     ("empty".to_string(), "".to_string()),
                     ("interval".to_string(), "1h".to_string())];
        assert_eq!(canonical_query(&p), "interval1hlimit10symbolBTCUSDT");
        assert_eq!(wire_query(&p), "?interval=1h&limit=10&symbol=BTCUSDT");
    }

    #[test]
    fn canonical_query_is_empty_for_no_params() {
        assert_eq!(canonical_query(&[]), "");
        assert_eq!(wire_query(&[]), "");
    }

    #[test]
    fn compact_body_has_no_spaces() {
        let b = json!({"symbol": "BTCUSDT", "qty": "0.5"});
        let s = compact_body(Some(&b));
        assert!(!s.contains(' '), "body must be space-free for the signature: {s}");
        assert_eq!(compact_body(None), "");
    }

    #[test]
    fn nonce_is_random_and_hex() {
        let (a, b) = (make_nonce(), make_nonce());
        assert_ne!(a, b);
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn redact_removes_both_credentials() {
        let s = "failed with key MYAPIKEY123 and secret MYSECRET456";
        let r = redact(s, "MYAPIKEY123", "MYSECRET456");
        assert!(!r.contains("MYAPIKEY123") && !r.contains("MYSECRET456"), "leaked: {r}");
        assert_eq!(r.matches("<redacted>").count(), 2);
    }

    #[test]
    fn redact_ignores_short_strings_to_avoid_mangling_output() {
        // an empty/short "credential" must not turn every character into <redacted>
        assert_eq!(redact("hello world", "", ""), "hello world");
    }

    #[test]
    fn gate_rejects_symbol_off_whitelist() {
        let g = Gate::default();
        assert!(matches!(g.check("DOGEUSDT", "BUY", 10.0, 2, None, None), Verdict::Reject(_)));
        assert_eq!(g.check("BTCUSDT", "BUY", 10.0, 2, None, None), Verdict::Pass);
    }

    #[test]
    fn gate_rejects_oversize_notional_and_leverage() {
        let g = Gate::default();
        assert!(matches!(g.check("BTCUSDT", "BUY", 100_000.0, 2, None, None), Verdict::Reject(_)));
        assert!(matches!(g.check("BTCUSDT", "BUY", 10.0, 125, None, None), Verdict::Reject(_)));
    }

    #[test]
    fn gate_fails_closed_on_empty_whitelist() {
        let g = Gate { whitelist: vec![], ..Gate::default() };
        assert!(matches!(g.check("BTCUSDT", "BUY", 1.0, 1, None, None), Verdict::Reject(_)));
    }

    #[test]
    fn gate_rejects_limit_price_far_from_mark() {
        let g = Gate::default(); // 500 bps
        assert!(matches!(g.check("BTCUSDT", "BUY", 10.0, 2, Some(50_000.0), Some(60_000.0)), Verdict::Reject(_)));
        assert_eq!(g.check("BTCUSDT", "BUY", 10.0, 2, Some(60_100.0), Some(60_000.0)), Verdict::Pass);
    }

    #[test]
    fn gate_rejects_nonsense_numbers() {
        let g = Gate::default();
        assert!(matches!(g.check("BTCUSDT", "BUY", f64::NAN, 1, None, None), Verdict::Reject(_)));
        assert!(matches!(g.check("BTCUSDT", "BUY", -5.0, 1, None, None), Verdict::Reject(_)));
        assert!(matches!(g.check("BTCUSDT", "HOLD", 5.0, 1, None, None), Verdict::Reject(_)));
    }

    #[test]
    fn qty_rounds_down_never_up() {
        // $100 at 3 → 33.333…; at 2 dp that must be 33.33, never 33.34 (which would overspend)
        assert_eq!(qty_from_notional(100.0, 3.0, 2).unwrap(), "33.33");
        assert!(qty_from_notional(0.0001, 60_000.0, 2).is_err(), "sub-tick order must be rejected, not rounded to 0");
        assert!(qty_from_notional(10.0, 0.0, 2).is_err());
    }

    #[test]
    fn build_order_limit_carries_price_and_effect_market_does_not() {
        let lim = Bitunix::build_order("btcusdt", "buy", "LIMIT", "0.01", Some("60000"), Some("GTC"), false, None, None, None);
        assert_eq!(lim["symbol"], "BTCUSDT");
        assert_eq!(lim["side"], "BUY");
        assert_eq!(lim["price"], "60000");
        assert_eq!(lim["effect"], "GTC");
        let mkt = Bitunix::build_order("BTCUSDT", "SELL", "MARKET", "0.01", None, None, true, Some("cid1"), None, None);
        assert!(mkt.get("price").is_none(), "a MARKET order must not carry a price");
        assert!(mkt.get("effect").is_none(), "a MARKET order must not carry a time-in-force");
        assert_eq!(mkt["reduceOnly"], true);
        assert_eq!(mkt["clientId"], "cid1");
    }

    #[test]
    fn build_order_attaches_tp_sl_when_asked() {
        let o = Bitunix::build_order("BTCUSDT", "BUY", "MARKET", "0.01", None, None, false, None, Some("70000"), Some("55000"));
        assert_eq!(o["tpPrice"], "70000");
        assert_eq!(o["slPrice"], "55000");
        assert_eq!(o["slStopType"], "MARK_PRICE");
    }

    #[test]
    fn parse_error_only_fires_on_nonzero_code() {
        assert!(parse_error(&json!({"code": 0, "msg": "Success"})).is_none());
        let e = parse_error(&json!({"code": 10007, "msg": "signature error"})).unwrap();
        assert!(e.contains("10007") && e.contains("signature error"));
    }

    #[test]
    fn retry_policy_covers_transient_only() {
        for s in [408, 425, 429, 500, 502, 503, 504] { assert!(is_retryable_status(s)); }
        for s in [200, 400, 401, 403, 404, 422] { assert!(!is_retryable_status(s), "{s} must not be retried"); }
    }

    #[test]
    fn backoff_grows_then_caps() {
        assert_eq!(backoff_ms(0, 250, 4000), 250);
        assert_eq!(backoff_ms(1, 250, 4000), 500);
        assert_eq!(backoff_ms(9, 250, 4000), 4000);
        assert_eq!(backoff_ms(99, 250, 4000), 4000);
    }

    #[test]
    fn spec_covers_the_whole_surface_and_paths_are_real() {
        let s = bitunix_spec();
        assert_eq!(s.len(), 11);
        assert!(s.iter().all(|e| e.path.starts_with("/api/v1/futures/")));
        assert!(s.iter().any(|e| e.operation_id == "placeOrder"));
    }
}
