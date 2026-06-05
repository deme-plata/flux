//! flux-advisor — propose-only advisor on the agent's OWN comms + trading.
//!
//! COMMS: flags fabrication risk (a metric with no read-from-file evidence),
//! nudges including other agents, checks actionability. TRADING: a Carl-Runefelt
//! action from price + sentiment + arb. It NEVER sends a message or spends — it
//! returns advice the agent (or human) acts on. This encodes the session's own
//! discipline: honest numbers, include the swarm, propose don't auto-execute.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommsFlag {
    /// A metric (%, $, GH/s, TPS…) appears with no evidence word nearby.
    FabricationRisk,
    /// Long message that doesn't reference the swarm / another agent.
    IncludeOthers,
    /// No clear next step / ask.
    NotActionable,
}

const EVIDENCE: &[&str] = &[
    "read-from", "read from", "measured", "verified", "output", "test result",
    "tok/s", "tok_per_s", "http ", "passed", "gh/s", "from a machine", "logged",
];

/// Advise on an outgoing comms message (propose-only — does not send).
pub fn comms_advice(msg: &str) -> Vec<CommsFlag> {
    let lc = msg.to_lowercase();
    let mut flags = vec![];
    let has_metric = msg.contains('%') || msg.contains('$')
        || lc.contains("tps") || lc.contains("gh/s") || lc.contains("x faster") || lc.contains("/mtok");
    if has_metric && !EVIDENCE.iter().any(|e| lc.contains(e)) {
        flags.push(CommsFlag::FabricationRisk);
    }
    if msg.len() > 40 && !msg.contains('@') && !lc.contains("swarm") && !lc.contains("rocky") {
        flags.push(CommsFlag::IncludeOthers);
    }
    if msg.len() > 40 && !lc.contains("next") && !lc.contains("want me to")
        && !msg.contains('?') && !msg.contains('→') {
        flags.push(CommsFlag::NotActionable);
    }
    flags
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeAdvice { BuyDip, TakeArb, Hold }

/// Carl-Runefelt advice from live signals. Risk-free arb first, then buy the
/// fear dip; otherwise HOLD the core (never sell). Propose-only.
pub fn trade_advice(price: f64, trend: f64, _fng: u8, arb_pct: f64) -> TradeAdvice {
    if arb_pct >= 2.0 { TradeAdvice::TakeArb }
    else if price < trend { TradeAdvice::BuyDip }
    else { TradeAdvice::Hold }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_a_naked_metric_as_fabrication_risk() {
        assert!(comms_advice("we hit 90% accuracy and 500k TPS").contains(&CommsFlag::FabricationRisk));
        // same numbers WITH evidence → no fabrication flag
        assert!(!comms_advice("90% accuracy, read-from-output of the test result").contains(&CommsFlag::FabricationRisk));
    }

    #[test]
    fn nudges_including_the_swarm() {
        assert!(comms_advice("I finished the bridge crate and all the tests are green now").contains(&CommsFlag::IncludeOthers));
        assert!(!comms_advice("@rocky-sigil the bridge crate is green").contains(&CommsFlag::IncludeOthers));
    }

    #[test]
    fn runefelt_trade_logic() {
        assert_eq!(trade_advice(60_000.0, 65_000.0, 20, 0.0), TradeAdvice::BuyDip);   // dip
        assert_eq!(trade_advice(70_000.0, 65_000.0, 80, 0.0), TradeAdvice::Hold);     // above trend, hold core
        assert_eq!(trade_advice(60_000.0, 65_000.0, 20, 3.5), TradeAdvice::TakeArb);  // arb wins
    }
}
