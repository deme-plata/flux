//! arb.rs — arbitrage-opportunity detector between a CEX price (Binance) and the
//! on-chain wBTC/USDS price (the SIGIL bridge's DEX). Spread ≥ threshold ⇒ an
//! opportunity; the direction says which leg to buy. Mirrors Quillon's
//! qshare_premium_arbitrage idea, generalised CEX↔on-chain.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArbDir {
    /// On-chain is cheaper: buy wBTC on-chain, sell on the CEX.
    BuyOnchainSellCex,
    /// CEX is cheaper: buy on the CEX, bridge in, sell on-chain.
    BuyCexSellOnchain,
    None,
}

#[derive(Debug, Clone)]
pub struct ArbSignal {
    pub cex_usd: f64,
    pub onchain_usd: f64,
    /// (cex - onchain) / onchain * 100. Positive ⇒ on-chain cheaper.
    pub spread_pct: f64,
    pub direction: ArbDir,
    pub actionable: bool,
}

impl ArbSignal {
    pub fn summary(&self) -> String {
        let dir = match self.direction {
            ArbDir::BuyOnchainSellCex => "buy on-chain → sell CEX",
            ArbDir::BuyCexSellOnchain => "buy CEX → bridge → sell on-chain",
            ArbDir::None => "no actionable spread",
        };
        format!("CEX ${:.2} vs on-chain ${:.2} · spread {:+.3}% · {}", self.cex_usd, self.onchain_usd, self.spread_pct, dir)
    }
}

/// Scan one CEX/on-chain pair. `min_pct` should exceed round-trip costs
/// (bridge + DEX 0.3% + CEX fee) before a spread is truly actionable.
pub fn scan(cex_usd: f64, onchain_usd: f64, min_pct: f64) -> ArbSignal {
    let spread_pct = if onchain_usd > 0.0 { (cex_usd - onchain_usd) / onchain_usd * 100.0 } else { 0.0 };
    let actionable = spread_pct.abs() >= min_pct;
    let direction = if !actionable {
        ArbDir::None
    } else if onchain_usd < cex_usd {
        ArbDir::BuyOnchainSellCex
    } else {
        ArbDir::BuyCexSellOnchain
    };
    ArbSignal { cex_usd, onchain_usd, spread_pct, direction, actionable }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_onchain_discount() {
        // on-chain 1% cheaper than CEX → buy on-chain, sell CEX.
        let s = scan(100_000.0, 99_000.0, 0.5);
        assert!(s.actionable);
        assert_eq!(s.direction, ArbDir::BuyOnchainSellCex);
        assert!((s.spread_pct - 1.0101).abs() < 0.01);
    }

    #[test]
    fn detects_cex_discount() {
        let s = scan(99_000.0, 100_000.0, 0.5);
        assert!(s.actionable);
        assert_eq!(s.direction, ArbDir::BuyCexSellOnchain);
    }

    #[test]
    fn ignores_sub_threshold_spread() {
        let s = scan(100_010.0, 100_000.0, 0.5); // 0.01% < 0.5%
        assert!(!s.actionable);
        assert_eq!(s.direction, ArbDir::None);
    }
}
