//! binance.rs — live spot price from Binance's public REST API (no key needed
//! for price reads). "so you know the price." A thin blocking client; the MCP
//! combo and the arb/DCA engine consume `spot_price`.

use serde::Deserialize;
use std::time::Duration;

#[derive(Deserialize)]
struct TickerPrice {
    #[allow(dead_code)]
    symbol: String,
    price: String,
}

#[derive(Deserialize)]
struct Ticker24h {
    #[serde(rename = "lastPrice")]
    last_price: String,
    #[serde(rename = "priceChangePercent")]
    price_change_percent: String,
    #[serde(rename = "highPrice")]
    high_price: String,
    #[serde(rename = "lowPrice")]
    low_price: String,
}

/// A 24h market snapshot for a symbol.
#[derive(Debug, Clone)]
pub struct MarketTicker {
    pub symbol: String,
    pub last: f64,
    pub change_pct_24h: f64,
    pub high_24h: f64,
    pub low_24h: f64,
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent("flux-market/0.1")
        .build()
        .map_err(|e| format!("client init: {e}"))
}

/// Live spot price for e.g. "BTCUSDT". Binance USDT ≈ USD for our purposes.
pub fn spot_price(symbol: &str) -> Result<f64, String> {
    let url = format!("https://api.binance.com/api/v3/ticker/price?symbol={symbol}");
    let t: TickerPrice = client()?
        .get(&url)
        .send()
        .map_err(|e| format!("connect: {e}"))?
        .error_for_status()
        .map_err(|e| format!("http: {e}"))?
        .json()
        .map_err(|e| format!("decode: {e}"))?;
    t.price.parse::<f64>().map_err(|e| format!("parse price: {e}"))
}

/// Live 24h ticker (last / change% / high / low) for a symbol.
pub fn ticker_24h(symbol: &str) -> Result<MarketTicker, String> {
    let url = format!("https://api.binance.com/api/v3/ticker/24hr?symbol={symbol}");
    let t: Ticker24h = client()?
        .get(&url)
        .send()
        .map_err(|e| format!("connect: {e}"))?
        .error_for_status()
        .map_err(|e| format!("http: {e}"))?
        .json()
        .map_err(|e| format!("decode: {e}"))?;
    let p = |s: &str| s.parse::<f64>().unwrap_or(0.0);
    Ok(MarketTicker {
        symbol: symbol.to_string(),
        last: p(&t.last_price),
        change_pct_24h: p(&t.price_change_percent),
        high_24h: p(&t.high_price),
        low_24h: p(&t.low_price),
    })
}
