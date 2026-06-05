//! flux-cmc — Flux-native CoinMarketCap engine: the DECIDE layer of the agentic-trading stack
//! (CMC decide → flux-0x execute → CDP settle). Built the flux way: the surface is a **flux-api**
//! `ApiEndpoint` spec (so flux-api emits OpenAPI 3.1 + SDKs for free), driven by a hardened reqwest
//! client. On top of the raw data it adds a `signal()` — market data boiled down to a decision an
//! agent can act on (bullish/bearish/neutral + why), so an LLM doesn't have to parse raw JSON.
//!
//! Auth: `X-CMC_PRO_API_KEY` header, read from `FLUX_CMC_API_KEY` env or `/root/.config/cmc/api_key`
//! (never committed). Base `https://pro-api.coinmarketcap.com`. CMC rate-limits hard (HTTP 429 +
//! in-body `status.error_code`), so the client retries transient failures and surfaces CMC's own
//! error message — same robustness discipline as flux-0x.

use flux_api::schema::{ApiEndpoint, ApiParameter, ApiResponse, ApiSchema, HttpMethod, ParamLocation};
use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const BASE: &str = "https://pro-api.coinmarketcap.com";

// ─────────────────────────── robustness: pure, unit-tested helpers ───────────────────────────

/// HTTP statuses worth retrying — transient only (CMC uses 429 for rate-limit).
pub fn is_retryable_status(status: u16) -> bool {
    matches!(status, 408 | 425 | 429 | 500 | 502 | 503 | 504)
}

/// Capped exponential backoff (jitter layered on at call time). `attempt` is 0-based.
pub fn backoff_ms(attempt: u32, base_ms: u64, cap_ms: u64) -> u64 {
    base_ms.checked_shl(attempt.min(20)).unwrap_or(cap_ms).min(cap_ms)
}

/// Pull CMC's own error message out of a `{status:{error_code,error_message}}` body.
pub fn parse_cmc_error(body: &Value) -> String {
    body.get("status").and_then(|s| s.get("error_message")).and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "error".to_string())
}

/// The agent-facing momentum verdict — the whole point of the DECIDE layer. Pure & testable.
pub fn momentum_verdict(ch24h: f64, ch7d: f64) -> &'static str {
    if ch24h >= 2.0 && ch7d >= 0.0 { "bullish" }
    else if ch24h <= -2.0 && ch7d <= 0.0 { "bearish" }
    else if ch24h.abs() < 2.0 { "neutral" }
    else { "mixed" } // short-term and longer-term disagree — caution
}

fn validate_symbols(s: &str) -> Result<(), String> {
    if s.trim().is_empty() { Err("symbol is required, e.g. BTC or BTC,ETH,SOL".into()) } else { Ok(()) }
}

fn qp(name: &str, required: bool, desc: &str) -> ApiParameter {
    ApiParameter { name: name.into(), location: ParamLocation::Query, required, schema: ApiSchema::string(), description: desc.into() }
}
fn ep(path: &str, op: &str, summary: &str, mut params: Vec<ApiParameter>) -> ApiEndpoint {
    params.insert(0, ApiParameter { name: "X-CMC_PRO_API_KEY".into(), location: ParamLocation::Header, required: true, schema: ApiSchema::string(), description: "CMC Pro API key".into() });
    ApiEndpoint {
        crate_name: "flux-cmc".into(), method: HttpMethod::GET, path: path.into(), operation_id: op.into(),
        summary: summary.into(), parameters: params, request_body: None,
        responses: vec![ApiResponse { status: 200, description: "ok".into(), schema: Some(ApiSchema::object().build()) }],
        tags: vec!["market".into()], middleware: None,
    }
}

/// The complete CMC surface we use, as a flux-api spec. Feed to `flux_api::generate_openapi`.
pub fn cmc_spec() -> Vec<ApiEndpoint> {
    vec![
        ep("/v1/cryptocurrency/quotes/latest", "quotesLatest", "Latest price + % changes for symbols", vec![
            qp("symbol", true, "comma-separated symbols, e.g. BTC,ETH,SOL"),
            qp("convert", false, "fiat/crypto to price in (default USD)"),
        ]),
        ep("/v1/global-metrics/quotes/latest", "globalMetrics", "Total market cap, BTC dominance, volume", vec![
            qp("convert", false, "default USD"),
        ]),
        ep("/v1/cryptocurrency/listings/latest", "listingsLatest", "Top coins / movers", vec![
            qp("limit", false, "how many (default 20)"),
            qp("sort", false, "e.g. percent_change_24h, market_cap, volume_24h"),
            qp("sort_dir", false, "asc | desc"),
            qp("convert", false, "default USD"),
        ]),
        ep("/v1/key/info", "keyInfo", "API key plan + credit usage (liveness)", vec![]),
    ]
}

/// Retry/throttle config — conservative defaults, env-overridable. Mirrors flux-0x.
#[derive(Debug, Clone, Copy)]
pub struct RetryConfig { pub max_retries: u32, pub base_backoff_ms: u64, pub cap_backoff_ms: u64, pub min_interval_ms: u64, pub timeout_s: u64 }
impl Default for RetryConfig {
    fn default() -> Self { RetryConfig { max_retries: 4, base_backoff_ms: 300, cap_backoff_ms: 8_000, min_interval_ms: 0, timeout_s: 20 } }
}
impl RetryConfig {
    pub fn from_env() -> Self {
        let mut c = RetryConfig::default();
        let g = |k: &str| std::env::var(k).ok().and_then(|s| s.trim().parse::<u64>().ok());
        if let Some(v) = g("FLUX_CMC_MAX_RETRIES") { c.max_retries = v as u32; }
        if let Some(v) = g("FLUX_CMC_MIN_INTERVAL_MS") { c.min_interval_ms = v; }
        if let Some(v) = g("FLUX_CMC_BASE_BACKOFF_MS") { c.base_backoff_ms = v; }
        if let Some(v) = g("FLUX_CMC_CAP_BACKOFF_MS") { c.cap_backoff_ms = v; }
        if let Some(v) = g("FLUX_CMC_TIMEOUT_S") { c.timeout_s = v.max(1); }
        c
    }
}

/// The typed, resilient CoinMarketCap client.
pub struct Cmc {
    key: String,
    base: String,
    http: reqwest::blocking::Client,
    cfg: RetryConfig,
    last_call: Mutex<Option<Instant>>,
}

impl Cmc {
    /// Key from `FLUX_CMC_API_KEY` env, else `/root/.config/cmc/api_key`.
    pub fn from_env() -> Result<Self, String> { Self::with_config(RetryConfig::from_env()) }

    pub fn with_config(cfg: RetryConfig) -> Result<Self, String> {
        let key = std::env::var("FLUX_CMC_API_KEY").ok()
            .or_else(|| std::fs::read_to_string("/root/.config/cmc/api_key").ok())
            .map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
            .ok_or("no CMC API key (set FLUX_CMC_API_KEY or /root/.config/cmc/api_key)")?;
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(cfg.timeout_s)).build().map_err(|e| e.to_string())?;
        Ok(Cmc { key, base: BASE.into(), http, cfg, last_call: Mutex::new(None) })
    }

    fn throttle(&self) {
        let min = self.cfg.min_interval_ms;
        if min == 0 { return; }
        let mut g = self.last_call.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(prev) = *g { let e = prev.elapsed().as_millis() as u64; if e < min { std::thread::sleep(Duration::from_millis(min - e)); } }
        *g = Some(Instant::now());
    }

    fn jitter(ms: u64) -> u64 {
        let spread = ms / 4 + 1;
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos() as u64).unwrap_or(0);
        ms + (n % spread)
    }

    /// Resilient GET: retries transient failures (429/5xx, timeouts) with capped backoff + jitter,
    /// honors `Retry-After`, and surfaces CMC's in-body `status.error_code` even on HTTP 200.
    fn get(&self, path: &str, params: &[(&str, String)]) -> Result<Value, String> {
        let url = format!("{}{}", self.base, path);
        let q: Vec<(&str, &str)> = params.iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| (*k, v.as_str())).collect();
        let mut attempt = 0u32;
        loop {
            self.throttle();
            match self.http.get(&url).query(&q).header("X-CMC_PRO_API_KEY", &self.key).header("Accept", "application/json").send() {
                Ok(resp) => {
                    let status = resp.status();
                    let code = status.as_u16();
                    if !status.is_success() {
                        let retry_after = resp.headers().get(reqwest::header::RETRY_AFTER).and_then(|h| h.to_str().ok()).and_then(|s| s.trim().parse::<u64>().ok()).map(|s| s * 1000);
                        let body = resp.json::<Value>().unwrap_or(Value::Null);
                        if is_retryable_status(code) && attempt < self.cfg.max_retries {
                            std::thread::sleep(Duration::from_millis(retry_after.unwrap_or_else(|| Self::jitter(backoff_ms(attempt, self.cfg.base_backoff_ms, self.cfg.cap_backoff_ms)))));
                            attempt += 1; continue;
                        }
                        return Err(format!("CMC {path} HTTP {code}: {}", parse_cmc_error(&body)));
                    }
                    let v = resp.json::<Value>().map_err(|e| format!("CMC {path} decode: {e}"))?;
                    // CMC can return 200 with an error in status.error_code
                    let ec = v.get("status").and_then(|s| s.get("error_code")).and_then(|c| c.as_i64()).unwrap_or(0);
                    if ec != 0 { return Err(format!("CMC {path} error {ec}: {}", parse_cmc_error(&v))); }
                    return Ok(v);
                }
                Err(e) => {
                    if (e.is_timeout() || e.is_connect()) && attempt < self.cfg.max_retries {
                        std::thread::sleep(Duration::from_millis(Self::jitter(backoff_ms(attempt, self.cfg.base_backoff_ms, self.cfg.cap_backoff_ms))));
                        attempt += 1; continue;
                    }
                    return Err(format!("CMC {path} network error: {e}"));
                }
            }
        }
    }

    /// Latest quote(s) for comma-separated symbols.
    pub fn quote(&self, symbols: &str) -> Result<Value, String> {
        validate_symbols(symbols)?;
        self.get("/v1/cryptocurrency/quotes/latest", &[("symbol", symbols.to_uppercase()), ("convert", "USD".into())])
    }
    /// Global market metrics (total cap, BTC dominance, volume).
    pub fn global(&self) -> Result<Value, String> {
        self.get("/v1/global-metrics/quotes/latest", &[("convert", "USD".into())])
    }
    /// Top movers — listings sorted (default by 24h change desc).
    pub fn movers(&self, limit: u32, sort: &str) -> Result<Value, String> {
        self.get("/v1/cryptocurrency/listings/latest", &[
            ("limit", limit.clamp(1, 100).to_string()),
            ("sort", if sort.is_empty() { "percent_change_24h".into() } else { sort.to_string() }),
            ("sort_dir", "desc".into()), ("convert", "USD".into()),
        ])
    }
    /// API key plan + credit usage — liveness probe.
    pub fn health(&self) -> Result<Value, String> { self.get("/v1/key/info", &[]) }

    /// THE DECIDE OUTPUT: market data boiled to a verdict an agent can act on.
    pub fn signal(&self, symbol: &str) -> Result<Value, String> {
        validate_symbols(symbol)?;
        let sym = symbol.to_uppercase();
        let v = self.quote(&sym)?;
        let q = v.get("data").and_then(|d| d.get(&sym)).and_then(|c| c.get("quote")).and_then(|q| q.get("USD"))
            .ok_or_else(|| format!("no USD quote for {sym}"))?;
        let f = |k: &str| q.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
        let (price, ch1h, ch24h, ch7d) = (f("price"), f("percent_change_1h"), f("percent_change_24h"), f("percent_change_7d"));
        let verdict = momentum_verdict(ch24h, ch7d);
        Ok(json!({
            "symbol": sym, "price_usd": price,
            "change": { "1h": ch1h, "24h": ch24h, "7d": ch7d },
            "verdict": verdict,
            "why": format!("24h {ch24h:+.2}% / 7d {ch7d:+.2}% ⇒ {verdict}"),
            "as_of": q.get("last_updated").cloned().unwrap_or(Value::Null),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_covers_the_decide_surface() {
        let s = cmc_spec();
        let paths: Vec<&str> = s.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"/v1/cryptocurrency/quotes/latest"));
        assert!(paths.contains(&"/v1/global-metrics/quotes/latest"));
        assert!(s.iter().all(|e| e.parameters.iter().any(|p| p.name == "X-CMC_PRO_API_KEY")));
    }

    #[test]
    fn spec_lowers_to_openapi() {
        let oa = flux_api::generate_openapi("CMC Flux Engine", "1.0.0", &cmc_spec()).to_string();
        assert!(oa.contains("/v1/cryptocurrency/quotes/latest") && oa.contains("openapi"));
    }

    #[test]
    fn only_transient_statuses_are_retried() {
        for s in [429, 500, 502, 503, 504] { assert!(is_retryable_status(s)); }
        for s in [200, 400, 401, 403, 404] { assert!(!is_retryable_status(s)); }
    }

    #[test]
    fn backoff_grows_then_caps() {
        assert_eq!(backoff_ms(0, 300, 8000), 300);
        assert_eq!(backoff_ms(1, 300, 8000), 600);
        assert_eq!(backoff_ms(99, 300, 8000), 8000);
    }

    #[test]
    fn verdict_reads_momentum() {
        assert_eq!(momentum_verdict(3.0, 5.0), "bullish");   // up short + up week
        assert_eq!(momentum_verdict(-4.0, -6.0), "bearish"); // down short + down week
        assert_eq!(momentum_verdict(0.5, 10.0), "neutral");  // flat short
        assert_eq!(momentum_verdict(4.0, -8.0), "mixed");    // pumping but down on the week ⇒ caution
    }

    #[test]
    fn cmc_error_body_yields_a_reason() {
        let b = serde_json::json!({"status": {"error_code": 1002, "error_message": "API key missing."}});
        assert_eq!(parse_cmc_error(&b), "API key missing.");
        assert!(!parse_cmc_error(&serde_json::json!({})).is_empty());
    }
}
