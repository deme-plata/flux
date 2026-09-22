//! Read-only market aggregation. Quotes are proposals; this module never signs or broadcasts.
use crate::{now_ms, Bitunix};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

pub type Params = Vec<(String, String)>;

fn required<'a>(p: &'a [(String, String)], key: &str) -> Result<&'a str, String> {
    let mut values = p.iter().filter(|(k, _)| k == key);
    let value = values.next().map(|(_, v)| v.as_str()).filter(|v| !v.trim().is_empty())
        .ok_or_else(|| format!("{key} is required"))?;
    if values.next().is_some() { return Err(format!("duplicate {key}")); }
    Ok(value)
}

fn integer(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|c| c.is_ascii_digit()) && value.bytes().any(|c| c != b'0')
}

/// No token catalog or small chain whitelist: 0x remains authoritative for liquidity and coverage.
pub fn query_for(route: &str, p: &[(String, String)]) -> Result<Params, String> {
    let (allowed, needed): (&[&str], &[&str]) = match route {
        "/swap/chains" | "/cross-chain/sources" => (&[], &[]),
        "/sources" => (&["chainId"], &["chainId"]),
        "/swap/permit2/price" | "/swap/allowance-holder/price" => (
            &["chainId", "sellToken", "buyToken", "sellAmount", "taker", "slippageBps"],
            &["chainId", "sellToken", "buyToken", "sellAmount"]),
        "/swap/permit2/quote" | "/swap/allowance-holder/quote" => (
            &["chainId", "sellToken", "buyToken", "sellAmount", "taker", "slippageBps"],
            &["chainId", "sellToken", "buyToken", "sellAmount", "taker"]),
        "/cross-chain/quotes" => (
            &["originChain", "destinationChain", "sellToken", "buyToken", "sellAmount",
              "originAddress", "destinationAddress", "sortQuotesBy", "slippageBps",
              "includedBridges", "excludedBridges", "excludedSwapSources", "maxNumQuotes",
              "gasPayer", "solanaEphemeralSignerPubkey"],
            &["originChain", "destinationChain", "sellToken", "buyToken", "sellAmount",
              "originAddress", "destinationAddress"]),
        "/cross-chain/status" => (
            &["originChain", "originTxHash", "quoteId"], &["originChain", "originTxHash"]),
        _ => return Err("unsupported read-only 0x route".into()),
    };
    for key in needed { required(p, key)?; }
    if p.iter().any(|(k,_)| k == "includedBridges") && p.iter().any(|(k,_)| k == "excludedBridges") {
        return Err("includedBridges and excludedBridges are mutually exclusive".into());
    }
    let mut query = Vec::new();
    for key in allowed {
        if p.iter().any(|(k, _)| k == key) {
            let v = required(p, key)?;
            if matches!(*key, "chainId" | "sellAmount") && !integer(v) {
                return Err(format!("{key} must be a positive base-unit integer"));
            }
            if *key == "slippageBps" && v.parse::<u32>().map_or(true, |v| v > 10_000) {
                return Err("slippageBps must be between 0 and 10000".into());
            }
            if *key == "sortQuotesBy" && !matches!(v, "price" | "speed") {
                return Err("sortQuotesBy must be price or speed".into());
            }
            if *key == "maxNumQuotes" && v.parse::<u32>().map_or(true, |v| !(1..=10).contains(&v)) {
                return Err("maxNumQuotes must be between 1 and 10".into());
            }
            if *key == "chainId" && matches!(v, "999999999991" | "999999999992" | "999999999993") {
                return Err("non-EVM chain: use /0x/solana/instructions or /0x/crosschain".into());
            }
            query.push((key.to_string(), v.to_string()));
        }
    }
    if route == "/cross-chain/quotes" && !query.iter().any(|(k, _)| k == "sortQuotesBy") {
        query.push(("sortQuotesBy".into(), "price".into()));
    }
    Ok(query)
}

fn client() -> Result<(reqwest::blocking::Client, String), String> {
    let key = std::env::var("FLUX_0X_API_KEY").ok()
        .or_else(|| std::fs::read_to_string("/root/.config/0x/api_key").ok())
        .map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
        .ok_or("0x API key is not configured")?;
    let http = reqwest::blocking::Client::builder().timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none()).build().map_err(|_| "0x client setup failed")?;
    Ok((http, key))
}

/// Preserve integer amounts as provider strings, and unavailable metrics as null, never zero.
pub fn quote_metrics(v: &Value) -> Value {
    let units = |value: Option<&Value>| value.and_then(|n| match n {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) if n.is_u64() || n.is_i64() => Some(n.to_string()),
        _ => None,
    });
    json!({
        "sell_amount_base_units": units(v.get("sellAmount")),
        "buy_amount_base_units": units(v.get("buyAmount").or_else(|| v.get("amount_out"))),
        "minimum_received_base_units": units(v.get("minBuyAmount").or_else(|| v.get("min_amount_out"))),
        "liquidity_available": v.get("liquidityAvailable"),
        "fees": v.get("fees"), "gas": v.get("gas"), "gas_price": v.get("gasPrice"),
        "total_network_fee": v.get("totalNetworkFee"),
        "issues": v.get("issues"), "token_metadata": v.get("tokenMetadata"),
        "route": v.get("route").or_else(|| v.get("route_plan")),
        "provider_request_id": v.get("zid"), "block_number": v.get("blockNumber"),
        "price_impact": v.get("priceImpact"), "requires_wallet_signature": true,
        "quote_id": v.get("quoteId"), "gas_costs": v.get("gasCosts"),
        "estimated_time_seconds": v.get("estimatedTimeSeconds"), "steps": v.get("steps"),
        "quotes": v.get("quotes").and_then(Value::as_array).map(|qs| qs.iter().map(quote_metrics).collect::<Vec<_>>()),
        "submitted": false
    })
}

fn response(r: reqwest::blocking::Response, started: Instant) -> Result<Value, String> {
    let status = r.status().as_u16();
    let retry_after = r.headers().get("retry-after").and_then(|v| v.to_str().ok()).map(str::to_owned);
    let data: Value = r.json().map_err(|_| format!("0x HTTP {status}: invalid JSON response"))?;
    Ok(json!({"ok": (200..300).contains(&status), "status": status, "source": "0x",
        "observed_at_ms": now_ms(), "latency_ms": started.elapsed().as_millis() as u64,
        "retry_after": retry_after, "metrics": quote_metrics(&data), "data": data}))
}

pub fn zerox(route: &str, params: &[(String, String)]) -> Result<Value, String> {
    let query = query_for(route, params)?;
    let (http, key) = client()?;
    let started = Instant::now();
    // Query encoding is essential: token addresses and user values must never inject parameters.
    let r = http.get(format!("https://api.0x.org{route}")).query(&query)
        .header("0x-api-key", key).header("0x-version", "v2")
        .send().map_err(|_| "0x request failed (network or timeout)".to_string())?;
    response(r, started)
}

pub fn solana_body(v: &Value) -> Result<Value, String> {
    let mut body = serde_json::Map::new();
    for key in ["taker", "token_in", "token_out"] {
        let s = v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
            .ok_or_else(|| format!("{key} is required"))?;
        body.insert(key.into(), json!(s));
    }
    // Accept a decimal string from JavaScript to avoid its 53-bit integer limit.
    let amount = v.get("amount_in").and_then(|a| a.as_u64().or_else(|| a.as_str()?.parse().ok()))
        .filter(|a| *a > 0).ok_or("amount_in must be a positive u64 base-unit integer")?;
    let slip = match v.get("slippage_bps") {
        Some(s) => s.as_u64().filter(|s| *s <= 10_000).ok_or("invalid slippage_bps")?,
        None => 50,
    };
    body.insert("amount_in".into(), json!(amount));
    body.insert("slippage_bps".into(), json!(slip));
    if let Some(recipient) = v.get("recipient") { body.insert("recipient".into(), recipient.clone()); }
    Ok(Value::Object(body))
}

pub fn solana_instructions(v: &Value) -> Result<Value, String> {
    let body = solana_body(v)?;
    let (http, key) = client()?;
    let started = Instant::now();
    let r = http.post("https://api.0x.org/solana/swap-instructions")
        .header("0x-api-key", key).json(&body).send()
        .map_err(|_| "0x Solana request failed (network or timeout)".to_string())?;
    response(r, started)
}

pub fn capabilities() -> Value {
    // Each upstream has its own error; a discovery outage is never an empty support list.
    let evm = zerox("/swap/chains", &[]).unwrap_or_else(|e| json!({"ok": false, "error": e}));
    let cross = zerox("/cross-chain/sources", &[]).unwrap_or_else(|e| json!({"ok": false, "error": e}));
    json!({"ok": evm["ok"] == true && cross["ok"] == true,
        "evm": evm, "cross_chain": cross,
        "solana": {"chain_id": "999999999991", "instructions_path": "/0x/solana/instructions",
                   "adapter_implemented": true, "live_availability": "requires successful quote"},
        "token_count": null, "token_policy": "address-based, subject to provider liquidity and eligibility",
        "execution": "wallet-signed only", "native_sigil_vm": "separate from external chain execution"})
}

pub fn bitunix_metrics(v: &Value) -> Value {
    let rows = v.get("data").and_then(Value::as_array).or_else(|| v.as_array());
    let result: Vec<Value> = rows.into_iter().flatten().map(|t| {
        let num = |key: &str| t.get(key).and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse().ok()))
            .filter(|v: &f64| v.is_finite());
        let change = num("open").filter(|p| *p > 0.0).and_then(|p| num("lastPrice").map(|last| (last / p - 1.0) * 100.0));
        json!({"symbol": t.get("symbol"), "market_type": "futures", "quote_currency": "USDT",
            "last_price": t.get("lastPrice"), "mark_price": t.get("markPrice"),
            "change_24h_pct": change, "high_24h": t.get("high"), "low_24h": t.get("low"),
            "base_volume_24h": t.get("baseVol"), "quote_volume_24h": t.get("quoteVol"),
            "provider_timestamp": t.get("timestamp"), "is_executable_dex_quote": false})
    }).collect();
    json!(result)
}

pub fn prices(p: &[(String, String)]) -> Value {
    let started = Instant::now();
    let symbols = p.iter().find(|(k, _)| k == "symbols").map(|(_, v)| v.as_str()).unwrap_or("");
    let bitunix = match Bitunix::public().and_then(|b| b.tickers(symbols)) {
        Ok(data) => json!({"ok": true, "source": "bitunix", "observed_at_ms": now_ms(),
            "latency_ms": started.elapsed().as_millis() as u64, "markets": bitunix_metrics(&data)}),
        Err(e) => json!({"ok": false, "source": "bitunix", "error": e}),
    };
    let dex = if p.iter().any(|(k, _)| k == "sellToken") {
        zerox("/swap/permit2/price", p).unwrap_or_else(|e| json!({"ok": false, "source": "0x", "error": e}))
    } else { json!({"requested": false}) };
    let partial = bitunix["ok"] != true || (dex.get("ok").is_some() && dex["ok"] != true);
    json!({"ok": !partial, "partial": partial, "bitunix": bitunix, "dex": dex,
        "note": "Futures prices and DEX quotes are separate markets; no blended or guaranteed execution price."})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn p(items: &[(&str, &str)]) -> Params { items.iter().map(|(k,v)| (k.to_string(),v.to_string())).collect() }
    #[test] fn future_evm_chain_and_large_amount_pass_without_catalog() {
        let q = query_for("/swap/permit2/price", &p(&[("chainId","123456789"),("sellToken","0xabc"),
            ("buyToken","0xdef"),("sellAmount","340282366920938463463374607431768211455")])).unwrap();
        assert_eq!(q[3].1, "340282366920938463463374607431768211455");
    }
    #[test] fn invalid_inputs_and_submission_routes_rejected() {
        assert!(query_for("/order", &[]).is_err());
        let mut q = p(&[("chainId","137"),("sellToken","a"),("buyToken","b"),("sellAmount","0")]);
        assert!(query_for("/swap/permit2/price", &q).is_err());
        q[3].1 = "1".into(); q.push(("chainId".into(), "1".into()));
        assert!(query_for("/swap/permit2/price", &q).is_err());
        assert!(query_for("/swap/permit2/quote", &q[..4]).is_err());
    }
    #[test] fn non_evm_routes_require_destination_and_preserve_names() {
        let mut q = p(&[("originChain","base"),("destinationChain","tron"),("sellToken","a"),
            ("buyToken","b"),("sellAmount","10"),("originAddress","c")]);
        assert!(query_for("/cross-chain/quotes", &q).is_err());
        q.push(("destinationAddress".into(), "Trecipient".into()));
        assert!(query_for("/cross-chain/quotes", &q).unwrap().contains(&("destinationChain".into(),"tron".into())));
    }
    #[test] fn amounts_and_unknown_metrics_remain_honest() {
        let m = quote_metrics(&json!({"buyAmount":"999999999999999999999999", "liquidityAvailable":false}));
        assert_eq!(m["buy_amount_base_units"], "999999999999999999999999");
        assert_eq!(m["liquidity_available"], false);
        assert!(m["price_impact"].is_null()); assert_eq!(m["submitted"], false);
    }
    #[test] fn svm_base_units_do_not_round() {
        let b = solana_body(&json!({"amount_in":"9007199254740993","taker":"a","token_in":"b","token_out":"c"})).unwrap();
        assert_eq!(b["amount_in"].as_u64(), Some(9007199254740993));
        assert_eq!(quote_metrics(&json!({"amount_out":9007199254740993u64}))["buy_amount_base_units"], "9007199254740993");
        assert!(solana_body(&json!({"amount_in":1.5})).is_err());
    }
    #[test] fn futures_are_not_dex_prices() {
        let m = bitunix_metrics(&json!({"data":[{"symbol":"BTCUSDT","open":"100","lastPrice":"110"}]}));
        assert!((m[0]["change_24h_pct"].as_f64().unwrap()-10.0).abs()<1e-10);
        assert_eq!(m[0]["is_executable_dex_quote"], false);
        assert!(m[0]["mark_price"].is_null());
    }
}
