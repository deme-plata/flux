//! flux_btc_* — the Bitcoin accumulation combos.
//!
//! Wraps the `flux-btc` brain (which composes flux-trade indicators + Kelly
//! and flux-market fear/greed) into MCP tools an agent calls to answer "is
//! this a dip worth DCA-ing into, and how much?". Everything is PROPOSE-ONLY:
//! the tools return a recommendation with `execute:false`; a human confirms
//! every real Bitcoin spend. On a strong-dip verdict, `flux_btc_analyze`
//! fires a webhook so a signed alert can land in the Buzz #trading channel.

use serde_json::{json, Value};
use crate::handlers::{ToolDef, ToolRegistry};
use fluxc_webhooks::webhook;

/// A dip this strong (0–100) is worth surfacing to the human proactively.
const ALERT_THRESHOLD: f64 = 65.0;

fn flux_btc_analyze(args: &Value) -> String {
    let interval = args.get("interval").and_then(|v| v.as_str()).unwrap_or("1d");
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(200) as u32;
    match flux_btc::analyze(interval, limit) {
        Ok(v) => {
            let out = v.to_json();
            if v.dip_strength >= ALERT_THRESHOLD {
                // Signed Buzz alert lands via the wired webhook sink.
                webhook::auto_dispatch("btc_dip_alert", json!({
                    "channel": "trading",
                    "dip_strength": v.dip_strength,
                    "price_usd": v.price_usd,
                    "reason": v.reason,
                }));
            }
            serde_json::to_string_pretty(&out).unwrap_or_else(|_| out.to_string())
        }
        Err(e) => json!({"ok": false, "error": e}).to_string(),
    }
}

fn flux_btc_dca_plan(args: &Value) -> String {
    let interval = args.get("interval").and_then(|v| v.as_str()).unwrap_or("1d");
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(200) as u32;
    let base_usd = args.get("base_usd").and_then(|v| v.as_f64()).unwrap_or(100.0);
    match flux_btc::dca_plan(interval, limit, base_usd) {
        Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string()),
        Err(e) => json!({"ok": false, "error": e}).to_string(),
    }
}

fn flux_btc_indicators(args: &Value) -> String {
    let interval = args.get("interval").and_then(|v| v.as_str()).unwrap_or("1d");
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(200) as u32;
    match flux_trade::decide("BTCUSDT", interval, limit) {
        Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string()),
        Err(e) => json!({"ok": false, "error": e}).to_string(),
    }
}

fn flux_btc_fear_greed(_args: &Value) -> String {
    match flux_market::fear_greed::fetch() {
        Ok(fg) => json!({
            "value": fg.value,
            "sentiment": fg.sentiment.label(),
            "dca_multiplier": fg.sentiment.dca_multiplier(),
        })
        .to_string(),
        Err(e) => json!({"ok": false, "error": e}).to_string(),
    }
}

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_btc_analyze",
            description: "PROPOSE-ONLY Bitcoin dip analysis: composes 9 technical indicators (RSI/MACD/Bollinger/ADX…), the crypto Fear & Greed index, and price-vs-MA into one dip_strength score (0–100) + a contrarian DCA multiple. Fires a Buzz #trading alert on a strong dip. Never moves real BTC. interval=Binance kline (1d/1w), limit=candles.",
            input_schema: json!({"type": "object", "properties": {
                "interval": {"type": "string", "description": "Binance kline interval (default 1d; try 1w for the macro view)"},
                "limit": {"type": "integer", "description": "Number of candles (default 200)"}
            }}),
        },
        flux_btc_analyze,
    );
    registry.register(
        ToolDef {
            name: "flux_btc_dca_plan",
            description: "PROPOSE-ONLY DCA proposal for Bitcoin: runs flux_btc_analyze then sizes a concrete ticket for a base USD amount (base_usd × dip-tilted multiple), returning suggested_usd + suggested_btc with execute:false. A human confirms every spend. Carl-Runefelt rule: DCA the dip, never sell the core.",
            input_schema: json!({"type": "object", "properties": {
                "interval": {"type": "string", "description": "Binance kline interval (default 1d)"},
                "limit": {"type": "integer", "description": "Number of candles (default 200)"},
                "base_usd": {"type": "number", "description": "Base DCA ticket in USD (default 100)"}
            }}),
        },
        flux_btc_dca_plan,
    );
    registry.register(
        ToolDef {
            name: "flux_btc_indicators",
            description: "Raw technical-indicator snapshot for BTCUSDT: RSI, MACD, Bollinger bands, ADX, ATR, Ichimoku, OBV, VWAP + confluence long/short signals. The DECIDE layer behind flux_btc_analyze, no paid signal API.",
            input_schema: json!({"type": "object", "properties": {
                "interval": {"type": "string", "description": "Binance kline interval (default 1d)"},
                "limit": {"type": "integer", "description": "Number of candles (default 200)"}
            }}),
        },
        flux_btc_indicators,
    );
    registry.register(
        ToolDef {
            name: "flux_btc_fear_greed",
            description: "Live crypto Fear & Greed index (0=extreme fear → 100=extreme greed) with its contrarian DCA multiplier. Extreme fear = buy more.",
            input_schema: json!({"type": "object", "properties": {}}),
        },
        flux_btc_fear_greed,
    );
}
