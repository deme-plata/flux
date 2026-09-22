//! The DEX leg of the same trading surface — a Uniswap-V2-style pool read straight off Polygon.
//!
//! Bitunix gives you a CEX order book; this gives you the wSIGIL/USDC pool. One slider UI drives
//! both, so both live behind one crate. Read-only: this module never signs or broadcasts anything,
//! it only tells you what a trade WOULD do.
//!
//! # The decimals trap, made structural
//! The live pair has `token0 = USDC (6 dp)` and `token1 = wSIGIL (18 dp)`. Getting that backwards is
//! not a small error — it is a **10^27** price error. So [`PoolState::price_usdc_per_token`] takes
//! the decimals from the reserve struct that was actually read from chain, and `token0_is_usdc` is
//! recorded from a live `token0()` call rather than assumed.

use serde_json::{json, Value};

/// Public Polygon RPCs, tried in order. The same list the live wsigil-market.js page uses.
pub const POLYGON_RPCS: &[&str] = &[
    "https://polygon-bor-rpc.publicnode.com",
    "https://polygon-rpc.com",
    "https://1rpc.io/matic",
];

/// USDC on Polygon (native, 6 decimals).
pub const USDC: &str = "0x3c499c542cef5e3811e1192ce70d8cc03d5c3359";
/// wSIGIL3 — the current wrapped SIGIL token (18 decimals).
pub const WSIGIL3: &str = "0x3fced760b0de6d57f96835c6110b1227941ec2e9";
/// The wSIGIL3/USDC pair.
pub const PAIR_WSIGIL3: &str = "0x7e9c5d73104ed2fc2bc55139ac0999036eaf41d7";
/// The OLDER wSIGIL/USDC pair — still referenced by the live market page, kept here so a caller can
/// read it deliberately rather than by accident.
pub const PAIR_WSIGIL_LEGACY: &str = "0x06895e77a192ce525c7bb5f25c52d2ce19c052d1";

// Uniswap V2 selectors
const SEL_RESERVES: &str = "0x0902f1ac"; // getReserves()
const SEL_TOKEN0: &str = "0x0dfe1681";   // token0()
const SEL_TOKEN1: &str = "0xd21220a7";   // token1()

fn eth_call(to: &str, data: &str) -> Result<String, String> {
    let http = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(12))
        .build().map_err(|e| e.to_string())?;
    let body = json!({"jsonrpc":"2.0","id":1,"method":"eth_call",
                      "params":[{"to":to,"data":data},"latest"]});
    let mut last = String::from("no RPC tried");
    for rpc in POLYGON_RPCS {
        match http.post(*rpc).json(&body).send() {
            Ok(r) => match r.json::<Value>() {
                Ok(v) => {
                    if let Some(res) = v.get("result").and_then(|x| x.as_str()) {
                        if res.len() > 2 { return Ok(res.to_string()); }
                        last = format!("{rpc}: empty result (contract has no code?)");
                    } else {
                        last = format!("{rpc}: {}", v.get("error").map(|e| e.to_string()).unwrap_or_default());
                    }
                }
                Err(e) => last = format!("{rpc}: bad JSON: {e}"),
            },
            Err(e) => last = format!("{rpc}: {e}"),
        }
    }
    Err(format!("no Polygon RPC answered: {last}"))
}

/// Split a hex return blob into 32-byte words as u128 (enough for any reserve; V2 reserves are u112).
fn words(hex_blob: &str) -> Vec<u128> {
    let h = hex_blob.trim_start_matches("0x");
    h.as_bytes().chunks(64).filter(|c| c.len() == 64)
        .map(|c| {
            let s = std::str::from_utf8(c).unwrap_or("0");
            // take the low 32 hex chars — a u112/u128 never needs more
            u128::from_str_radix(&s[32..], 16).unwrap_or(0)
        }).collect()
}

fn word_to_address(hex_blob: &str) -> String {
    let h = hex_blob.trim_start_matches("0x");
    if h.len() < 64 { return String::new(); }
    format!("0x{}", &h[24..64].to_lowercase())
}

#[derive(Debug, Clone)]
pub struct PoolState {
    pub pair: String,
    pub token0: String,
    pub token1: String,
    pub reserve0: u128,
    pub reserve1: u128,
    pub token0_is_usdc: bool,
}

impl PoolState {
    /// USDC reserve as a human number. USDC is 6 dp on Polygon.
    pub fn usdc(&self) -> f64 {
        let raw = if self.token0_is_usdc { self.reserve0 } else { self.reserve1 };
        raw as f64 / 1e6
    }
    /// wSIGIL reserve as a human number. wSIGIL is 18 dp.
    pub fn wsigil(&self) -> f64 {
        let raw = if self.token0_is_usdc { self.reserve1 } else { self.reserve0 };
        raw as f64 / 1e18
    }
    /// Spot price, USDC per wSIGIL. `None` when the pool holds no wSIGIL (undefined, not zero).
    pub fn price_usdc_per_token(&self) -> Option<f64> {
        let w = self.wsigil();
        if w > 0.0 { Some(self.usdc() / w) } else { None }
    }
    /// Total value locked, in USDC terms (both sides).
    pub fn tvl_usd(&self) -> f64 { self.usdc() * 2.0 }
}

/// Constant-product output for a swap, with the standard 0.3% fee. Pure and tested — this is the
/// number a slider must show BEFORE anyone signs.
pub fn amm_out(amount_in: f64, reserve_in: f64, reserve_out: f64) -> f64 {
    if amount_in <= 0.0 || reserve_in <= 0.0 || reserve_out <= 0.0 { return 0.0; }
    let in_after_fee = amount_in * 0.997;
    (in_after_fee * reserve_out) / (reserve_in + in_after_fee)
}

/// Price impact of a trade, in percent. This is the number that decides whether a pool is tradeable
/// at all — on a thin pool it is the whole story, and it must never be hidden behind a pretty slider.
pub fn price_impact_pct(amount_in: f64, reserve_in: f64, reserve_out: f64) -> f64 {
    let out = amm_out(amount_in, reserve_in, reserve_out);
    if out <= 0.0 || reserve_in <= 0.0 { return f64::INFINITY; }
    let spot = reserve_out / reserve_in;      // out-per-in before the trade
    let effective = out / amount_in;          // out-per-in actually received
    (spot / effective - 1.0) * 100.0
}

/// Read a pair's live state from Polygon.
pub fn read_pool(pair: &str) -> Result<PoolState, String> {
    let t0 = word_to_address(&eth_call(pair, SEL_TOKEN0)?);
    let t1 = word_to_address(&eth_call(pair, SEL_TOKEN1)?);
    let res = words(&eth_call(pair, SEL_RESERVES)?);
    if res.len() < 2 { return Err(format!("pair {pair} returned no reserves")); }
    Ok(PoolState {
        pair: pair.to_lowercase(),
        token0_is_usdc: t0.eq_ignore_ascii_case(USDC),
        token0: t0, token1: t1,
        reserve0: res[0], reserve1: res[1],
    })
}

/// The full quote a slider needs: what you get, what it costs you, and how badly the pool moves.
/// `side` is "buy" (spend USDC, receive wSIGIL) or "sell" (spend wSIGIL, receive USDC).
pub fn quote(p: &PoolState, side: &str, amount_in: f64) -> Value {
    let (r_in, r_out, in_sym, out_sym) = if side.eq_ignore_ascii_case("sell") {
        (p.wsigil(), p.usdc(), "wSIGIL", "USDC")
    } else {
        (p.usdc(), p.wsigil(), "USDC", "wSIGIL")
    };
    let out = amm_out(amount_in, r_in, r_out);
    let impact = price_impact_pct(amount_in, r_in, r_out);
    // A pool this thin cannot absorb a real trade; say so in the payload, not in a footnote.
    let verdict = if !impact.is_finite() { "UNTRADEABLE — pool is empty on one side" }
        else if impact > 50.0 { "UNTRADEABLE — over 50% price impact; you would lose most of the input" }
        else if impact > 5.0 { "POOR — over 5% price impact" }
        else if impact > 1.0 { "OK — 1–5% price impact" }
        else { "GOOD — under 1% price impact" };
    json!({
        "side": side.to_lowercase(),
        "amount_in": amount_in, "in_symbol": in_sym,
        "amount_out": out, "out_symbol": out_sym,
        "effective_price_usdc_per_wsigil":
            if side.eq_ignore_ascii_case("sell") { if amount_in > 0.0 { out / amount_in } else { 0.0 } }
            else if out > 0.0 { amount_in / out } else { 0.0 },
        "spot_price_usdc_per_wsigil": p.price_usdc_per_token(),
        "price_impact_pct": if impact.is_finite() { json!(impact) } else { json!("infinite") },
        "verdict": verdict,
        "fee_pct": 0.3,
    })
}

/// Pool state + a slippage ladder — the exact payload the trading UI's slider renders.
pub fn pool_report(pair: &str) -> Result<Value, String> {
    let p = read_pool(pair)?;
    let ladder: Vec<Value> = [1.0f64, 5.0, 10.0, 50.0, 100.0].iter()
        .map(|usd| quote(&p, "buy", *usd)).collect();
    Ok(json!({
        "ok": true,
        "pair": p.pair,
        "token0": p.token0, "token1": p.token1,
        "token0_is_usdc": p.token0_is_usdc,
        "reserve_usdc": p.usdc(),
        "reserve_wsigil": p.wsigil(),
        "spot_price_usdc_per_wsigil": p.price_usdc_per_token(),
        "tvl_usd": p.tvl_usd(),
        "buy_ladder": ladder,
        "note": "read-only. Reserves come from a live getReserves() call; decimals are taken from the \
                 on-chain token0() result, never assumed (USDC 6dp vs wSIGIL 18dp is a 10^27 error).",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(usdc_units: u128, wsigil_units: u128) -> PoolState {
        PoolState { pair: "0xpair".into(), token0: USDC.into(), token1: WSIGIL3.into(),
                    reserve0: usdc_units, reserve1: wsigil_units, token0_is_usdc: true }
    }

    #[test]
    fn decimals_are_applied_per_token_not_uniformly() {
        // 1_000_000 USDC units = 1.0 USDC ; 1e18 wSIGIL units = 1.0 wSIGIL
        let p = pool(1_000_000, 1_000_000_000_000_000_000);
        assert!((p.usdc() - 1.0).abs() < 1e-12);
        assert!((p.wsigil() - 1.0).abs() < 1e-12);
        assert!((p.price_usdc_per_token().unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn reversed_token_order_still_reads_correctly() {
        let mut p = pool(1_000_000, 1_000_000_000_000_000_000);
        // flip: now token0 is wSIGIL and token1 is USDC, with the reserves swapped to match
        p.token0_is_usdc = false;
        p.reserve0 = 1_000_000_000_000_000_000;
        p.reserve1 = 1_000_000;
        assert!((p.usdc() - 1.0).abs() < 1e-12, "USDC must still be read at 6dp from the right slot");
        assert!((p.wsigil() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn price_is_none_not_zero_when_one_side_is_empty() {
        let p = pool(1_000_000, 0);
        assert!(p.price_usdc_per_token().is_none(), "an empty side makes price undefined, not zero");
    }

    #[test]
    fn amm_out_respects_constant_product_and_fee() {
        // deep pool: 1_000_000 USDC vs 1_000_000 wSIGIL, buy with 1000 USDC
        let out = amm_out(1000.0, 1_000_000.0, 1_000_000.0);
        assert!(out < 1000.0, "fee + curve mean you always get less than 1:1");
        assert!(out > 995.0, "a 0.1% trade in a deep pool should be close to 1:1, got {out}");
    }

    #[test]
    fn amm_out_is_zero_for_nonsense_inputs() {
        assert_eq!(amm_out(0.0, 100.0, 100.0), 0.0);
        assert_eq!(amm_out(-5.0, 100.0, 100.0), 0.0);
        assert_eq!(amm_out(10.0, 0.0, 100.0), 0.0);
    }

    #[test]
    fn price_impact_is_small_on_deep_pools_and_huge_on_thin_ones() {
        let deep = price_impact_pct(1.0, 1_000_000.0, 1_000_000.0);
        assert!(deep < 0.5, "a $1 trade in a $1M pool must barely move it, got {deep}%");
        let thin = price_impact_pct(1.0, 0.098, 0.0102);
        assert!(thin > 100.0, "a $1 trade in a $0.10 pool must show massive impact, got {thin}%");
    }

    #[test]
    fn quote_calls_a_thin_pool_untradeable() {
        // the REAL measured wSIGIL3 pool: 0.097983 USDC / 0.010239 wSIGIL
        let p = pool(97_983, 10_239_000_000_000_000);
        let q = quote(&p, "buy", 10.0);
        assert!(q["verdict"].as_str().unwrap().starts_with("UNTRADEABLE"),
                "10 USD into a 20-cent pool must be flagged, got {}", q["verdict"]);
    }

    #[test]
    fn quote_calls_a_deep_pool_good() {
        let p = pool(1_000_000_000_000, 1_000_000_000_000_000_000_000_000);
        let q = quote(&p, "buy", 10.0);
        assert!(q["verdict"].as_str().unwrap().starts_with("GOOD"), "got {}", q["verdict"]);
    }

    #[test]
    fn word_decoding_extracts_an_address_from_a_padded_slot() {
        let padded = "0x0000000000000000000000003c499c542cef5e3811e1192ce70d8cc03d5c3359";
        assert_eq!(word_to_address(padded), USDC);
    }

    #[test]
    fn words_splits_a_getreserves_blob() {
        let blob = "0x0000000000000000000000000000000000000000000000000000000000017ebf\
                    00000000000000000000000000000000000000000000000000246052ea06e1b4\
                    000000000000000000000000000000000000000000000000000000006a9d9269";
        let w = words(blob);
        assert_eq!(w.len(), 3);
        assert_eq!(w[0], 0x17ebf);
    }
}
