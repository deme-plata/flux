//! Dependency-light HTTP client for a `sigil-rpcd` money daemon.
//!
//! No tokio, no reqwest, no TLS stack — just `std::net::TcpStream`, the same
//! transport `sigil-rpcd` itself speaks. That keeps an agentic-money agent a
//! single small static binary you can drop on any box in the fleet.
//!
//! `sigil-rpcd` routes (the money surface these templates target):
//!   GET  /health
//!   GET  /status
//!   GET  /balance?wallet=<hex>&token=<hex>
//!   GET  /pools                      → { "pools": [ { reserve_a, reserve_b, .. } ] }
//!   GET  /tokens   /wallets   /economy
//!   POST /swap            { from, pool, dir, amount_in, min_out }
//!   POST /add_liquidity   { from, pool, amount_a, amount_b }
//!   POST /onboard         { }        → { wallet, seed, .. }
//!
//! Everything returns JSON. We return the raw body and parse with serde_json at
//! the call site so a fork can add fields without touching this client.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// A connection target like `127.0.0.1:8099`. One `Rpc` is cheap; it opens a
/// fresh `Connection: close` socket per request (sigil-rpcd is request/close).
#[derive(Clone)]
pub struct Rpc {
    /// `host:port`, with any `http://` prefix / trailing slash stripped.
    addr: String,
    timeout: Duration,
}

impl Rpc {
    /// `Rpc::new("http://127.0.0.1:8099")` or `Rpc::new("127.0.0.1:8099")`.
    pub fn new(base: &str) -> Self {
        let addr = base
            .trim_start_matches("http://")
            .trim_start_matches("https://")
            .trim_end_matches('/')
            .to_string();
        Self { addr, timeout: Duration::from_secs(20) }
    }

    pub fn with_timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }

    fn request(&self, method: &str, path: &str, body: Option<&str>) -> std::io::Result<String> {
        let mut stream = TcpStream::connect(&self.addr)?;
        stream.set_read_timeout(Some(self.timeout))?;
        stream.set_write_timeout(Some(self.timeout))?;
        let body = body.unwrap_or("");
        let req = format!(
            "{method} {path} HTTP/1.1\r\n\
             Host: {host}\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {len}\r\n\
             Connection: close\r\n\r\n\
             {body}",
            host = self.addr,
            len = body.len(),
        );
        stream.write_all(req.as_bytes())?;
        let mut raw = String::new();
        stream.read_to_string(&mut raw)?;
        // strip the status line + headers; return just the body
        Ok(raw.splitn(2, "\r\n\r\n").nth(1).unwrap_or("").to_string())
    }

    /// GET `path` (include the query string), returns the raw JSON body.
    pub fn get(&self, path: &str) -> std::io::Result<String> {
        self.request("GET", path, None)
    }

    /// POST `json` to `path`, returns the raw JSON body.
    pub fn post(&self, path: &str, json: &str) -> std::io::Result<String> {
        self.request("POST", path, Some(json))
    }

    // ── typed convenience helpers over the common money routes ──

    /// Wallet balance for a token (returns 0 on any parse/transport miss —
    /// callers that need to distinguish "zero" from "unreachable" should use
    /// [`Rpc::get`] directly).
    pub fn balance(&self, wallet_hex: &str, token_hex: &str) -> u128 {
        let path = format!("/balance?wallet={wallet_hex}&token={token_hex}");
        self.get(&path)
            .ok()
            .and_then(|b| serde_json::from_str::<serde_json::Value>(&b).ok())
            .and_then(|v| v.get("balance").and_then(parse_u128))
            .unwrap_or(0)
    }

    /// Reserves of pool index `i` as `(reserve_a, reserve_b)`.
    pub fn pool_reserves(&self, i: usize) -> Option<(u128, u128)> {
        let body = self.get("/pools").ok()?;
        let v: serde_json::Value = serde_json::from_str(&body).ok()?;
        let p = v.get("pools")?.get(i)?;
        Some((parse_u128(p.get("reserve_a")?)?, parse_u128(p.get("reserve_b")?)?))
    }

    /// Execute a swap. Returns the raw result JSON (look for `"ok":true`).
    pub fn swap(&self, from: &str, pool: &str, dir: &str, amount_in: u128, min_out: u128) -> std::io::Result<String> {
        let body = format!(
            "{{\"from\":\"{from}\",\"pool\":\"{pool}\",\"dir\":\"{dir}\",\"amount_in\":{amount_in},\"min_out\":{min_out}}}"
        );
        self.post("/swap", &body)
    }
}

/// Tolerant u128 parse — accepts JSON numbers OR strings (SIGIL serializes
/// large balances as strings to survive JSON's 2^53 integer ceiling).
pub fn parse_u128(v: &serde_json::Value) -> Option<u128> {
    if let Some(n) = v.as_u64() {
        return Some(n as u128);
    }
    if let Some(s) = v.as_str() {
        return s.parse::<u128>().ok();
    }
    None
}
