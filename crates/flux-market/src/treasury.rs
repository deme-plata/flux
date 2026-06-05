//! treasury.rs — the trading agent's money. Mining profit (GPU-coins→BTC swap)
//! credits the treasury; the agent **DCAs** it into BTC on a schedule and scans
//! for arbitrage from BOTH Binance (CEX↔on-chain) and Polymarket (betting).
//! PROPOSES opportunities; never auto-spends real money.

use crate::arb::ArbSignal;
use crate::dca::{DcaPlan, USDS_ONE};
use crate::polymarket::ArbMarket;

/// The agent's treasury: a DCA plan + accumulated mining profit to deploy.
#[derive(Debug, Clone)]
pub struct Treasury {
    pub dca: DcaPlan,
    /// Accumulated mining profit available to deploy (sats).
    pub profit_sat: u128,
}

impl Treasury {
    pub fn new(dca: DcaPlan) -> Self {
        Self { dca, profit_sat: 0 }
    }

    /// Mining (GPU-coin → BTC swap) credits realised profit here.
    pub fn credit_mining_profit(&mut self, sat: u128) {
        self.profit_sat += sat;
    }

    /// How much USDS the accumulated profit is worth at `btc_usd` (for sizing
    /// the next DCA buys). 6-dec USDS units.
    pub fn deployable_usds(&self, btc_usd: f64) -> u128 {
        let btc = self.profit_sat as f64 / 100_000_000.0;
        ((btc * btc_usd) * USDS_ONE as f64) as u128
    }

    /// Is a DCA buy due, and is there profit to fund at least one buy?
    pub fn ready_to_dca(&self, now: u64, btc_usd: f64) -> bool {
        self.dca.due(now) && self.deployable_usds(btc_usd) >= self.dca.buy_usds
    }
}

/// Everything the agent looks at in one cycle.
#[derive(Debug, Clone)]
pub struct Opportunities {
    /// Binance vs on-chain wBTC/USDS arb.
    pub cex_arb: ArbSignal,
    /// Polymarket buy-both betting arbs (best first).
    pub bet_arbs: Vec<ArbMarket>,
}

impl Opportunities {
    pub fn summary(&self) -> String {
        let top_bet = self
            .bet_arbs
            .first()
            .map(|m| format!("{:.1}% on \"{}\"", m.margin_pct, truncate(&m.question, 40)))
            .unwrap_or_else(|| "none".into());
        format!("CEX arb: {} | best bet arb: {} ({} markets)", self.cex_arb.summary(), top_bet, self.bet_arbs.len())
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "…"
    }
}

/// Run the agent's combined opportunity scan: live Binance arb + live Polymarket
/// betting arbs, in one call (the trading-agent's MCP combo).
pub fn scan_opportunities(
    symbol: &str,
    onchain_usd: f64,
    min_cex_pct: f64,
    min_bet_pct: f64,
    bet_limit: u32,
) -> Result<Opportunities, String> {
    let snap = crate::snapshot(symbol, onchain_usd, min_cex_pct)?;
    let bet_arbs = crate::polymarket::scan_arbs(min_bet_pct, bet_limit)?;
    Ok(Opportunities { cex_arb: snap.arb, bet_arbs })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dca::Interval;

    #[test]
    fn mining_profit_accumulates_and_sizes_dca() {
        let mut t = Treasury::new(DcaPlan::new("wBTC", 100 * USDS_ONE, Interval::Daily, 0));
        t.credit_mining_profit(150_000); // 150k sat from GPU→BTC swaps
        // at $80k/BTC, 150k sat = 0.0015 BTC = $120 → 120 USDS deployable.
        let dep = t.deployable_usds(80_000.0);
        assert!((dep as i128 - 120 * USDS_ONE as i128).abs() < USDS_ONE as i128, "≈120 USDS, got {}", dep / USDS_ONE);
        // enough to fund a $100 daily buy.
        assert!(t.ready_to_dca(Interval::Daily.secs(), 80_000.0));
    }
}
