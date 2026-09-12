//! flux-coinbase — Flux-native **Coinbase Advanced Trade** engine. Built the flux way: the
//! authenticated surface is declared as a **flux-api** `ApiEndpoint` spec (OpenAPI/SDKs for free),
//! a typed blocking-reqwest client executes it, the `flux_coinbase_*` MCP combos drive it
//! agentically, `flux-coinbase tui` renders it, and every write goes through the Verified
//! Execution Gate and an append-only ledger.
//!
//! # Auth — CDP API keys sign a JWT per request (NOT HMAC)
//! Coinbase Developer Platform keys come as a JSON file `{ "name": "organizations/…/apiKeys/…",
//! "privateKey": "-----BEGIN EC PRIVATE KEY-----…" }` (ES256 / P-256) or, for the newer keys,
//! `{ "id": "…", "privateKey": "<base64 64 bytes>" }` (Ed25519 / EdDSA). Each request carries
//! `Authorization: Bearer <jwt>` where
//!
//! ```text
//!   header  = { alg: ES256|EdDSA, kid: <key name>, nonce: <16 random bytes hex>, typ: JWT }
//!   payload = { sub: <key name>, iss: "cdp", nbf: now, exp: now+120,
//!               uri: "<METHOD> api.coinbase.com<path-without-query>" }
//! ```
//!
//! The JWT is valid for two minutes and bound to one method+path, so a leaked token buys an
//! attacker almost nothing — but the private key buys everything. It lives in
//! `/root/.config/coinbase/cdp_api_key.json` (chmod 600) or `FLUX_COINBASE_KEY_FILE`, is never
//! logged, never echoed into a tool result, and `key_fingerprint()` is the only thing that leaves.
//!
//! # Safety: this crate touches real money
//! `place_order` / `cancel` are propose-only unless `confirm == true` AND the [`Gate`] passes
//! (product whitelist · notional cap · leverage cap · limit-price deviation bound). Perpetual
//! futures (`*-PERP-INTX`, leverage) go through the same gate with a tighter default cap.

pub mod dca;
pub mod ta;
pub mod tui;

use base64::Engine;
use flux_api::schema::{ApiEndpoint, ApiParameter, ApiResponse, ApiSchema, HttpMethod, ParamLocation};
use serde_json::{json, Map, Value};
use std::time::{SystemTime, UNIX_EPOCH};

/// Advanced Trade REST base + the host that goes into the JWT `uri` claim.
pub const BASE: &str = "https://api.coinbase.com";
pub const HOST: &str = "api.coinbase.com";
pub const PREFIX: &str = "/api/v3/brokerage";
/// Default key file (CDP JSON download, chmod 600).
pub const KEY_FILE: &str = "/root/.config/coinbase/cdp_api_key.json";
/// Append-only record of everything that touched money.
pub const LEDGER: &str = "/home/storage/claude-code/flux-coinbase/ledger.jsonl";

pub fn now_s() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn b64url(b: &[u8]) -> String { base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b) }

pub fn make_nonce() -> String {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

// ─────────────────────────────── credentials + JWT (pure, tested) ───────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind { Es256, Ed25519 }

/// A loaded CDP key. The secret never implements Display/Debug output.
#[derive(Clone)]
pub struct Credentials {
    pub name: String,
    pub kind: KeyKind,
    secret: Vec<u8>, // PEM bytes (ES256) or 32-byte seed (Ed25519)
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Credentials({}, {:?}, <secret>)", self.fingerprint(), self.kind)
    }
}

impl Credentials {
    /// Parse the CDP key JSON. Accepts both the legacy `{name, privateKey(PEM)}` and the newer
    /// `{id, privateKey(base64 ed25519)}` layouts, plus `{keyName, keySecret}` (some SDK configs).
    pub fn parse(text: &str) -> Result<Self, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("key file is not JSON: {e}"))?;
        let name = ["name", "id", "keyName", "api_key_name", "key_name"].iter()
            .find_map(|k| v.get(*k).and_then(|x| x.as_str())).map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or("key file has no `name`/`id` field")?;
        let pk = ["privateKey", "keySecret", "private_key", "secret"].iter()
            .find_map(|k| v.get(*k).and_then(|x| x.as_str())).map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or("key file has no `privateKey` field")?;
        Self::from_parts(&name, &pk)
    }

    pub fn from_parts(name: &str, private_key: &str) -> Result<Self, String> {
        let pk = private_key.replace("\\n", "\n");
        if pk.contains("-----BEGIN") {
            // validate now so a bad PEM fails at load, not at the first signed request
            parse_p256(&pk)?;
            Ok(Credentials { name: name.into(), kind: KeyKind::Es256, secret: pk.into_bytes() })
        } else {
            let raw = base64::engine::general_purpose::STANDARD.decode(pk.trim())
                .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(pk.trim()))
                .map_err(|e| format!("Ed25519 privateKey is not base64: {e}"))?;
            if raw.len() != 64 && raw.len() != 32 {
                return Err(format!("Ed25519 privateKey must decode to 32 or 64 bytes, got {}", raw.len()));
            }
            Ok(Credentials { name: name.into(), kind: KeyKind::Ed25519, secret: raw[..32].to_vec() })
        }
    }

    /// Env first (`FLUX_COINBASE_KEY_NAME` + `FLUX_COINBASE_PRIVATE_KEY`), then the file named by
    /// `FLUX_COINBASE_KEY_FILE`, then the default root-only file.
    pub fn load() -> Result<Self, String> {
        if let (Ok(n), Ok(k)) = (std::env::var("FLUX_COINBASE_KEY_NAME"), std::env::var("FLUX_COINBASE_PRIVATE_KEY")) {
            if !n.trim().is_empty() && !k.trim().is_empty() { return Self::from_parts(n.trim(), k.trim()); }
        }
        let path = key_file_path();
        let text = std::fs::read_to_string(&path)
            .map_err(|_| format!("no Coinbase key at {path} — run `flux-coinbase setup` (or set FLUX_COINBASE_KEY_FILE)"))?;
        Self::parse(&text)
    }

    /// `organizations/abcd…/apiKeys/…wxyz (ES256)` — enough to know WHICH key, never the key.
    pub fn fingerprint(&self) -> String {
        let n = &self.name;
        let short = if n.len() > 20 { format!("{}…{}", &n[..10], &n[n.len() - 6..]) } else { n.clone() };
        format!("{short} ({})", self.alg())
    }

    pub fn alg(&self) -> &'static str { match self.kind { KeyKind::Es256 => "ES256", KeyKind::Ed25519 => "EdDSA" } }

    /// Build the per-request JWT. `path` must be the path WITHOUT query string.
    pub fn jwt(&self, method: &str, path: &str) -> Result<String, String> {
        let now = now_s();
        self.jwt_at(method, path, now, &make_nonce())
    }

    /// Deterministic variant (fixed time + nonce) so the encoding can be unit-tested byte-for-byte.
    pub fn jwt_at(&self, method: &str, path: &str, now: u64, nonce: &str) -> Result<String, String> {
        let header = json!({"alg": self.alg(), "kid": self.name, "nonce": nonce, "typ": "JWT"});
        let payload = json!({"sub": self.name, "iss": "cdp", "nbf": now, "exp": now + 120,
                             "uri": jwt_uri(method, path)});
        let signing_input = format!("{}.{}", b64url(header.to_string().as_bytes()),
                                              b64url(payload.to_string().as_bytes()));
        let sig = self.sign(signing_input.as_bytes())?;
        Ok(format!("{signing_input}.{}", b64url(&sig)))
    }

    fn sign(&self, msg: &[u8]) -> Result<Vec<u8>, String> {
        match self.kind {
            KeyKind::Es256 => {
                use p256::ecdsa::signature::Signer;
                let sk = parse_p256(std::str::from_utf8(&self.secret).map_err(|e| e.to_string())?)?;
                let sig: p256::ecdsa::Signature = sk.sign(msg);
                Ok(sig.to_bytes().to_vec()) // raw r‖s, 64 bytes — what JWS ES256 wants
            }
            KeyKind::Ed25519 => {
                use ed25519_dalek::Signer;
                let mut seed = [0u8; 32];
                seed.copy_from_slice(&self.secret[..32]);
                let sk = ed25519_dalek::SigningKey::from_bytes(&seed);
                Ok(sk.sign(msg).to_bytes().to_vec())
            }
        }
    }
}

fn parse_p256(pem: &str) -> Result<p256::ecdsa::SigningKey, String> {
    use p256::pkcs8::DecodePrivateKey;
    let sk = if pem.contains("EC PRIVATE KEY") {
        p256::SecretKey::from_sec1_pem(pem).map_err(|e| format!("bad SEC1 EC key: {e}"))?
    } else {
        p256::SecretKey::from_pkcs8_pem(pem).map_err(|e| format!("bad PKCS#8 key: {e}"))?
    };
    Ok(p256::ecdsa::SigningKey::from(sk))
}

/// `GET api.coinbase.com/api/v3/brokerage/accounts` — the claim Coinbase checks against the
/// request it actually receives. Query strings are NOT part of it.
pub fn jwt_uri(method: &str, path: &str) -> String {
    let p = path.split('?').next().unwrap_or(path);
    format!("{} {}{}", method.to_uppercase(), HOST, p)
}

pub fn key_file_path() -> String {
    std::env::var("FLUX_COINBASE_KEY_FILE").ok().filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| KEY_FILE.to_string())
}

/// Install a key file: validate, write 0600, return the fingerprint. Never prints the key.
pub fn install_key(text: &str) -> Result<String, String> {
    let c = Credentials::parse(text)?;
    let path = key_file_path();
    if let Some(dir) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, text).map_err(|e| format!("write {path}: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(c.fingerprint())
}

// ─────────────────────────────── numbers: increments + sizing (pure) ───────────────────────────────

/// Number of decimals implied by an increment string (`"0.00000001"` → 8, `"1"` → 0, `"0.01"` → 2).
pub fn decimals_of(increment: &str) -> usize {
    let s = increment.trim();
    match s.split_once('.') {
        Some((_, frac)) => frac.trim_end_matches('0').len(),
        None => 0,
    }
}

/// Round DOWN to the increment and format with exactly that many decimals — the string the API
/// wants. Rounding down is the conservative direction for a buy size.
pub fn round_down(x: f64, increment: &str) -> String {
    let d = decimals_of(increment);
    let m = 10f64.powi(d as i32);
    let v = (x * m + 1e-9).floor() / m;
    format!("{:.*}", d, v.max(0.0))
}

/// `$50 at $100,000` → `0.0005` BTC, floored to the base increment.
pub fn base_from_quote(usd: f64, price: f64, base_increment: &str) -> Result<String, String> {
    if !(price > 0.0) || !price.is_finite() { return Err(format!("price must be positive, got {price}")); }
    if !(usd > 0.0) || !usd.is_finite() { return Err(format!("notional must be positive, got {usd}")); }
    let q = round_down(usd / price, base_increment);
    if q.parse::<f64>().unwrap_or(0.0) <= 0.0 {
        return Err(format!("${usd} at {price} is below one base increment ({base_increment})"));
    }
    Ok(q)
}

pub fn is_perp(product_id: &str) -> bool { product_id.to_uppercase().contains("-PERP") }

/// Human granularity → Coinbase enum + seconds per candle.
pub fn granularity(g: &str) -> Result<(&'static str, u64), String> {
    Ok(match g.trim().to_lowercase().as_str() {
        "1m" | "one_minute" => ("ONE_MINUTE", 60),
        "5m" | "five_minute" => ("FIVE_MINUTE", 300),
        "15m" | "fifteen_minute" => ("FIFTEEN_MINUTE", 900),
        "30m" | "thirty_minute" => ("THIRTY_MINUTE", 1800),
        "1h" | "one_hour" | "" => ("ONE_HOUR", 3600),
        "2h" | "two_hour" => ("TWO_HOUR", 7200),
        "6h" | "six_hour" => ("SIX_HOUR", 21600),
        "1d" | "one_day" | "d" => ("ONE_DAY", 86400),
        other => return Err(format!("unknown granularity {other:?} (1m 5m 15m 30m 1h 2h 6h 1d)")),
    })
}

// ─────────────────────────────────── the Verified Execution Gate ───────────────────────────────────

/// Every check an order must clear before it may become real. Same discipline as
/// `flux-bitunix::Gate` — the layer that makes it safe for a language model to hold an API key.
#[derive(Debug, Clone)]
pub struct Gate {
    /// Only these products may trade. Empty rejects everything (fail closed).
    pub whitelist: Vec<String>,
    /// Hard cap on notional per order in USD. For leverage this is the POSITION notional
    /// (margin × leverage), not the margin — leverage does not shrink the number the gate sees.
    pub max_notional_usd: f64,
    /// Hard cap on leverage. Coinbase perps offer up to 10× in most jurisdictions.
    pub max_leverage: u32,
    /// Reject a LIMIT price further than this from the best bid/ask mid, in basis points.
    pub max_price_deviation_bps: u32,
}

impl Default for Gate {
    /// Deliberately tight. Widening this is an operator decision, taken once, in the open.
    fn default() -> Self {
        Gate {
            whitelist: ["BTC-USD", "ETH-USD", "BTC-USDC", "ETH-USDC", "BTC-PERP-INTX", "ETH-PERP-INTX"]
                .iter().map(|s| s.to_string()).collect(),
            max_notional_usd: 100.0,
            max_leverage: 3,
            max_price_deviation_bps: 500,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict { Pass, Reject(String) }

impl Gate {
    pub fn check(&self, product_id: &str, side: &str, notional_usd: f64, leverage: u32,
                 price: Option<f64>, mark: Option<f64>) -> Verdict {
        let pid = product_id.to_uppercase();
        if pid.is_empty() { return Verdict::Reject("empty product_id".into()); }
        if !matches!(side.to_uppercase().as_str(), "BUY" | "SELL") {
            return Verdict::Reject(format!("side must be BUY or SELL, got {side:?}"));
        }
        if self.whitelist.is_empty() {
            return Verdict::Reject("gate whitelist is empty — fails closed by design".into());
        }
        if !self.whitelist.iter().any(|w| w.eq_ignore_ascii_case(&pid)) {
            return Verdict::Reject(format!("{pid} is not on the gate whitelist {:?}", self.whitelist));
        }
        if !(notional_usd > 0.0) || !notional_usd.is_finite() {
            return Verdict::Reject(format!("notional must be a positive finite number, got {notional_usd}"));
        }
        if notional_usd > self.max_notional_usd {
            return Verdict::Reject(format!("notional ${notional_usd:.2} exceeds the gate cap ${:.2}", self.max_notional_usd));
        }
        if leverage == 0 { return Verdict::Reject("leverage must be at least 1".into()); }
        if leverage > 1 && !is_perp(&pid) {
            return Verdict::Reject(format!("{pid} is a spot product — leverage {leverage}x needs a *-PERP-INTX product"));
        }
        if leverage > self.max_leverage {
            return Verdict::Reject(format!("leverage {leverage}x exceeds the gate cap {}x", self.max_leverage));
        }
        if let (Some(p), Some(m)) = (price, mark) {
            if m > 0.0 && p > 0.0 {
                let dev_bps = ((p - m).abs() / m * 10_000.0).round() as u32;
                if dev_bps > self.max_price_deviation_bps {
                    return Verdict::Reject(format!("limit price {p} is {dev_bps} bps from mark {m}, gate allows {}",
                                                   self.max_price_deviation_bps));
                }
            }
        }
        Verdict::Pass
    }

    pub fn to_json(&self) -> Value {
        json!({"whitelist": self.whitelist, "max_notional_usd": self.max_notional_usd,
               "max_leverage": self.max_leverage, "max_price_deviation_bps": self.max_price_deviation_bps})
    }
}

// ─────────────────────────────────────── typed market data ───────────────────────────────────────

fn f64_of(v: &Value) -> f64 {
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).unwrap_or(0.0)
}
fn f64_at(v: &Value, k: &str) -> f64 { v.get(k).map(f64_of).unwrap_or(0.0) }
fn str_at(v: &Value, k: &str) -> String { v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string() }

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Level { pub price: f64, pub size: f64 }

#[derive(Debug, Clone, Default)]
pub struct Book { pub product_id: String, pub bids: Vec<Level>, pub asks: Vec<Level>, pub time: String }

impl Book {
    pub fn parse(v: &Value) -> Book {
        let pb = v.get("pricebook").unwrap_or(v);
        let lv = |k: &str| pb.get(k).and_then(|a| a.as_array()).map(|a| a.iter()
            .map(|l| Level { price: f64_at(l, "price"), size: f64_at(l, "size") }).collect()).unwrap_or_default();
        Book { product_id: str_at(pb, "product_id"), bids: lv("bids"), asks: lv("asks"), time: str_at(pb, "time") }
    }
    pub fn best_bid(&self) -> Option<f64> { self.bids.first().map(|l| l.price) }
    pub fn best_ask(&self) -> Option<f64> { self.asks.first().map(|l| l.price) }
    pub fn mid(&self) -> Option<f64> { Some((self.best_bid()? + self.best_ask()?) / 2.0) }
    pub fn spread_bps(&self) -> Option<f64> {
        let (b, a) = (self.best_bid()?, self.best_ask()?);
        if b <= 0.0 { return None; }
        Some((a - b) / ((a + b) / 2.0) * 10_000.0)
    }
    /// Bid minus ask liquidity in the top-N levels, normalised to [-1, 1]. >0 = buyers stacked.
    pub fn imbalance(&self, n: usize) -> f64 {
        let b: f64 = self.bids.iter().take(n).map(|l| l.size).sum();
        let a: f64 = self.asks.iter().take(n).map(|l| l.size).sum();
        if b + a <= 0.0 { 0.0 } else { (b - a) / (b + a) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Candle { pub start: u64, pub open: f64, pub high: f64, pub low: f64, pub close: f64, pub volume: f64 }

/// Coinbase returns candles NEWEST first; this returns them oldest → newest.
pub fn parse_candles(v: &Value) -> Vec<Candle> {
    let mut out: Vec<Candle> = v.get("candles").and_then(|a| a.as_array()).map(|a| a.iter().map(|c| Candle {
        start: c.get("start").map(f64_of).unwrap_or(0.0) as u64,
        open: f64_at(c, "open"), high: f64_at(c, "high"), low: f64_at(c, "low"),
        close: f64_at(c, "close"), volume: f64_at(c, "volume"),
    }).collect()).unwrap_or_default();
    out.sort_by_key(|c| c.start);
    out
}

#[derive(Debug, Clone, Default)]
pub struct Trade { pub trade_id: String, pub price: f64, pub size: f64, pub side: String, pub time: String }

#[derive(Debug, Clone, Default)]
pub struct Tape { pub trades: Vec<Trade>, pub best_bid: f64, pub best_ask: f64 }

pub fn parse_tape(v: &Value) -> Tape {
    Tape {
        trades: v.get("trades").and_then(|a| a.as_array()).map(|a| a.iter().map(|t| Trade {
            trade_id: str_at(t, "trade_id"), price: f64_at(t, "price"), size: f64_at(t, "size"),
            side: str_at(t, "side"), time: str_at(t, "time") }).collect()).unwrap_or_default(),
        best_bid: f64_at(v, "best_bid"), best_ask: f64_at(v, "best_ask"),
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProductInfo {
    pub product_id: String, pub base: String, pub quote: String,
    pub price: f64, pub change_24h_pct: f64, pub volume_24h: f64,
    pub base_increment: String, pub quote_increment: String,
    pub base_min_size: String, pub quote_min_size: String,
    pub product_type: String, pub status: String,
    pub perp: bool, pub max_leverage: u32, pub funding_rate: f64, pub open_interest: f64,
}

impl ProductInfo {
    pub fn parse(p: &Value) -> ProductInfo {
        let fpd = p.get("future_product_details");
        let pd = fpd.and_then(|f| f.get("perpetual_details"));
        let perp = fpd.and_then(|f| f.get("contract_expiry_type")).and_then(|x| x.as_str()) == Some("PERPETUAL")
            || is_perp(&str_at(p, "product_id"));
        ProductInfo {
            product_id: str_at(p, "product_id"),
            base: str_at(p, "base_currency_id"), quote: str_at(p, "quote_currency_id"),
            price: f64_at(p, "price"), change_24h_pct: f64_at(p, "price_percentage_change_24h"),
            volume_24h: f64_at(p, "volume_24h"),
            base_increment: { let s = str_at(p, "base_increment"); if s.is_empty() { "0.00000001".into() } else { s } },
            quote_increment: { let s = str_at(p, "quote_increment"); if s.is_empty() { "0.01".into() } else { s } },
            base_min_size: str_at(p, "base_min_size"), quote_min_size: str_at(p, "quote_min_size"),
            product_type: str_at(p, "product_type"), status: str_at(p, "status"),
            perp,
            max_leverage: pd.map(|d| f64_at(d, "max_leverage") as u32).unwrap_or(0),
            funding_rate: pd.map(|d| f64_at(d, "funding_rate")).unwrap_or(0.0),
            open_interest: pd.map(|d| f64_at(d, "open_interest")).unwrap_or(0.0),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Balance { pub currency: String, pub available: f64, pub hold: f64, pub name: String, pub kind: String }

pub fn parse_accounts(v: &Value) -> Vec<Balance> {
    v.get("accounts").and_then(|a| a.as_array()).map(|a| a.iter().map(|x| Balance {
        currency: str_at(x, "currency"),
        available: x.get("available_balance").map(|b| f64_at(b, "value")).unwrap_or(0.0),
        hold: x.get("hold").map(|b| f64_at(b, "value")).unwrap_or(0.0),
        name: str_at(x, "name"), kind: str_at(x, "type"),
    }).collect()).unwrap_or_default()
}

#[derive(Debug, Clone, Default)]
pub struct Position {
    pub product_id: String, pub side: String, pub size: f64, pub entry: f64, pub mark: f64,
    pub liq: f64, pub leverage: f64, pub unrealized_pnl: f64, pub margin: f64,
}

pub fn parse_positions(v: &Value) -> Vec<Position> {
    v.get("positions").and_then(|a| a.as_array()).map(|a| a.iter().map(|p| Position {
        product_id: str_at(p, "product_id"), side: str_at(p, "position_side"),
        size: f64_at(p, "net_size"), entry: f64_at(p, "entry_vwap"), mark: f64_at(p, "mark_price"),
        liq: f64_at(p, "liquidation_price"), leverage: f64_at(p, "leverage"),
        unrealized_pnl: p.get("unrealized_pnl").map(|x| f64_at(x, "value")).unwrap_or_else(|| f64_at(p, "unrealized_pnl")),
        margin: p.get("im_notional").map(|x| f64_at(x, "value")).unwrap_or(0.0),
    }).collect()).unwrap_or_default()
}

#[derive(Debug, Clone, Default)]
pub struct OpenOrder {
    pub order_id: String, pub product_id: String, pub side: String, pub kind: String,
    pub price: f64, pub size: f64, pub filled: f64, pub status: String, pub created: String,
}

pub fn parse_orders(v: &Value) -> Vec<OpenOrder> {
    v.get("orders").and_then(|a| a.as_array()).map(|a| a.iter().map(|o| {
        let cfg = o.get("order_configuration").cloned().unwrap_or(Value::Null);
        let (kind, price, size) = cfg.as_object().and_then(|m| m.iter().next()).map(|(k, c)| (
            k.clone(), f64_at(c, "limit_price").max(f64_at(c, "stop_price")),
            f64_at(c, "base_size").max(f64_at(c, "quote_size")))).unwrap_or_default();
        OpenOrder {
            order_id: str_at(o, "order_id"), product_id: str_at(o, "product_id"), side: str_at(o, "side"),
            kind, price: if price > 0.0 { price } else { f64_at(o, "average_filled_price") }, size,
            filled: f64_at(o, "filled_size"), status: str_at(o, "status"), created: str_at(o, "created_time"),
        }
    }).collect()).unwrap_or_default()
}

// ───────────────────────────────────────────── orders ─────────────────────────────────────────────

/// What kind of order — mirrors Coinbase's `order_configuration` variants we support.
#[derive(Debug, Clone, PartialEq)]
pub enum OrderKind {
    /// Market IOC sized in quote currency (spot only: "spend $50").
    MarketQuote { quote_size: String },
    /// Market IOC sized in base currency (perps, or spot sells).
    MarketBase { base_size: String },
    /// Limit GTC.
    Limit { base_size: String, limit_price: String, post_only: bool },
}

/// Build the exact order body that would be sent — byte for byte what "confirm" sends. Perps
/// carry `leverage` + `margin_type`; spot must not (Coinbase rejects a leverage field on spot).
pub fn build_order(product_id: &str, side: &str, kind: &OrderKind, leverage: Option<u32>,
                   margin_type: Option<&str>, client_order_id: Option<&str>) -> Value {
    let mut m = Map::new();
    m.insert("client_order_id".into(), json!(client_order_id.map(|s| s.to_string())
        .unwrap_or_else(|| format!("flux-{}-{}", now_ms(), &make_nonce()[..8]))));
    m.insert("product_id".into(), json!(product_id.to_uppercase()));
    m.insert("side".into(), json!(side.to_uppercase()));
    let cfg = match kind {
        OrderKind::MarketQuote { quote_size } => json!({"market_market_ioc": {"quote_size": quote_size}}),
        OrderKind::MarketBase { base_size } => json!({"market_market_ioc": {"base_size": base_size}}),
        OrderKind::Limit { base_size, limit_price, post_only } =>
            json!({"limit_limit_gtc": {"base_size": base_size, "limit_price": limit_price, "post_only": post_only}}),
    };
    m.insert("order_configuration".into(), cfg);
    if is_perp(product_id) {
        if let Some(l) = leverage { if l > 1 { m.insert("leverage".into(), json!(l.to_string())); } }
        m.insert("margin_type".into(), json!(margin_type.unwrap_or("CROSS").to_uppercase()));
    }
    Value::Object(m)
}

/// Strip the `client_order_id` — the preview endpoint does not take one.
pub fn preview_body(order: &Value) -> Value {
    let mut o = order.clone();
    if let Some(m) = o.as_object_mut() { m.remove("client_order_id"); }
    o
}

/// Pull the human reason out of a Coinbase error envelope. HTTP-level errors look like
/// `{error, message, error_details}`; order-level failures are HTTP 200 with `success:false`.
pub fn parse_error(status: u16, body: &Value) -> Option<String> {
    if status >= 400 {
        let e = body.get("error").and_then(|x| x.as_str()).unwrap_or("");
        let msg = body.get("message").and_then(|x| x.as_str()).unwrap_or("");
        let det = body.get("error_details").and_then(|x| x.as_str()).unwrap_or("");
        return Some(format!("HTTP {status}: {e} {msg} {det}").trim().to_string());
    }
    if body.get("success") == Some(&json!(false)) {
        let er = body.get("error_response").cloned().unwrap_or(Value::Null);
        let e = er.get("error").and_then(|x| x.as_str()).unwrap_or("");
        let msg = er.get("message").and_then(|x| x.as_str()).unwrap_or("");
        let pf = er.get("preview_failure_reason").and_then(|x| x.as_str()).unwrap_or("");
        return Some(format!("order rejected: {e} {msg} {pf}").trim().to_string());
    }
    None
}

pub fn is_retryable_status(status: u16) -> bool { matches!(status, 408 | 425 | 429 | 500 | 502 | 503 | 504) }
pub fn backoff_ms(attempt: u32, base_ms: u64, cap_ms: u64) -> u64 {
    base_ms.checked_shl(attempt.min(20)).unwrap_or(cap_ms).min(cap_ms)
}

/// Append-only ledger of everything that touched money. Never overwritten, never rotated by us.
pub fn record(v: &Value) {
    use std::io::Write;
    let p = std::env::var("FLUX_COINBASE_LEDGER").unwrap_or_else(|_| LEDGER.to_string());
    if let Some(dir) = std::path::Path::new(&p).parent() { let _ = std::fs::create_dir_all(dir); }
    if let Ok(mut fh) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let mut row = v.clone();
        row["ts"] = json!(now_s());
        let _ = writeln!(fh, "{}", serde_json::to_string(&row).unwrap_or_default());
    }
}
pub fn ledger_path() -> String { std::env::var("FLUX_COINBASE_LEDGER").unwrap_or_else(|_| LEDGER.to_string()) }

// ───────────────────────────────────────────── the client ─────────────────────────────────────────

pub struct Coinbase {
    creds: Option<Credentials>,
    base: String,
    http: reqwest::blocking::Client,
}

impl Coinbase {
    fn http() -> Result<reqwest::blocking::Client, String> {
        reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .user_agent("flux-coinbase/1.0")
            .build().map_err(|e| e.to_string())
    }
    fn base() -> String { std::env::var("FLUX_COINBASE_BASE").unwrap_or_else(|_| BASE.to_string()) }

    /// Signed client. Errs loudly if no key is present — a silently unauthenticated client would
    /// just return UNAUTHENTICATED forever.
    pub fn from_env() -> Result<Self, String> {
        Ok(Coinbase { creds: Some(Credentials::load()?), base: Self::base(), http: Self::http()? })
    }
    /// Public (unsigned) client — market data only.
    pub fn public() -> Result<Self, String> {
        Ok(Coinbase { creds: None, base: Self::base(), http: Self::http()? })
    }
    /// Signed if a key exists, public otherwise. What a UI wants: never refuse to show prices.
    pub fn best_effort() -> Result<Self, String> {
        match Credentials::load() {
            Ok(c) => Ok(Coinbase { creds: Some(c), base: Self::base(), http: Self::http()? }),
            Err(_) => Self::public(),
        }
    }
    pub fn is_signed(&self) -> bool { self.creds.is_some() }
    pub fn key_fingerprint(&self) -> String {
        self.creds.as_ref().map(|c| c.fingerprint()).unwrap_or_else(|| "<no key>".into())
    }

    /// One request with bounded retry on transient failures only. Never retries a 4xx — retrying
    /// a rejected order is how you place it twice.
    fn request(&self, method: &str, path: &str, query: &[(String, String)], body: Option<&Value>,
               signed: bool) -> Result<Value, String> {
        if signed && self.creds.is_none() {
            return Err("this endpoint needs credentials — run `flux-coinbase setup`".into());
        }
        let qs: Vec<(String, String)> = query.iter().filter(|(_, v)| !v.is_empty()).cloned().collect();
        let url = format!("{}{}", self.base, path);
        let body_str = body.map(|b| serde_json::to_string(b).unwrap_or_default());
        let mut last = String::new();
        for attempt in 0..3u32 {
            let mut req = match method {
                "GET" => self.http.get(&url).query(&qs),
                "DELETE" => self.http.delete(&url).query(&qs),
                _ => self.http.post(&url).query(&qs).body(body_str.clone().unwrap_or_default()),
            };
            req = req.header("Content-Type", "application/json").header("Accept", "application/json");
            if let Some(c) = &self.creds {
                if signed || std::env::var("FLUX_COINBASE_SIGN_ALL").is_ok() {
                    req = req.bearer_auth(c.jwt(method, path)?);
                }
            }
            match req.send() {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let text = resp.text().unwrap_or_default();
                    if is_retryable_status(status) && attempt < 2 {
                        last = format!("HTTP {status}");
                        std::thread::sleep(std::time::Duration::from_millis(backoff_ms(attempt, 300, 4000)));
                        continue;
                    }
                    let v: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({"raw": text.chars().take(400).collect::<String>()}));
                    if let Some(e) = parse_error(status, &v) { return Err(e); }
                    return Ok(v);
                }
                Err(e) => {
                    last = e.to_string();
                    if attempt < 2 {
                        std::thread::sleep(std::time::Duration::from_millis(backoff_ms(attempt, 300, 4000)));
                        continue;
                    }
                }
            }
        }
        Err(format!("coinbase request failed after 3 attempts: {last}"))
    }

    fn p(path: &str) -> String { format!("{PREFIX}{path}") }

    // ── public market data (unsigned) ────────────────────────────────────────────────────────
    pub fn server_time(&self) -> Result<Value, String> { self.request("GET", &Self::p("/time"), &[], None, false) }

    /// `perps=true` filters to perpetual futures; else spot. Returns the raw envelope.
    pub fn products_raw(&self, perps: bool) -> Result<Value, String> {
        let q = if perps { vec![("product_type".to_string(), "FUTURE".to_string()),
                                ("contract_expiry_type".to_string(), "PERPETUAL".to_string())] }
                else { vec![("product_type".to_string(), "SPOT".to_string())] };
        self.request("GET", &Self::p("/market/products"), &q, None, false)
    }
    pub fn products(&self, perps: bool) -> Result<Vec<ProductInfo>, String> {
        let v = self.products_raw(perps)?;
        Ok(v.get("products").and_then(|a| a.as_array()).map(|a| a.iter().map(ProductInfo::parse).collect()).unwrap_or_default())
    }
    pub fn product(&self, product_id: &str) -> Result<ProductInfo, String> {
        let v = self.request("GET", &Self::p(&format!("/market/products/{}", product_id.to_uppercase())), &[], None, false)?;
        Ok(ProductInfo::parse(&v))
    }
    pub fn book_raw(&self, product_id: &str, limit: u32) -> Result<Value, String> {
        self.request("GET", &Self::p("/market/product_book"),
                     &[("product_id".into(), product_id.to_uppercase()), ("limit".into(), limit.clamp(1, 250).to_string())], None, false)
    }
    pub fn book(&self, product_id: &str, limit: u32) -> Result<Book, String> { Ok(Book::parse(&self.book_raw(product_id, limit)?)) }

    pub fn candles_raw(&self, product_id: &str, gran: &str, count: u32) -> Result<Value, String> {
        let (g, secs) = granularity(gran)?;
        let n = count.clamp(1, 350) as u64;
        let end = now_s();
        let start = end.saturating_sub(n * secs);
        self.request("GET", &Self::p(&format!("/market/products/{}/candles", product_id.to_uppercase())),
                     &[("start".into(), start.to_string()), ("end".into(), end.to_string()), ("granularity".into(), g.into())], None, false)
    }
    pub fn candles(&self, product_id: &str, gran: &str, count: u32) -> Result<Vec<Candle>, String> {
        Ok(parse_candles(&self.candles_raw(product_id, gran, count)?))
    }
    pub fn trades_raw(&self, product_id: &str, limit: u32) -> Result<Value, String> {
        self.request("GET", &Self::p(&format!("/market/products/{}/ticker", product_id.to_uppercase())),
                     &[("limit".into(), limit.clamp(1, 100).to_string())], None, false)
    }
    pub fn tape(&self, product_id: &str, limit: u32) -> Result<Tape, String> { Ok(parse_tape(&self.trades_raw(product_id, limit)?)) }

    /// Mid of best bid/ask, falling back to the product's last price. The number the gate uses.
    pub fn mark(&self, product_id: &str) -> Result<f64, String> {
        if let Ok(b) = self.book(product_id, 1) { if let Some(m) = b.mid() { if m > 0.0 { return Ok(m); } } }
        let p = self.product(product_id)?;
        if p.price > 0.0 { Ok(p.price) } else { Err(format!("no price for {product_id}")) }
    }

    // ── account (signed, read-only) ──────────────────────────────────────────────────────────
    /// `{can_view, can_trade, can_transfer, portfolio_uuid, portfolio_type}` — the setup check.
    pub fn key_permissions(&self) -> Result<Value, String> { self.request("GET", &Self::p("/key_permissions"), &[], None, true) }
    pub fn accounts_raw(&self) -> Result<Value, String> {
        self.request("GET", &Self::p("/accounts"), &[("limit".into(), "250".into())], None, true)
    }
    pub fn accounts(&self) -> Result<Vec<Balance>, String> { Ok(parse_accounts(&self.accounts_raw()?)) }
    pub fn portfolios(&self) -> Result<Value, String> { self.request("GET", &Self::p("/portfolios"), &[], None, true) }
    pub fn portfolio_breakdown(&self, uuid: &str) -> Result<Value, String> {
        self.request("GET", &Self::p(&format!("/portfolios/{uuid}")), &[], None, true)
    }
    /// The INTX (perpetuals) portfolio uuid, if the account has one. `None` = perps not enabled.
    pub fn intx_portfolio_uuid(&self) -> Result<Option<String>, String> {
        let v = self.portfolios()?;
        Ok(v.get("portfolios").and_then(|a| a.as_array()).and_then(|a| a.iter()
            .find(|p| p.get("type").and_then(|t| t.as_str()) == Some("INTX"))
            .and_then(|p| p.get("uuid").and_then(|u| u.as_str())).map(|s| s.to_string())))
    }
    pub fn intx_portfolio(&self, uuid: &str) -> Result<Value, String> {
        self.request("GET", &Self::p(&format!("/intx/portfolio/{uuid}")), &[], None, true)
    }
    pub fn intx_positions_raw(&self, uuid: &str) -> Result<Value, String> {
        self.request("GET", &Self::p(&format!("/intx/positions/{uuid}")), &[], None, true)
    }
    /// All perp positions, or an empty list (and a reason) when the account has no INTX portfolio.
    pub fn positions(&self) -> Result<(Vec<Position>, Option<String>), String> {
        match self.intx_portfolio_uuid()? {
            Some(u) => Ok((parse_positions(&self.intx_positions_raw(&u)?), None)),
            None => Ok((vec![], Some("no INTX (perpetuals) portfolio on this account — leverage is not enabled here".into()))),
        }
    }
    pub fn open_orders_raw(&self, product_id: &str, limit: u32) -> Result<Value, String> {
        let mut q = vec![("order_status".to_string(), "OPEN".to_string()), ("limit".to_string(), limit.clamp(1, 100).to_string())];
        if !product_id.is_empty() { q.push(("product_id".into(), product_id.to_uppercase())); }
        self.request("GET", &Self::p("/orders/historical/batch"), &q, None, true)
    }
    pub fn open_orders(&self, product_id: &str) -> Result<Vec<OpenOrder>, String> {
        Ok(parse_orders(&self.open_orders_raw(product_id, 100)?))
    }
    pub fn order(&self, order_id: &str) -> Result<Value, String> {
        self.request("GET", &Self::p(&format!("/orders/historical/{order_id}")), &[], None, true)
    }
    pub fn fills(&self, product_id: &str, limit: u32) -> Result<Value, String> {
        let mut q = vec![("limit".to_string(), limit.clamp(1, 100).to_string())];
        if !product_id.is_empty() { q.push(("product_id".into(), product_id.to_uppercase())); }
        self.request("GET", &Self::p("/orders/historical/fills"), &q, None, true)
    }

    // ── trading (signed, WRITE — gated) ──────────────────────────────────────────────────────
    /// Ask Coinbase what the order would cost — fees, fill estimate, warnings — without placing it.
    /// This is the number a human sees before saying yes.
    pub fn preview(&self, order: &Value) -> Result<Value, String> {
        self.request("POST", &Self::p("/orders/preview"), &[], Some(&preview_body(order)), true)
    }
    /// Place an order. **Requires `confirm == true`**; anything else returns the proposal and
    /// sends nothing. The gate must already have passed — callers run it so the decision is
    /// visible in the proposal rather than hidden in here.
    pub fn place_order(&self, order: &Value, confirm: bool) -> Result<Value, String> {
        if !confirm {
            return Ok(json!({"ok": true, "dry_run": true, "would_send": order,
                             "endpoint": "POST /api/v3/brokerage/orders",
                             "note": "propose-only — nothing was sent. Re-call with confirm=true to place this order."}));
        }
        let r = self.request("POST", &Self::p("/orders"), &[], Some(order), true)?;
        let row = json!({"kind": "order", "sent": order, "result": r});
        record(&row);
        Ok(json!({"ok": true, "dry_run": false, "sent": order, "result": r}))
    }
    pub fn cancel(&self, order_ids: &[String], confirm: bool) -> Result<Value, String> {
        let body = json!({"order_ids": order_ids});
        if !confirm {
            return Ok(json!({"ok": true, "dry_run": true, "would_send": body,
                             "endpoint": "POST /api/v3/brokerage/orders/batch_cancel",
                             "note": "propose-only — re-call with confirm=true to cancel."}));
        }
        let r = self.request("POST", &Self::p("/orders/batch_cancel"), &[], Some(&body), true)?;
        record(&json!({"kind": "cancel", "sent": body, "result": r}));
        Ok(json!({"ok": true, "dry_run": false, "result": r}))
    }
    /// Cancel everything open on a product (or everything, when product is empty).
    pub fn cancel_all(&self, product_id: &str, confirm: bool) -> Result<Value, String> {
        let ids: Vec<String> = self.open_orders(product_id)?.into_iter().map(|o| o.order_id).collect();
        if ids.is_empty() { return Ok(json!({"ok": true, "cancelled": 0, "note": "nothing open"})); }
        self.cancel(&ids, confirm)
    }

    /// Everything a trading panel needs in one call. Signed parts degrade to `{error}` when the
    /// key is missing — a UI must never refuse to show the market because the wallet is absent.
    pub fn panel(&self, product_id: &str) -> Value {
        let pid = product_id.to_uppercase();
        let product = self.product(&pid).map(|p| product_json(&p)).unwrap_or_else(|e| json!({"error": e}));
        let book = self.book(&pid, 10).map(|b| json!({
            "best_bid": b.best_bid(), "best_ask": b.best_ask(), "mid": b.mid(), "spread_bps": b.spread_bps(),
            "imbalance_top10": b.imbalance(10),
            "bids": b.bids.iter().map(|l| json!([l.price, l.size])).collect::<Vec<_>>(),
            "asks": b.asks.iter().map(|l| json!([l.price, l.size])).collect::<Vec<_>>(),
        })).unwrap_or_else(|e| json!({"error": e}));
        let (accounts, positions, orders) = if self.is_signed() {
            (self.accounts().map(|a| a.iter().filter(|b| b.available > 0.0 || b.hold > 0.0)
                    .map(|b| json!({"currency": b.currency, "available": b.available, "hold": b.hold})).collect::<Vec<_>>())
                .map(Value::Array).unwrap_or_else(|e| json!({"error": e})),
             self.positions().map(|(p, why)| json!({"positions": p.iter().map(position_json).collect::<Vec<_>>(), "note": why}))
                .unwrap_or_else(|e| json!({"error": e})),
             self.open_orders("").map(|o| o.iter().map(order_json).collect::<Vec<_>>()).map(Value::Array)
                .unwrap_or_else(|e| json!({"error": e})))
        } else {
            let e = json!({"error": "no key — run `flux-coinbase setup`"});
            (e.clone(), e.clone(), e)
        };
        json!({"ok": true, "venue": "coinbase-advanced-trade", "base": self.base, "key": self.key_fingerprint(),
               "product": product, "book": book, "accounts": accounts, "positions": positions,
               "open_orders": orders, "gate": Gate::default().to_json(),
               "note": "reads only. Placing an order needs flux_coinbase_order with confirm=true and a passing gate."})
    }
}

pub fn product_json(p: &ProductInfo) -> Value {
    json!({"product_id": p.product_id, "price": p.price, "change_24h_pct": p.change_24h_pct, "volume_24h": p.volume_24h,
           "base_increment": p.base_increment, "quote_increment": p.quote_increment, "base_min_size": p.base_min_size,
           "quote_min_size": p.quote_min_size, "type": p.product_type, "status": p.status, "perp": p.perp,
           "max_leverage": p.max_leverage, "funding_rate": p.funding_rate, "open_interest": p.open_interest})
}
pub fn position_json(p: &Position) -> Value {
    json!({"product_id": p.product_id, "side": p.side, "size": p.size, "entry": p.entry, "mark": p.mark,
           "liquidation": p.liq, "leverage": p.leverage, "unrealized_pnl": p.unrealized_pnl, "margin": p.margin})
}
pub fn order_json(o: &OpenOrder) -> Value {
    json!({"order_id": o.order_id, "product_id": o.product_id, "side": o.side, "type": o.kind, "price": o.price,
           "size": o.size, "filled": o.filled, "status": o.status, "created": o.created})
}

// ───────────────────────────────────── flux-api endpoint spec ─────────────────────────────────────

fn qp(name: &str, required: bool, desc: &str) -> ApiParameter {
    ApiParameter { name: name.into(), location: ParamLocation::Query, required,
                   schema: ApiSchema::string(), description: desc.into() }
}

fn ep(method: HttpMethod, path: &str, op: &str, summary: &str, mut params: Vec<ApiParameter>, tag: &str) -> ApiEndpoint {
    // every SIGNED call carries the per-request CDP JWT; public market calls do not
    if !summary.contains("(public)") {
        params.insert(0, ApiParameter { name: "Authorization".into(), location: ParamLocation::Header, required: true,
            schema: ApiSchema::string(), description: "Bearer <CDP JWT: ES256|EdDSA, iss=cdp, uri=METHOD host+path, exp=120s>".into() });
    }
    ApiEndpoint {
        crate_name: "flux-coinbase".into(), method, path: format!("{PREFIX}{path}"),
        operation_id: op.into(), summary: summary.into(), parameters: params, request_body: None,
        responses: vec![ApiResponse { status: 200, description: "Coinbase Advanced Trade JSON".into(),
                                      schema: Some(ApiSchema::object().build()) }],
        tags: vec![tag.into()], middleware: None,
    }
}

/// The declarative Coinbase surface — flux-api turns this into OpenAPI 3.1, the SDKs and the docsite.
pub fn coinbase_spec() -> Vec<ApiEndpoint> {
    use HttpMethod::{GET, POST};
    vec![
        ep(GET, "/time", "serverTime", "Server time (public)", vec![], "market"),
        ep(GET, "/market/products", "products", "Products (public)",
           vec![qp("product_type", false, "SPOT | FUTURE"), qp("contract_expiry_type", false, "PERPETUAL for perps")], "market"),
        ep(GET, "/market/products/{product_id}", "product", "One product (public)", vec![], "market"),
        ep(GET, "/market/product_book", "book", "Order book (public)",
           vec![qp("product_id", true, "e.g. BTC-USD"), qp("limit", false, "levels per side, max 250")], "market"),
        ep(GET, "/market/products/{product_id}/candles", "candles", "Candles (public)",
           vec![qp("start", true, "unix seconds"), qp("end", true, "unix seconds"), qp("granularity", true, "ONE_MINUTE … ONE_DAY")], "market"),
        ep(GET, "/market/products/{product_id}/ticker", "trades", "Recent trades (public)", vec![qp("limit", false, "max 100")], "market"),
        ep(GET, "/key_permissions", "keyPermissions", "What this key may do (signed)", vec![], "account"),
        ep(GET, "/accounts", "accounts", "Balances (signed)", vec![qp("limit", false, "max 250")], "account"),
        ep(GET, "/portfolios", "portfolios", "Portfolios DEFAULT/CONSUMER/INTX (signed)", vec![], "account"),
        ep(GET, "/intx/portfolio/{portfolio_uuid}", "intxPortfolio", "Perps portfolio summary (signed)", vec![], "perps"),
        ep(GET, "/intx/positions/{portfolio_uuid}", "intxPositions", "Perps positions (signed)", vec![], "perps"),
        ep(GET, "/orders/historical/batch", "orders", "Orders (signed)",
           vec![qp("order_status", false, "OPEN | FILLED | CANCELLED"), qp("product_id", false, ""), qp("limit", false, "max 100")], "trade"),
        ep(GET, "/orders/historical/fills", "fills", "Fills (signed)", vec![qp("product_id", false, ""), qp("limit", false, "max 100")], "trade"),
        ep(POST, "/orders/preview", "previewOrder", "Preview an order — fees, fill estimate (signed)", vec![], "trade"),
        ep(POST, "/orders", "placeOrder", "PLACE an order (signed, GATED, confirm required)", vec![], "trade"),
        ep(POST, "/orders/batch_cancel", "cancelOrders", "Cancel orders (signed, GATED, confirm required)", vec![], "trade"),
    ]
}

// ─────────────────────────────────────────────── tests ───────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ec_key() -> Credentials {
        // fresh P-256 key generated in-test; no real credential is ever embedded here
        use p256::pkcs8::EncodePrivateKey;
        let sk = p256::SecretKey::random(&mut rand::thread_rng());
        let pem = sk.to_pkcs8_pem(p256::pkcs8::LineEnding::LF).unwrap().to_string();
        Credentials::from_parts("organizations/1234567890/apiKeys/abcdef-uvwxyz", &pem).unwrap()
    }

    #[test]
    fn jwt_uri_drops_query_and_uppercases_method() {
        assert_eq!(jwt_uri("get", "/api/v3/brokerage/accounts?limit=250"), "GET api.coinbase.com/api/v3/brokerage/accounts");
    }

    #[test]
    fn es256_jwt_has_three_parts_and_correct_claims() {
        let c = ec_key();
        let t = c.jwt_at("GET", "/api/v3/brokerage/accounts", 1_700_000_000, "00ff").unwrap();
        let parts: Vec<&str> = t.split('.').collect();
        assert_eq!(parts.len(), 3);
        let dec = |s: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s).unwrap();
        let h: Value = serde_json::from_slice(&dec(parts[0])).unwrap();
        let p: Value = serde_json::from_slice(&dec(parts[1])).unwrap();
        assert_eq!(h["alg"], "ES256"); assert_eq!(h["kid"], c.name); assert_eq!(h["nonce"], "00ff"); assert_eq!(h["typ"], "JWT");
        assert_eq!(p["iss"], "cdp"); assert_eq!(p["sub"], c.name);
        assert_eq!(p["nbf"], 1_700_000_000u64); assert_eq!(p["exp"], 1_700_000_120u64);
        assert_eq!(p["uri"], "GET api.coinbase.com/api/v3/brokerage/accounts");
        assert_eq!(dec(parts[2]).len(), 64, "ES256 JWS signature is raw r‖s, 64 bytes");
    }

    #[test]
    fn es256_signature_verifies_against_public_key() {
        use p256::ecdsa::signature::Verifier;
        let c = ec_key();
        let t = c.jwt_at("POST", "/api/v3/brokerage/orders", 1_700_000_000, "ab").unwrap();
        let (input, sig) = t.rsplit_once('.').unwrap();
        let sk = parse_p256(std::str::from_utf8(&c.secret).unwrap()).unwrap();
        let vk = sk.verifying_key();
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(sig).unwrap();
        let s = p256::ecdsa::Signature::from_slice(&raw).unwrap();
        assert!(vk.verify(input.as_bytes(), &s).is_ok());
    }

    #[test]
    fn ed25519_key_parses_and_signs_64_bytes() {
        let seed = [7u8; 32];
        let sk = ed25519_dalek::SigningKey::from_bytes(&seed);
        let mut full = seed.to_vec(); full.extend_from_slice(sk.verifying_key().as_bytes());
        let b64 = base64::engine::general_purpose::STANDARD.encode(&full);
        let text = format!(r#"{{"id":"11111111-2222-3333-4444-555555555555","privateKey":"{b64}"}}"#);
        let c = Credentials::parse(&text).unwrap();
        assert_eq!(c.kind, KeyKind::Ed25519);
        let t = c.jwt_at("GET", "/api/v3/brokerage/accounts", 1, "n").unwrap();
        let sig = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(t.rsplit('.').next().unwrap()).unwrap();
        assert_eq!(sig.len(), 64);
        let h: Value = serde_json::from_slice(&base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(t.split('.').next().unwrap()).unwrap()).unwrap();
        assert_eq!(h["alg"], "EdDSA");
    }

    #[test]
    fn key_file_with_escaped_newlines_parses() {
        use p256::pkcs8::EncodePrivateKey;
        let sk = p256::SecretKey::random(&mut rand::thread_rng());
        let pem = sk.to_pkcs8_pem(p256::pkcs8::LineEnding::LF).unwrap().to_string();
        let v = json!({"name": "organizations/x/apiKeys/y", "privateKey": pem});
        assert!(Credentials::parse(&v.to_string()).is_ok());
        assert!(Credentials::parse(r#"{"name":"x"}"#).is_err());
        assert!(Credentials::parse("nope").is_err());
    }

    #[test]
    fn fingerprint_never_contains_secret() {
        let c = ec_key();
        let fp = c.fingerprint();
        assert!(!fp.contains("PRIVATE"));
        assert!(fp.contains("ES256"));
        assert!(!format!("{c:?}").contains("BEGIN"));
    }

    #[test]
    fn increments_and_rounding() {
        assert_eq!(decimals_of("0.00000001"), 8);
        assert_eq!(decimals_of("0.01"), 2);
        assert_eq!(decimals_of("1"), 0);
        assert_eq!(decimals_of("0.00100000"), 3);
        assert_eq!(round_down(0.123456789, "0.00000001"), "0.12345678");
        assert_eq!(round_down(123.456, "0.01"), "123.45");
        assert_eq!(round_down(5.9, "1"), "5");
        assert_eq!(base_from_quote(50.0, 100_000.0, "0.00000001").unwrap(), "0.00050000");
        assert!(base_from_quote(0.0001, 100_000.0, "0.00000001").is_err());
        assert!(base_from_quote(50.0, 0.0, "0.00000001").is_err());
    }

    #[test]
    fn gate_defaults_are_tight_and_fail_closed() {
        let g = Gate::default();
        assert_eq!(g.check("BTC-USD", "BUY", 50.0, 1, None, None), Verdict::Pass);
        assert!(matches!(g.check("DOGE-USD", "BUY", 50.0, 1, None, None), Verdict::Reject(_)));
        assert!(matches!(g.check("BTC-USD", "BUY", 500.0, 1, None, None), Verdict::Reject(_)));
        assert!(matches!(g.check("BTC-USD", "BUY", 50.0, 2, None, None), Verdict::Reject(_)), "leverage on spot");
        assert_eq!(g.check("BTC-PERP-INTX", "BUY", 50.0, 3, None, None), Verdict::Pass);
        assert!(matches!(g.check("BTC-PERP-INTX", "BUY", 50.0, 4, None, None), Verdict::Reject(_)));
        assert!(matches!(g.check("BTC-USD", "HOLD", 50.0, 1, None, None), Verdict::Reject(_)));
        assert!(matches!(g.check("BTC-USD", "BUY", 50.0, 1, Some(90_000.0), Some(100_000.0)), Verdict::Reject(_)));
        assert_eq!(g.check("BTC-USD", "BUY", 50.0, 1, Some(99_800.0), Some(100_000.0)), Verdict::Pass);
        let empty = Gate { whitelist: vec![], ..Gate::default() };
        assert!(matches!(empty.check("BTC-USD", "BUY", 1.0, 1, None, None), Verdict::Reject(_)));
    }

    #[test]
    fn order_bodies_spot_vs_perp() {
        let spot = build_order("btc-usd", "buy", &OrderKind::MarketQuote { quote_size: "50.00".into() }, Some(3), None, Some("cid"));
        assert_eq!(spot["product_id"], "BTC-USD"); assert_eq!(spot["side"], "BUY");
        assert_eq!(spot["order_configuration"]["market_market_ioc"]["quote_size"], "50.00");
        assert!(spot.get("leverage").is_none(), "spot must not carry leverage");
        assert!(spot.get("margin_type").is_none());
        let perp = build_order("BTC-PERP-INTX", "BUY", &OrderKind::MarketBase { base_size: "0.001".into() }, Some(3), Some("isolated"), None);
        assert_eq!(perp["leverage"], "3"); assert_eq!(perp["margin_type"], "ISOLATED");
        assert!(perp["client_order_id"].as_str().unwrap().starts_with("flux-"));
        let lim = build_order("ETH-USD", "SELL", &OrderKind::Limit { base_size: "0.1".into(), limit_price: "4000".into(), post_only: true }, None, None, None);
        assert_eq!(lim["order_configuration"]["limit_limit_gtc"]["post_only"], true);
        assert!(preview_body(&lim).get("client_order_id").is_none());
    }

    #[test]
    fn error_envelopes() {
        assert!(parse_error(401, &json!({"error": "UNAUTHENTICATED", "message": "bad jwt"})).unwrap().contains("UNAUTHENTICATED"));
        assert!(parse_error(200, &json!({"success": false, "error_response": {"error": "INSUFFICIENT_FUND", "message": "no"}})).unwrap().contains("INSUFFICIENT_FUND"));
        assert!(parse_error(200, &json!({"success": true})).is_none());
        assert!(parse_error(200, &json!({"accounts": []})).is_none());
    }

    #[test]
    fn book_and_candles_parse() {
        let b = Book::parse(&json!({"pricebook": {"product_id": "BTC-USD",
            "bids": [{"price": "100", "size": "2"}, {"price": "99", "size": "1"}],
            "asks": [{"price": "101", "size": "1"}, {"price": "102", "size": "5"}], "time": "t"}}));
        assert_eq!(b.best_bid(), Some(100.0)); assert_eq!(b.best_ask(), Some(101.0)); assert_eq!(b.mid(), Some(100.5));
        assert!((b.spread_bps().unwrap() - 99.5).abs() < 0.1);
        assert!((b.imbalance(1) - (1.0 / 3.0)).abs() < 1e-9);
        let c = parse_candles(&json!({"candles": [
            {"start": "200", "open": "1", "high": "2", "low": "0.5", "close": "1.5", "volume": "9"},
            {"start": "100", "open": "1", "high": "2", "low": "0.5", "close": "1.2", "volume": "9"}]}));
        assert_eq!(c[0].start, 100); assert_eq!(c[1].close, 1.5);
    }

    #[test]
    fn granularity_map() {
        assert_eq!(granularity("1h").unwrap(), ("ONE_HOUR", 3600));
        assert_eq!(granularity("1d").unwrap(), ("ONE_DAY", 86400));
        assert!(granularity("3h").is_err());
    }

    #[test]
    fn spec_has_writes_marked_authenticated() {
        let s = coinbase_spec();
        let auth = |e: &ApiEndpoint| e.parameters.iter().any(|p| p.name == "Authorization");
        assert!(s.iter().any(|e| e.path.ends_with("/orders") && auth(e)));
        assert!(s.iter().any(|e| e.path.ends_with("/market/product_book") && !auth(e)));
    }
}
