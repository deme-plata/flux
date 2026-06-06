//! flux-0x — Flux-native 0x Protocol engine. Built the flux way: the whole 0x surface (Swap API v2
//! + Cross-Chain API) is declared as a **flux-api** `ApiEndpoint` spec — so flux-api generates the
//! OpenAPI 3.1 + the 6-language SDKs + the docsite for free — and a typed reqwest client executes it.
//! The MCP combos (`flux_0x_*` in fluxc-mcp) drive it agentically.
//!
//! Auth: `0x-api-key` header (read from `FLUX_0X_API_KEY` or `/root/.config/0x/api_key`, never
//! committed) + `0x-version: v2`. Base `https://api.0x.org`, all chains via `chainId` (Swap) or
//! `originChain`/`destinationChain` (Cross-Chain). Endpoint shapes verified live against the API.

use flux_api::schema::{ApiEndpoint, ApiParameter, ApiResponse, ApiSchema, HttpMethod, ParamLocation};
use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const BASE: &str = "https://api.0x.org";

// ─────────────────────────── robustness: pure, unit-tested helpers ───────────────────────────
// The 0x API is famously stable; the only way this engine flakes is OUR client. So every decision
// (retry? how long? is this input even valid?) is a pure function tested without touching the
// network — the live calls just wire these together.

/// HTTP statuses worth retrying — transient only. 4xx (except the rate-limit/timeout ones) are
/// our fault or 0x telling us "no", and must NOT be retried (retrying a 400 is just a slower 400).
pub fn is_retryable_status(status: u16) -> bool {
    matches!(status, 408 | 425 | 429 | 500 | 502 | 503 | 504)
}

/// Exponential backoff with a hard cap (full-jitter is layered on at call time). `attempt` is 0-based.
pub fn backoff_ms(attempt: u32, base_ms: u64, cap_ms: u64) -> u64 {
    let shifted = base_ms.checked_shl(attempt.min(20)).unwrap_or(cap_ms);
    shifted.min(cap_ms)
}

/// Pull a human reason out of a 0x error body ({name,message,data} / {reason} / validationErrors).
pub fn parse_0x_error(body: &Value) -> String {
    if let Some(m) = body.get("message").and_then(|v| v.as_str()) {
        // surface the first validation detail too, if present — that's what actually tells you what's wrong
        if let Some(first) = body.get("data").and_then(|d| d.get("details")).and_then(|x| x.as_array()).and_then(|a| a.first())
            .or_else(|| body.get("validationErrors").and_then(|x| x.as_array()).and_then(|a| a.first()))
        {
            let f = first.get("reason").or_else(|| first.get("message")).and_then(|v| v.as_str()).unwrap_or("");
            if !f.is_empty() { return format!("{m} — {f}"); }
        }
        return m.to_string();
    }
    body.get("reason").and_then(|v| v.as_str()).map(|s| s.to_string())
        .unwrap_or_else(|| "error".to_string())
}

fn validate_chain(chain_id: u64) -> Result<(), String> {
    if chain_id == 0 { Err("chainId must be a non-zero EVM chain id (1=Ethereum, 137=Polygon, 8453=Base, …)".into()) } else { Ok(()) }
}
fn validate_token(name: &str, t: &str) -> Result<(), String> {
    if t.trim().is_empty() { Err(format!("{name} is required (token address)")) } else { Ok(()) }
}
fn validate_amount(name: &str, amt: &str) -> Result<(), String> {
    if amt.is_empty() { return Err(format!("{name} is required")); }
    if !amt.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("{name} must be an integer in base units, e.g. 1 ETH = \"1000000000000000000\" (got '{amt}')"));
    }
    if amt.bytes().all(|b| b == b'0') { return Err(format!("{name} must be > 0")); }
    Ok(())
}

fn qp(name: &str, required: bool, desc: &str) -> ApiParameter {
    ApiParameter { name: name.into(), location: ParamLocation::Query, required, schema: ApiSchema::string(), description: desc.into() }
}
fn hdr(name: &str, desc: &str) -> ApiParameter {
    ApiParameter { name: name.into(), location: ParamLocation::Header, required: true, schema: ApiSchema::string(), description: desc.into() }
}
fn ep(method: HttpMethod, path: &str, op: &str, summary: &str, mut params: Vec<ApiParameter>, tag: &str) -> ApiEndpoint {
    // every 0x call carries the auth + version headers
    params.insert(0, hdr("0x-api-key", "0x API key"));
    params.insert(1, hdr("0x-version", "API version (v2)"));
    ApiEndpoint {
        crate_name: "flux-0x".into(), method, path: path.into(), operation_id: op.into(),
        summary: summary.into(), parameters: params, request_body: None,
        responses: vec![ApiResponse { status: 200, description: "ok".into(), schema: Some(ApiSchema::object().build()) }],
        tags: vec![tag.into()], middleware: None,
    }
}

/// The complete 0x surface as a flux-api spec. Feed to `flux_api::generate_openapi` / the SDK
/// generators for the OpenAPI + TS/Python/Rust/Go/Kotlin/Chrome clients.
pub fn zerox_spec() -> Vec<ApiEndpoint> {
    use HttpMethod::GET;
    let swap_params = || vec![
        qp("chainId", true, "EVM chain id (1=Ethereum, 137=Polygon, 8453=Base, …)"),
        qp("sellToken", true, "token being sold (address or symbol)"),
        qp("buyToken", true, "token being bought"),
        qp("sellAmount", false, "amount to sell, base units (or use buyAmount)"),
        qp("buyAmount", false, "amount to buy, base units (or use sellAmount)"),
        qp("taker", false, "taker address (required for /quote)"),
        qp("slippageBps", false, "max slippage in bps"),
    ];
    let xchain_params = || vec![
        qp("originChain", true, "source chain id"),
        qp("destinationChain", true, "destination chain id"),
        qp("sellToken", true, "token sold on the origin chain"),
        qp("buyToken", true, "token bought on the destination chain"),
        qp("sellAmount", true, "amount to bridge-and-swap, base units"),
        qp("originAddress", true, "taker / sender address on the origin chain"),
        qp("destinationAddress", false, "recipient address on the destination chain — REQUIRED for non-EVM destinations (e.g. Solana)"),
        qp("sortQuotesBy", true, "route preference: price | speed"),
    ];
    vec![
        ep(GET, "/swap/permit2/price", "swapPrice", "Indicative Swap price (Permit2)", swap_params(), "swap"),
        ep(GET, "/swap/permit2/quote", "swapQuote", "Firm Swap quote with calldata (Permit2)", swap_params(), "swap"),
        ep(GET, "/swap/allowance-holder/price", "swapPriceAH", "Indicative Swap price (AllowanceHolder)", swap_params(), "swap"),
        ep(GET, "/swap/allowance-holder/quote", "swapQuoteAH", "Firm Swap quote (AllowanceHolder)", swap_params(), "swap"),
        ep(GET, "/sources", "swapSources", "Liquidity sources for a chain", vec![qp("chainId", true, "chain id")], "swap"),
        ep(GET, "/cross-chain/quotes", "xchainQuotes", "Cross-chain bridge-and-swap quotes", xchain_params(), "cross-chain"),
        ep(GET, "/cross-chain/quotes/stream", "xchainQuotesStream", "Cross-chain quotes as an SSE stream", xchain_params(), "cross-chain"),
        ep(GET, "/cross-chain/status", "xchainStatus", "Track a cross-chain trade", vec![
            qp("originChain", true, "origin chain id"),
            qp("originTxHash", true, "origin transaction hash"),
            qp("quoteId", false, "quote id from /quotes"),
        ], "cross-chain"),
    ]
}

/// Tunables for the resilient transport. Sensible, conservative defaults; overridable via env.
#[derive(Debug, Clone, Copy)]
pub struct RetryConfig {
    pub max_retries: u32,     // transient-error retries (idempotent GETs only)
    pub base_backoff_ms: u64, // first backoff step
    pub cap_backoff_ms: u64,  // backoff ceiling
    pub min_interval_ms: u64, // client-side throttle between requests (proactively dodge 429s)
    pub timeout_s: u64,
}
impl Default for RetryConfig {
    fn default() -> Self {
        RetryConfig { max_retries: 4, base_backoff_ms: 250, cap_backoff_ms: 8_000, min_interval_ms: 0, timeout_s: 30 }
    }
}
impl RetryConfig {
    /// Read overrides from env (FLUX_0X_MAX_RETRIES / _BASE_BACKOFF_MS / _CAP_BACKOFF_MS / _MIN_INTERVAL_MS / _TIMEOUT_S).
    pub fn from_env() -> Self {
        let mut c = RetryConfig::default();
        let g = |k: &str| std::env::var(k).ok().and_then(|s| s.trim().parse::<u64>().ok());
        if let Some(v) = g("FLUX_0X_MAX_RETRIES") { c.max_retries = v as u32; }
        if let Some(v) = g("FLUX_0X_BASE_BACKOFF_MS") { c.base_backoff_ms = v; }
        if let Some(v) = g("FLUX_0X_CAP_BACKOFF_MS") { c.cap_backoff_ms = v; }
        if let Some(v) = g("FLUX_0X_MIN_INTERVAL_MS") { c.min_interval_ms = v; }
        if let Some(v) = g("FLUX_0X_TIMEOUT_S") { c.timeout_s = v.max(1); }
        c
    }
}

/// The typed, resilient 0x client — executes the spec against the live API with retry/backoff,
/// rate-limit awareness, client-side throttling and fail-fast input validation.
pub struct Zerox {
    key: String,
    base: String,
    http: reqwest::blocking::Client,
    cfg: RetryConfig,
    last_call: Mutex<Option<Instant>>,
}

impl Zerox {
    /// Key from `FLUX_0X_API_KEY` env, else `/root/.config/0x/api_key`. Errs if neither is set.
    pub fn from_env() -> Result<Self, String> {
        Self::with_config(RetryConfig::from_env())
    }

    /// Same key resolution, explicit retry/throttle config.
    pub fn with_config(cfg: RetryConfig) -> Result<Self, String> {
        let key = std::env::var("FLUX_0X_API_KEY").ok()
            .or_else(|| std::fs::read_to_string("/root/.config/0x/api_key").ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or("no 0x API key (set FLUX_0X_API_KEY or /root/.config/0x/api_key)")?;
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(cfg.timeout_s))
            .build().map_err(|e| e.to_string())?;
        Ok(Zerox { key, base: BASE.into(), http, cfg, last_call: Mutex::new(None) })
    }

    /// Client-side throttle: never fire requests closer together than `min_interval_ms`.
    fn throttle(&self) {
        let min = self.cfg.min_interval_ms;
        if min == 0 { return; }
        let mut guard = self.last_call.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(prev) = *guard {
            let elapsed = prev.elapsed().as_millis() as u64;
            if elapsed < min { std::thread::sleep(Duration::from_millis(min - elapsed)); }
        }
        *guard = Some(Instant::now());
    }

    /// Resilient GET: retries transient failures (timeouts, connect errors, 429/5xx) with capped
    /// exponential backoff + jitter, honoring `Retry-After`. Non-retryable errors fail fast & loud.
    fn get(&self, path: &str, params: &[(&str, String)]) -> Result<Value, String> {
        let url = format!("{}{}", self.base, path);
        let q: Vec<(&str, &str)> = params.iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| (*k, v.as_str())).collect();
        let mut attempt: u32 = 0;
        loop {
            self.throttle();
            match self.http.get(&url).query(&q)
                .header("0x-api-key", &self.key).header("0x-version", "v2").send()
            {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        return resp.json::<Value>().map_err(|e| format!("0x {path} decode: {e}"));
                    }
                    let code = status.as_u16();
                    let retry_after_ms = resp.headers().get(reqwest::header::RETRY_AFTER)
                        .and_then(|h| h.to_str().ok())
                        .and_then(|s| s.trim().parse::<u64>().ok())
                        .map(|secs| secs.saturating_mul(1000));
                    let body = resp.json::<Value>().unwrap_or(Value::Null);
                    let reason = parse_0x_error(&body);
                    if is_retryable_status(code) && attempt < self.cfg.max_retries {
                        let wait = retry_after_ms.unwrap_or_else(|| Self::jitter(backoff_ms(attempt, self.cfg.base_backoff_ms, self.cfg.cap_backoff_ms)));
                        std::thread::sleep(Duration::from_millis(wait));
                        attempt += 1;
                        continue;
                    }
                    return Err(format!("0x {path} HTTP {code}: {reason}{}",
                        if attempt > 0 { format!(" (after {attempt} retries)") } else { String::new() }));
                }
                Err(e) => {
                    let transient = e.is_timeout() || e.is_connect();
                    if transient && attempt < self.cfg.max_retries {
                        let wait = Self::jitter(backoff_ms(attempt, self.cfg.base_backoff_ms, self.cfg.cap_backoff_ms));
                        std::thread::sleep(Duration::from_millis(wait));
                        attempt += 1;
                        continue;
                    }
                    return Err(format!("0x {path} network error{}: {e}",
                        if attempt > 0 { format!(" (after {attempt} retries)") } else { String::new() }));
                }
            }
        }
    }

    /// Layer mild jitter on a backoff so concurrent agents don't retry in lockstep (thundering herd).
    fn jitter(ms: u64) -> u64 {
        let spread = ms / 4 + 1;
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos() as u64).unwrap_or(0);
        ms + (n % spread)
    }

    /// Liveness probe — confirms key + connectivity by listing sources on Ethereum. For monitoring.
    pub fn health(&self) -> Result<Value, String> {
        let s = self.sources(1)?;
        let n = s.get("sources").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
        Ok(json!({ "ok": true, "base": self.base, "chain": 1, "sources": n,
                   "config": {"max_retries": self.cfg.max_retries, "timeout_s": self.cfg.timeout_s, "min_interval_ms": self.cfg.min_interval_ms} }))
    }

    /// Indicative Swap price (no taker needed).
    pub fn swap_price(&self, chain_id: u64, sell: &str, buy: &str, sell_amount: &str) -> Result<Value, String> {
        validate_chain(chain_id)?; validate_token("sellToken", sell)?; validate_token("buyToken", buy)?; validate_amount("sellAmount", sell_amount)?;
        self.get("/swap/permit2/price", &[("chainId", chain_id.to_string()), ("sellToken", sell.into()), ("buyToken", buy.into()), ("sellAmount", sell_amount.into())])
    }
    /// Firm Swap quote (taker required) — carries the calldata to submit.
    pub fn swap_quote(&self, chain_id: u64, sell: &str, buy: &str, sell_amount: &str, taker: &str) -> Result<Value, String> {
        validate_chain(chain_id)?; validate_token("sellToken", sell)?; validate_token("buyToken", buy)?; validate_amount("sellAmount", sell_amount)?; validate_token("taker", taker)?;
        self.get("/swap/permit2/quote", &[("chainId", chain_id.to_string()), ("sellToken", sell.into()), ("buyToken", buy.into()), ("sellAmount", sell_amount.into()), ("taker", taker.into())])
    }

    /// Indicative Swap price via the **AllowanceHolder** flow — the alternative to Permit2 for
    /// integrations/tokens that prefer a plain ERC-20 approval over signing an EIP-712 permit.
    pub fn swap_price_ah(&self, chain_id: u64, sell: &str, buy: &str, sell_amount: &str) -> Result<Value, String> {
        validate_chain(chain_id)?; validate_token("sellToken", sell)?; validate_token("buyToken", buy)?; validate_amount("sellAmount", sell_amount)?;
        self.get("/swap/allowance-holder/price", &[("chainId", chain_id.to_string()), ("sellToken", sell.into()), ("buyToken", buy.into()), ("sellAmount", sell_amount.into())])
    }
    /// Firm Swap quote via the **AllowanceHolder** flow (taker required) — calldata to submit after a
    /// standard ERC-20 approval to the allowanceTarget (no Permit2 signature needed).
    pub fn swap_quote_ah(&self, chain_id: u64, sell: &str, buy: &str, sell_amount: &str, taker: &str) -> Result<Value, String> {
        validate_chain(chain_id)?; validate_token("sellToken", sell)?; validate_token("buyToken", buy)?; validate_amount("sellAmount", sell_amount)?; validate_token("taker", taker)?;
        self.get("/swap/allowance-holder/quote", &[("chainId", chain_id.to_string()), ("sellToken", sell.into()), ("buyToken", buy.into()), ("sellAmount", sell_amount.into()), ("taker", taker.into())])
    }
    /// Cross-chain bridge-and-swap quotes (origin → destination chain). `dest_address` is the
    /// recipient on the destination chain — leave empty for EVM→EVM (defaults to the taker), but it
    /// is REQUIRED for non-EVM destinations like Solana (the 0x API rejects the quote otherwise).
    pub fn crosschain_quotes(&self, origin_chain: u64, dest_chain: u64, origin_token: &str, dest_token: &str, amount: &str, origin_address: &str, dest_address: &str, sort: &str) -> Result<Value, String> {
        validate_chain(origin_chain)?; validate_chain(dest_chain)?; validate_token("sellToken", origin_token)?; validate_token("buyToken", dest_token)?; validate_amount("sellAmount", amount)?; validate_token("originAddress", origin_address)?;
        self.get("/cross-chain/quotes", &[
            ("originChain", origin_chain.to_string()), ("destinationChain", dest_chain.to_string()),
            ("sellToken", origin_token.into()), ("buyToken", dest_token.into()),
            ("sellAmount", amount.into()), ("originAddress", origin_address.into()),
            ("destinationAddress", dest_address.into()), // omitted by get() when empty
            ("sortQuotesBy", if sort.is_empty() { "price".into() } else { sort.to_string() }),
        ])
    }
    /// Track a cross-chain trade by its origin tx hash.
    pub fn crosschain_status(&self, origin_chain: u64, origin_tx: &str, quote_id: &str) -> Result<Value, String> {
        validate_chain(origin_chain)?; validate_token("originTxHash", origin_tx)?;
        self.get("/cross-chain/status", &[("originChain", origin_chain.to_string()), ("originTxHash", origin_tx.into()), ("quoteId", quote_id.into())])
    }
    /// Liquidity sources for a chain.
    pub fn sources(&self, chain_id: u64) -> Result<Value, String> {
        validate_chain(chain_id)?;
        self.get("/sources", &[("chainId", chain_id.to_string())])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_covers_swap_and_crosschain() {
        let s = zerox_spec();
        assert!(s.len() >= 8, "swap + cross-chain endpoints");
        let paths: Vec<&str> = s.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"/swap/permit2/quote"));
        assert!(paths.contains(&"/cross-chain/quotes"));
        assert!(paths.contains(&"/cross-chain/status"));
        // every endpoint carries the auth + version headers
        assert!(s.iter().all(|e| e.parameters.iter().any(|p| p.name == "0x-api-key")
            && e.parameters.iter().any(|p| p.name == "0x-version")));
        // cross-chain uses origin/destination chain naming (verified live)
        let xq = s.iter().find(|e| e.path == "/cross-chain/quotes").unwrap();
        assert!(xq.parameters.iter().any(|p| p.name == "originChain"));
        assert!(xq.parameters.iter().any(|p| p.name == "destinationChain"));
    }

    #[test]
    fn spec_lowers_to_openapi() {
        // the flux way: flux-api turns our spec into an OpenAPI 3.1 document
        let spec = zerox_spec();
        let oa = flux_api::generate_openapi("0x Flux Engine", "2.0.0", &spec).to_string();
        assert!(oa.contains("/cross-chain/quotes") && oa.contains("/swap/permit2/quote"));
        assert!(oa.contains("openapi"));
    }

    // ───────────── robustness (no network — these are why the engine never flakes) ─────────────

    #[test]
    fn only_transient_statuses_are_retried() {
        // retry the transient ones …
        for s in [408, 425, 429, 500, 502, 503, 504] { assert!(is_retryable_status(s), "{s} should retry"); }
        // … never the ones that are our fault or a definitive "no" (retrying a 400 is just a slower 400)
        for s in [200, 400, 401, 403, 404, 422] { assert!(!is_retryable_status(s), "{s} must NOT retry"); }
    }

    #[test]
    fn backoff_grows_then_caps() {
        let (base, cap) = (250, 8_000);
        assert_eq!(backoff_ms(0, base, cap), 250);
        assert_eq!(backoff_ms(1, base, cap), 500);
        assert_eq!(backoff_ms(2, base, cap), 1_000);
        assert_eq!(backoff_ms(5, base, cap), 8_000); // 250<<5 = 8000, exactly the cap
        assert_eq!(backoff_ms(99, base, cap), cap);  // never exceeds the ceiling, never overflows
        // strictly non-decreasing
        let mut last = 0;
        for a in 0..30 { let b = backoff_ms(a, base, cap); assert!(b >= last); last = b; }
    }

    #[test]
    fn input_is_validated_before_we_ever_hit_the_network() {
        assert!(validate_chain(0).is_err());
        assert!(validate_chain(1).is_ok());
        assert!(validate_token("sellToken", "  ").is_err());
        assert!(validate_token("sellToken", "0xeeee...").is_ok());
        assert!(validate_amount("sellAmount", "").is_err());
        assert!(validate_amount("sellAmount", "0").is_err());          // zero is not a trade
        assert!(validate_amount("sellAmount", "12.5").is_err());       // must be base-units integer
        assert!(validate_amount("sellAmount", "1a2").is_err());
        assert!(validate_amount("sellAmount", "1000000000000000000").is_ok());
    }

    #[test]
    fn error_body_is_turned_into_a_useful_reason() {
        let plain = serde_json::json!({"name": "INPUT_INVALID", "message": "input is invalid"});
        assert_eq!(parse_0x_error(&plain), "input is invalid");
        let detailed = serde_json::json!({
            "message": "Validation failed",
            "data": { "details": [ {"field": "sellAmount", "reason": "must be a positive integer"} ] }
        });
        assert!(parse_0x_error(&detailed).contains("must be a positive integer"));
        // never panics / always yields something even on a shape we don't recognise
        assert!(!parse_0x_error(&serde_json::json!({"weird": true})).is_empty());
    }

    #[test]
    fn retry_config_reads_env_overrides() {
        std::env::set_var("FLUX_0X_MAX_RETRIES", "7");
        std::env::set_var("FLUX_0X_MIN_INTERVAL_MS", "120");
        let c = RetryConfig::from_env();
        assert_eq!(c.max_retries, 7);
        assert_eq!(c.min_interval_ms, 120);
        assert!(c.timeout_s >= 1); // defaulted, sane
        std::env::remove_var("FLUX_0X_MAX_RETRIES");
        std::env::remove_var("FLUX_0X_MIN_INTERVAL_MS");
    }
}
