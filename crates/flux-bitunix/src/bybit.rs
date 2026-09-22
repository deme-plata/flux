//! Bybit V5 — the second venue.
//!
//! Two exchanges is not twice one exchange. A single venue only lets you take a price; two venues
//! let you compare one against the other, and against the on-chain pool, which is what turns this
//! from a trading toy into something that can spot a real spread. That is what [`crate::arb`] does
//! with the readings this module produces.
//!
//! # Auth — HMAC-SHA256, and NOT the same as Bitunix
//! Bitunix signs with two rounds of plain SHA-256 and a nonce. Bybit signs with HMAC-SHA256 over
//!
//! ```text
//!   timestamp ‖ api_key ‖ recv_window ‖ (queryString for GET | raw JSON body for POST)
//! ```
//!
//! Headers: `X-BAPI-API-KEY`, `X-BAPI-TIMESTAMP`, `X-BAPI-RECV-WINDOW`, `X-BAPI-SIGN`.
//! The two schemes share no code on purpose — a "unified signer" that tried to cover both would be
//! one `if` away from signing a Bybit order with a Bitunix digest and failing in a way that looks
//! like a permissions problem.
//!
//! Timestamp discipline matters here: Bybit requires
//! `server_time - recv_window <= ts < server_time + 1000`, so a box with a drifting clock gets
//! `10002 invalid timestamp` and no amount of re-signing helps. [`Bybit::server_time_skew_ms`]
//! measures the drift rather than guessing at it.
//!
//! Credentials: `FLUX_BYBIT_API_KEY` / `FLUX_BYBIT_SECRET_KEY`, else the plain files
//! `api_key` / `secret_key` in the bybit config directory, else the older two-line
//! `api_key=…` / `api_secret=…` `credentials` file that was already on this box.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const BASE: &str = "https://api.bybit.com";
pub const RECV_WINDOW: &str = "5000";

// ───────────────────────────── HMAC-SHA256, self-contained ─────────────────────────────
// Built on the sha2 already vendored here rather than pulling a new dependency in. HMAC is a
// fully specified construction (RFC 2104), and the tests below pin it to the RFC 4231 vectors —
// if this were subtly wrong, those fail loudly rather than producing a plausible-looking hex string.

const BLOCK: usize = 64;

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().into()
}

/// HMAC-SHA256 as hex. `key` longer than the block size is hashed first, shorter is zero-padded.
pub fn hmac_sha256_hex(key: &[u8], msg: &[u8]) -> String {
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&sha256(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let mut inner = Vec::with_capacity(BLOCK + msg.len());
    inner.extend_from_slice(&ipad);
    inner.extend_from_slice(msg);
    let inner_hash = sha256(&inner);

    let mut outer = Vec::with_capacity(BLOCK + 32);
    outer.extend_from_slice(&opad);
    outer.extend_from_slice(&inner_hash);
    hex::encode(sha256(&outer))
}

/// The exact string Bybit V5 signs. Kept separate from the HMAC so the *ordering* — the part that
/// is easy to get wrong and impossible to debug from the error message — is testable on its own.
pub fn sign_payload(timestamp: &str, api_key: &str, recv_window: &str, tail: &str) -> String {
    format!("{timestamp}{api_key}{recv_window}{tail}")
}

/// Query string in the order given. Bybit signs the string it receives, so this must be byte-identical
/// to what goes on the URL — no sorting, no re-encoding between signing and sending.
pub fn query_string(params: &[(String, String)]) -> String {
    params
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// Bybit wraps everything in `{retCode, retMsg, result}`. `retCode == 0` is success.
pub fn parse_error(body: &Value) -> Option<String> {
    let code = body.get("retCode").and_then(|c| c.as_i64()).unwrap_or(0);
    if code == 0 {
        return None;
    }
    let msg = body.get("retMsg").and_then(|m| m.as_str()).unwrap_or("unknown error");
    let hint = match code {
        10002 => " — the local clock is outside Bybit's accepted window; check server_time_skew_ms",
        10003 | 10004 => " — API key or signature rejected",
        10005 => " — this key lacks permission for that endpoint",
        _ => "",
    };
    Some(format!("bybit retCode {code}: {msg}{hint}"))
}

pub struct Bybit {
    api_key: String,
    secret_key: String,
    base: String,
    http: reqwest::blocking::Client,
}

impl Bybit {
    pub fn from_env() -> Result<Self, String> {
        let (api_key, secret_key) = load_credentials()?;
        let base = std::env::var("FLUX_BYBIT_BASE").unwrap_or_else(|_| BASE.to_string());
        let http = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .user_agent("flux-bybit/1.0")
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Bybit { api_key, secret_key, base, http })
    }

    pub fn public() -> Result<Self, String> {
        let http = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .user_agent("flux-bybit/1.0")
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Bybit {
            api_key: String::new(),
            secret_key: String::new(),
            base: std::env::var("FLUX_BYBIT_BASE").unwrap_or_else(|_| BASE.to_string()),
            http,
        })
    }

    pub fn key_fingerprint(&self) -> String {
        let k = &self.api_key;
        if k.len() < 8 { return "<short-or-absent>".into(); }
        format!("{}…{} ({} chars)", &k[..4], &k[k.len() - 4..], k.len())
    }

    fn err(&self, e: impl std::fmt::Display) -> String {
        crate::redact(&e.to_string(), &self.api_key, &self.secret_key)
    }

    fn request(&self, method: &str, path: &str, params: &[(String, String)],
               body: Option<&Value>, signed: bool) -> Result<Value, String> {
        if signed && self.api_key.is_empty() {
            return Err("this endpoint needs Bybit credentials".into());
        }
        let qs = query_string(params);
        let body_str = body.map(|b| serde_json::to_string(b).unwrap_or_default()).unwrap_or_default();
        let url = if qs.is_empty() {
            format!("{}{path}", self.base)
        } else {
            format!("{}{path}?{qs}", self.base)
        };

        let mut last = String::new();
        for attempt in 0..3u32 {
            let mut req = match method {
                "POST" => self.http.post(&url).body(body_str.clone()),
                _ => self.http.get(&url),
            };
            req = req.header("Content-Type", "application/json");
            if signed {
                let ts = crate::now_ms().to_string();
                let tail = if method == "POST" { body_str.as_str() } else { qs.as_str() };
                let payload = sign_payload(&ts, &self.api_key, RECV_WINDOW, tail);
                let sign = hmac_sha256_hex(self.secret_key.as_bytes(), payload.as_bytes());
                req = req
                    .header("X-BAPI-API-KEY", &self.api_key)
                    .header("X-BAPI-TIMESTAMP", ts)
                    .header("X-BAPI-RECV-WINDOW", RECV_WINDOW)
                    .header("X-BAPI-SIGN", sign);
            }
            match req.send() {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let text = resp.text().unwrap_or_default();
                    if crate::is_retryable_status(status) && attempt < 2 {
                        last = format!("HTTP {status}");
                        std::thread::sleep(std::time::Duration::from_millis(
                            crate::backoff_ms(attempt, 250, 4000)));
                        continue;
                    }
                    let v: Value = serde_json::from_str(&text).map_err(|_| {
                        self.err(format!("HTTP {status}: non-JSON: {}",
                                         text.chars().take(240).collect::<String>()))
                    })?;
                    if let Some(e) = parse_error(&v) { return Err(self.err(e)); }
                    return Ok(v);
                }
                Err(e) => {
                    last = self.err(&e);
                    if attempt < 2 {
                        std::thread::sleep(std::time::Duration::from_millis(
                            crate::backoff_ms(attempt, 250, 4000)));
                        continue;
                    }
                }
            }
        }
        Err(format!("bybit request failed after 3 attempts: {last}"))
    }

    // ── public ────────────────────────────────────────────────────────────────────────
    /// `category` is one of spot | linear | inverse | option.
    pub fn tickers(&self, category: &str, symbol: &str) -> Result<Value, String> {
        self.request("GET", "/v5/market/tickers",
            &[("category".into(), cat(category)), ("symbol".into(), symbol.to_uppercase())],
            None, false)
    }
    pub fn kline(&self, category: &str, symbol: &str, interval: &str, limit: u32) -> Result<Value, String> {
        self.request("GET", "/v5/market/kline",
            &[("category".into(), cat(category)), ("symbol".into(), symbol.to_uppercase()),
              ("interval".into(), interval.into()), ("limit".into(), limit.clamp(1, 1000).to_string())],
            None, false)
    }
    pub fn orderbook(&self, category: &str, symbol: &str, limit: u32) -> Result<Value, String> {
        self.request("GET", "/v5/market/orderbook",
            &[("category".into(), cat(category)), ("symbol".into(), symbol.to_uppercase()),
              ("limit".into(), limit.clamp(1, 200).to_string())], None, false)
    }
    /// Bybit's own clock, for the drift check below.
    pub fn server_time_ms(&self) -> Result<u64, String> {
        let v = self.request("GET", "/v5/market/time", &[], None, false)?;
        v.get("result").and_then(|r| r.get("timeNano")).and_then(|t| t.as_str())
            .and_then(|s| s.parse::<u128>().ok()).map(|n| (n / 1_000_000) as u64)
            .or_else(|| v.get("time").and_then(|t| t.as_u64()))
            .ok_or_else(|| "no time in response".to_string())
    }
    /// Local clock minus Bybit's, in ms. Anything beyond the recv window makes every signed call
    /// fail with retCode 10002 no matter how correct the signature is.
    pub fn server_time_skew_ms(&self) -> Result<i64, String> {
        let theirs = self.server_time_ms()? as i64;
        Ok(crate::now_ms() as i64 - theirs)
    }

    // ── signed reads ──────────────────────────────────────────────────────────────────
    pub fn wallet_balance(&self, account_type: &str) -> Result<Value, String> {
        let at = if account_type.is_empty() { "UNIFIED" } else { account_type };
        self.request("GET", "/v5/account/wallet-balance",
            &[("accountType".into(), at.to_uppercase())], None, true)
    }
    pub fn positions(&self, category: &str, symbol: &str) -> Result<Value, String> {
        let mut p = vec![("category".into(), cat(category))];
        if symbol.is_empty() { p.push(("settleCoin".into(), "USDT".into())); }
        else { p.push(("symbol".into(), symbol.to_uppercase())); }
        self.request("GET", "/v5/position/list", &p, None, true)
    }
    pub fn open_orders(&self, category: &str, symbol: &str) -> Result<Value, String> {
        let mut p = vec![("category".into(), cat(category))];
        if !symbol.is_empty() { p.push(("symbol".into(), symbol.to_uppercase())); }
        else { p.push(("settleCoin".into(), "USDT".into())); }
        self.request("GET", "/v5/order/realtime", &p, None, true)
    }

    // ── trading (GATED) ───────────────────────────────────────────────────────────────
    /// The exact order body. Same contract as the Bitunix side: a proposal shows this verbatim.
    pub fn build_order(category: &str, symbol: &str, side: &str, order_type: &str, qty: &str,
                       price: Option<&str>, reduce_only: bool,
                       tp: Option<&str>, sl: Option<&str>) -> Value {
        let mut m = serde_json::Map::new();
        m.insert("category".into(), json!(cat(category)));
        m.insert("symbol".into(), json!(symbol.to_uppercase()));
        // Bybit wants Buy/Sell in title case; BUY is rejected.
        m.insert("side".into(), json!(title_case(side)));
        m.insert("orderType".into(), json!(title_case(order_type)));
        m.insert("qty".into(), json!(qty));
        if order_type.eq_ignore_ascii_case("limit") {
            if let Some(p) = price { m.insert("price".into(), json!(p)); }
            m.insert("timeInForce".into(), json!("GTC"));
        }
        if reduce_only { m.insert("reduceOnly".into(), json!(true)); }
        if let Some(t) = tp { m.insert("takeProfit".into(), json!(t)); }
        if let Some(s) = sl { m.insert("stopLoss".into(), json!(s)); }
        Value::Object(m)
    }

    /// Refuses without `confirm`. Callers run [`crate::Gate`] first and pass the verdict in, so the
    /// decision is visible in the proposal rather than buried here.
    pub fn place_order(&self, order: &Value, confirm: bool) -> Result<Value, String> {
        if !confirm {
            return Ok(json!({"ok": true, "dry_run": true, "would_send": order,
                             "endpoint": "POST /v5/order/create",
                             "note": "propose-only — nothing sent. Re-call with confirm=true."}));
        }
        let r = self.request("POST", "/v5/order/create", &[], Some(order), true)?;
        Ok(json!({"ok": true, "dry_run": false, "sent": order, "result": r}))
    }

    pub fn cancel_order(&self, category: &str, symbol: &str, order_id: &str, confirm: bool) -> Result<Value, String> {
        let body = json!({"category": cat(category), "symbol": symbol.to_uppercase(), "orderId": order_id});
        if !confirm {
            return Ok(json!({"ok": true, "dry_run": true, "would_send": body,
                             "endpoint": "POST /v5/order/cancel",
                             "note": "propose-only — re-call with confirm=true."}));
        }
        let r = self.request("POST", "/v5/order/cancel", &[], Some(&body), true)?;
        Ok(json!({"ok": true, "dry_run": false, "result": r}))
    }

    /// Last traded price as f64 — the number the arb scanner compares.
    pub fn last_price(&self, category: &str, symbol: &str) -> Result<f64, String> {
        let t = self.tickers(category, symbol)?;
        t.get("result").and_then(|r| r.get("list")).and_then(|l| l.as_array())
            .and_then(|a| a.first())
            .and_then(|o| o.get("lastPrice").or_else(|| o.get("markPrice")))
            .and_then(|p| p.as_str().and_then(|s| s.parse::<f64>().ok()))
            .ok_or_else(|| format!("no lastPrice for {symbol} on bybit {category}"))
    }
}

fn cat(c: &str) -> String {
    let c = c.trim().to_lowercase();
    if c.is_empty() { "linear".into() } else { c }
}

fn title_case(s: &str) -> String {
    let s = s.trim().to_lowercase();
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Read credentials from env, then the plain per-field files, then the older two-line
/// `api_key=…` / `api_secret=…` file that already existed on this box. Supporting the older shape
/// costs six lines and avoids a confusing "no credentials" error next to a file that plainly has them.
fn load_credentials() -> Result<(String, String), String> {
    let dir = std::env::var("FLUX_BYBIT_DIR").unwrap_or_else(|_| "/root/.config/bybit".into());
    let env_or_file = |env: &str, file: &str| -> Option<String> {
        std::env::var(env).ok()
            .or_else(|| std::fs::read_to_string(format!("{dir}/{file}")).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    if let (Some(k), Some(s)) = (env_or_file("FLUX_BYBIT_API_KEY", "api_key"),
                                 env_or_file("FLUX_BYBIT_SECRET_KEY", "secret_key")) {
        return Ok((k, s));
    }
    if let Ok(txt) = std::fs::read_to_string(format!("{dir}/credentials")) {
        let (mut k, mut s) = (String::new(), String::new());
        for line in txt.lines() {
            if let Some((a, b)) = line.split_once('=').or_else(|| line.split_once(':')) {
                match a.trim() {
                    "api_key" => k = b.trim().trim_matches('"').to_string(),
                    "api_secret" | "secret_key" => s = b.trim().trim_matches('"').to_string(),
                    _ => {}
                }
            }
        }
        if !k.is_empty() && !s.is_empty() { return Ok((k, s)); }
    }
    Err("no Bybit credentials (set FLUX_BYBIT_API_KEY / FLUX_BYBIT_SECRET_KEY, or place \
         api_key + secret_key files in the bybit config directory)".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 4231 test vectors — if the hand-rolled HMAC drifts, these fail loudly rather than
    // producing a plausible hex string that Bybit silently rejects as a bad signature.
    #[test]
    fn hmac_matches_rfc4231_case_1() {
        let key = [0x0bu8; 20];
        assert_eq!(hmac_sha256_hex(&key, b"Hi There"),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
    }

    #[test]
    fn hmac_matches_rfc4231_case_2() {
        assert_eq!(hmac_sha256_hex(b"Jefe", b"what do ya want for nothing?"),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    }

    #[test]
    fn hmac_matches_rfc4231_case_3_long_message() {
        let key = [0xaau8; 20];
        let data = [0xddu8; 50];
        assert_eq!(hmac_sha256_hex(&key, &data),
            "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe");
    }

    #[test]
    fn hmac_hashes_a_key_longer_than_the_block() {
        // RFC 4231 case 6: 131-byte key must be pre-hashed, not truncated
        let key = [0xaau8; 131];
        assert_eq!(hmac_sha256_hex(&key, b"Test Using Larger Than Block-Size Key - Hash Key First"),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54");
    }

    #[test]
    fn sign_payload_is_the_documented_concatenation_order() {
        // straight from Bybit's own GET example
        assert_eq!(
            sign_payload("1658384314791", "XXXXXXXXXX", "5000",
                         "category=option&symbol=BTC-29JUL22-25000-C"),
            "1658384314791XXXXXXXXXX5000category=option&symbol=BTC-29JUL22-25000-C");
    }

    #[test]
    fn sign_payload_changes_with_every_component() {
        let base = sign_payload("1", "k", "5000", "q");
        for other in [sign_payload("2", "k", "5000", "q"), sign_payload("1", "K", "5000", "q"),
                      sign_payload("1", "k", "6000", "q"), sign_payload("1", "k", "5000", "Q")] {
            assert_ne!(base, other);
        }
    }

    #[test]
    fn query_string_preserves_order_and_drops_empties() {
        let p = vec![("category".to_string(), "linear".to_string()),
                     ("symbol".to_string(), "BTCUSDT".to_string()),
                     ("blank".to_string(), "".to_string())];
        // order is NOT sorted — Bybit signs exactly the string that goes on the wire
        assert_eq!(query_string(&p), "category=linear&symbol=BTCUSDT");
    }

    #[test]
    fn ret_code_zero_is_success_everything_else_is_an_error() {
        assert!(parse_error(&json!({"retCode": 0, "retMsg": "OK"})).is_none());
        let e = parse_error(&json!({"retCode": 10002, "retMsg": "invalid request"})).unwrap();
        assert!(e.contains("10002") && e.contains("clock"), "10002 must hint at clock drift: {e}");
    }

    #[test]
    fn order_side_and_type_are_title_cased_because_bybit_rejects_uppercase() {
        let o = Bybit::build_order("linear", "btcusdt", "BUY", "MARKET", "0.01", None, false, None, None);
        assert_eq!(o["side"], "Buy");
        assert_eq!(o["orderType"], "Market");
        assert_eq!(o["symbol"], "BTCUSDT");
        assert!(o.get("price").is_none());
        assert!(o.get("timeInForce").is_none(), "a market order carries no time-in-force");
    }

    #[test]
    fn limit_order_carries_price_and_tif() {
        let o = Bybit::build_order("linear", "BTCUSDT", "sell", "limit", "0.5", Some("70000"),
                                   true, Some("60000"), Some("75000"));
        assert_eq!(o["side"], "Sell");
        assert_eq!(o["orderType"], "Limit");
        assert_eq!(o["price"], "70000");
        assert_eq!(o["timeInForce"], "GTC");
        assert_eq!(o["reduceOnly"], true);
        assert_eq!(o["takeProfit"], "60000");
        assert_eq!(o["stopLoss"], "75000");
    }

    #[test]
    fn category_defaults_to_linear_but_respects_an_explicit_one() {
        assert_eq!(cat(""), "linear");
        assert_eq!(cat(" SPOT "), "spot");
    }
}
