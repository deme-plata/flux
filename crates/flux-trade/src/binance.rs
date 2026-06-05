//! Binance Futures (fapi) market-data client — the price feed for the DECIDE layer. PUBLIC endpoints
//! only (klines + mark price), so NO API key is needed to read the market. Lifted/slimmed from
//! q-narwhalknight's `q-trading-bot::binance` (which also has signed order placement; trading is the
//! gated next step, requires key+secret). Resilient: short retry on transient failures.

use serde_json::Value;

pub const FAPI: &str = "https://fapi.binance.com";

/// One OHLCV candle (the indicators eat these).
#[derive(Debug, Clone, Copy)]
pub struct Candle { pub high: f64, pub low: f64, pub close: f64, pub volume: f64 }

pub struct Binance { base: String, http: reqwest::blocking::Client }

impl Binance {
    pub fn new() -> Self {
        let http = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(15)).build().unwrap_or_default();
        Binance { base: FAPI.into(), http }
    }

    fn get(&self, path: &str, q: &[(&str, String)]) -> Result<Value, String> {
        let url = format!("{}{}", self.base, path);
        let mut attempt = 0u32;
        loop {
            match self.http.get(&url).query(q).send() {
                Ok(r) => {
                    let s = r.status();
                    if s.is_success() { return r.json::<Value>().map_err(|e| format!("binance {path} decode: {e}")); }
                    let code = s.as_u16();
                    if matches!(code, 429 | 418 | 500 | 502 | 503 | 504) && attempt < 3 {
                        std::thread::sleep(std::time::Duration::from_millis(300 * (1 << attempt)));
                        attempt += 1; continue;
                    }
                    return Err(format!("binance {path} HTTP {code}"));
                }
                Err(e) => {
                    if (e.is_timeout() || e.is_connect()) && attempt < 3 {
                        std::thread::sleep(std::time::Duration::from_millis(300 * (1 << attempt)));
                        attempt += 1; continue;
                    }
                    return Err(format!("binance {path} network: {e}"));
                }
            }
        }
    }

    /// Current mark price for a symbol (e.g. "BTCUSDT").
    pub fn mark_price(&self, symbol: &str) -> Result<f64, String> {
        let v = self.get("/fapi/v1/ticker/price", &[("symbol", symbol.to_uppercase())])?;
        v.get("price").and_then(|p| p.as_str()).and_then(|s| s.parse().ok())
            .ok_or_else(|| format!("no price for {symbol}"))
    }

    /// Candlestick OHLCV. `interval` like 1m/5m/15m/1h/4h/1d. Binance returns arrays:
    /// [openTime, open, high, low, close, volume, closeTime, ...].
    pub fn klines(&self, symbol: &str, interval: &str, limit: u32) -> Result<Vec<Candle>, String> {
        let v = self.get("/fapi/v1/klines", &[
            ("symbol", symbol.to_uppercase()), ("interval", interval.into()),
            ("limit", limit.clamp(1, 1500).to_string()),
        ])?;
        let arr = v.as_array().ok_or("klines: expected an array")?;
        let f = |k: &Value, i: usize| k.get(i).and_then(|x| x.as_str()).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
        Ok(arr.iter().filter_map(|k| {
            if k.as_array().map(|a| a.len() >= 6).unwrap_or(false) {
                Some(Candle { high: f(k, 2), low: f(k, 3), close: f(k, 4), volume: f(k, 5) })
            } else { None }
        }).collect())
    }
}

impl Default for Binance { fn default() -> Self { Self::new() } }
