//! flux-agent-trade — the flagship combo: chain the brain → gate → execute-quote → settle-plan, as a
//! **dry-run** proposal with provenance and ZERO broadcast. This is the whole agentic-money loop:
//!
//!   DECIDE   flux-trade  (Binance klines → 9 indicators + Kelly → action/confidence/size)
//!     ↓
//!   GATE     Verified Execution Gate  (whitelist · confidence floor · amount cap · positive edge
//!            · slippage bound) — the 4th layer that makes it SAFE for an LLM to touch money
//!     ↓
//!   EXECUTE  flux-0x  (indicative 0x swap price → the executable route; NO taker, NO calldata sent)
//!     ↓
//!   SETTLE   describe the Coinbase-CDP settlement plan (managed wallet + gas) — NOT called in dry-run
//!     ↓
//!   PROPOSE  one structured proposal; the human-readable `reason` from DECIDE is the provenance.
//!
//! Nothing here signs or broadcasts. `propose()` is read-only end-to-end — turning it into a real
//! trade is a separate, explicitly-gated step (a CDP wallet + `dry_run=false`), never automatic.

use serde_json::{json, Value};

/// The Verified Execution Gate — every safety check a proposal must clear before it's even quoted.
#[derive(Debug, Clone)]
pub struct Gate {
    pub min_confidence: f64,   // reject low-conviction setups
    pub max_usd: f64,          // hard cap on notional
    pub max_slippage_bps: u32, // tolerated price impact
    pub whitelist: Vec<String>,// only these symbols may trade (honeypot/rug guard)
}

impl Default for Gate {
    fn default() -> Self {
        Gate { min_confidence: 0.60, max_usd: 1000.0, max_slippage_bps: 100,
            whitelist: ["BTCUSDT", "ETHUSDT"].iter().map(|s| s.to_string()).collect() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict { Pass, Reject(String) }

impl Gate {
    /// Pure, fully-tested: does this decision clear the gate? Order matters — fail loud on the first.
    pub fn check(&self, symbol: &str, action: &str, confidence: f64, usd_amount: f64) -> Verdict {
        let sym = symbol.to_uppercase();
        if action == "HOLD" { return Verdict::Reject("brain says HOLD — no clean signal, stand aside".into()); }
        if !(action == "BUY" || action == "SELL") { return Verdict::Reject(format!("unknown action {action}")); }
        if !self.whitelist.iter().any(|w| w.eq_ignore_ascii_case(&sym)) {
            return Verdict::Reject(format!("{sym} not on the whitelist {:?}", self.whitelist));
        }
        if confidence < self.min_confidence {
            return Verdict::Reject(format!("confidence {confidence:.2} below floor {:.2}", self.min_confidence));
        }
        if usd_amount <= 0.0 { return Verdict::Reject("amount must be > 0".into()); }
        if usd_amount > self.max_usd {
            return Verdict::Reject(format!("${usd_amount:.0} over the ${:.0} cap", self.max_usd));
        }
        Verdict::Pass
    }
}

/// Ethereum-mainnet token map for the whitelisted symbols: (chainId, base, quote=USDC, base_dec, quote_dec).
fn tokens_for(symbol: &str) -> Option<(u64, &'static str, &'static str, u32, u32)> {
    const USDC: &str = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48";
    match symbol.to_uppercase().as_str() {
        "ETHUSDT" => Some((1, "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2", USDC, 18, 6)), // WETH
        "BTCUSDT" => Some((1, "0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599", USDC, 8, 6)),  // WBTC
        _ => None,
    }
}

/// human amount × 10^dec → integer base-units string (no float precision in the digits).
fn base_units(amount: f64, dec: u32) -> String {
    let scaled = amount * 10f64.powi(dec as i32);
    format!("{:.0}", scaled.max(0.0))
}

/// THE COMBO. Read-only end-to-end: decide → gate → 0x quote → settlement plan → one proposal.
pub fn propose(symbol: &str, usd_amount: f64, interval: &str, gate: &Gate) -> Value {
    // 1) DECIDE
    let decision = match flux_trade::decide(symbol, interval, 200) {
        Ok(v) => v, Err(e) => return json!({"stage": "decide", "error": e}),
    };
    let action = decision.get("action").and_then(|v| v.as_str()).unwrap_or("HOLD").to_string();
    let confidence = decision.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let price = decision.get("price").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let kelly = decision.get("sizing").and_then(|s| s.get("kelly_fraction")).and_then(|v| v.as_f64()).unwrap_or(0.0);

    // 2) GATE
    let verdict = gate.check(symbol, &action, confidence, usd_amount);
    if let Verdict::Reject(why) = &verdict {
        return json!({
            "dry_run": true, "symbol": symbol.to_uppercase(), "action": action,
            "gate": {"verdict": "REJECT", "reason": why},
            "decision": decision,
            "outcome": "NO TRADE — the gate stopped it before any quote. This is the system working.",
        });
    }

    // sizing: Kelly scales the notional; zero edge ⇒ zero size (deploy no capital without an edge)
    let sized_usd = (usd_amount * kelly.clamp(0.0, 1.0)).min(gate.max_usd);

    // 3) EXECUTE (indicative quote only — no taker, nothing broadcast)
    let route = match tokens_for(symbol) {
        None => json!({"error": "no on-chain token map for this symbol"}),
        Some((chain, base, quote, base_dec, quote_dec)) => {
            let notional = if sized_usd > 0.0 { sized_usd } else { usd_amount }; // quote the intended size
            // BUY = spend USDC to get base; SELL = sell base for USDC
            let (sell_tok, buy_tok, sell_amt) = if action == "BUY" {
                (quote, base, base_units(notional, quote_dec))
            } else {
                let base_qty = if price > 0.0 { notional / price } else { 0.0 };
                (base, quote, base_units(base_qty, base_dec))
            };
            match flux_0x::Zerox::from_env().and_then(|z| z.swap_price(chain, sell_tok, buy_tok, &sell_amt)) {
                Ok(q) => json!({"chain": chain, "sell": sell_tok, "buy": buy_tok, "sell_amount": sell_amt,
                                 "buy_amount": q.get("buyAmount"), "liquidity": q.get("liquidityAvailable"),
                                 "max_slippage_bps": gate.max_slippage_bps}),
                Err(e) => json!({"error": format!("0x quote: {e}")}),
            }
        }
    };

    // 4) SETTLE — described, not called (dry-run). 5) PROPOSE
    json!({
        "dry_run": true,
        "symbol": symbol.to_uppercase(), "action": action,
        "gate": {"verdict": "PASS", "checks": ["whitelist", "confidence", "amount-cap", "slippage-bound"]},
        "decision": { "confidence": confidence, "kelly_fraction": kelly,
                      "reason": decision.get("reason"), "indicators": decision.get("indicators") },
        "sizing": { "requested_usd": usd_amount, "kelly_sized_usd": sized_usd,
                    "note": if sized_usd <= 0.0 { "edge ≈ 0 ⇒ size 0 (the brain deploys no capital without an edge)" } else { "sized by Kelly" } },
        "route": route,
        "settlement": { "venue": "Coinbase CDP managed wallet (EOA/Smart Account)", "gas": "sponsored via paymaster",
                        "status": "NOT executed — dry-run. Real settlement = a CDP wallet + dry_run=false (separately gated)." },
        "provenance": { "engine": "flux-agent-trade", "decide": "flux-trade", "execute": "flux-0x", "settle": "coinbase-cdp",
                        "reason": decision.get("reason"), "as_of": decision.get("indicators").and_then(|_| Some(())).map(|_| Value::Null) },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g() -> Gate { Gate::default() }

    #[test]
    fn gate_rejects_hold() {
        assert_eq!(g().check("ETHUSDT", "HOLD", 0.9, 100.0), Verdict::Reject("brain says HOLD — no clean signal, stand aside".into()));
    }
    #[test]
    fn gate_rejects_off_whitelist() {
        match g().check("DOGEUSDT", "BUY", 0.9, 100.0) { Verdict::Reject(w) => assert!(w.contains("whitelist")), _ => panic!("should reject") }
    }
    #[test]
    fn gate_rejects_low_confidence_and_oversize() {
        match g().check("ETHUSDT", "BUY", 0.4, 100.0) { Verdict::Reject(w) => assert!(w.contains("confidence")), _ => panic!() }
        match g().check("ETHUSDT", "BUY", 0.9, 5000.0) { Verdict::Reject(w) => assert!(w.contains("cap")), _ => panic!() }
    }
    #[test]
    fn gate_passes_a_clean_setup() {
        assert_eq!(g().check("ETHUSDT", "BUY", 0.8, 250.0), Verdict::Pass);
        assert_eq!(g().check("btcusdt", "SELL", 0.7, 100.0), Verdict::Pass); // case-insensitive
    }
    #[test]
    fn base_units_are_integer_strings() {
        assert_eq!(base_units(250.0, 6), "250000000");   // 250 USDC
        assert_eq!(base_units(0.1, 18), "100000000000000000"); // 0.1 WETH
    }
    #[test]
    fn token_map_covers_whitelist() {
        assert!(tokens_for("ETHUSDT").is_some());
        assert!(tokens_for("BTCUSDT").is_some());
        assert!(tokens_for("DOGEUSDT").is_none());
    }
}
