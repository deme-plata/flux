//! dca.rs — dollar-cost-averaging engine. Quillon's q-trading-bot dca.rs +
//! dca_api.rs (recurring interval buys, slippage-protected, avg-cost tracked),
//! Flux-native and quoted in USDS (the SIGIL stablecoin, 6 decimals = $1).
//!
//! A plan buys a fixed USDS amount of an asset every interval; we track total
//! spent + total acquired so `avg_cost` is always known — the whole point of DCA.

/// USDS has 6 decimals: 1 USDS (=$1) = 1_000_000 units.
pub const USDS_DECIMALS: u32 = 6;
pub const USDS_ONE: u128 = 1_000_000;

/// satoshis per BTC (the acquired asset is wBTC, sat-denominated).
pub const SATS_PER_BTC: f64 = 100_000_000.0;

/// Buy cadence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interval {
    Hourly,
    Daily,
    Weekly,
}

impl Interval {
    pub fn secs(self) -> u64 {
        match self {
            Interval::Hourly => 3_600,
            Interval::Daily => 86_400,
            Interval::Weekly => 604_800,
        }
    }
}

/// A recurring DCA plan + its running cost basis.
#[derive(Debug, Clone)]
pub struct DcaPlan {
    pub asset: String,
    /// USDS spent per buy (6-dec units).
    pub buy_usds: u128,
    pub interval: Interval,
    pub last_buy_ts: u64,
    pub total_spent_usds: u128,
    /// satoshis acquired so far.
    pub total_acquired_sat: u128,
}

impl DcaPlan {
    pub fn new(asset: impl Into<String>, buy_usds: u128, interval: Interval, start_ts: u64) -> Self {
        Self { asset: asset.into(), buy_usds, interval, last_buy_ts: start_ts, total_spent_usds: 0, total_acquired_sat: 0 }
    }

    /// Is a buy due at `now`?
    pub fn due(&self, now: u64) -> bool {
        now >= self.last_buy_ts.saturating_add(self.interval.secs())
    }

    /// Execute one DCA buy at `price_usd` (USD per BTC). Spends `buy_usds`,
    /// acquires the corresponding sats, advances the clock, updates cost basis.
    /// Returns sats acquired this buy.
    pub fn record_buy(&mut self, price_usd: f64, now: u64) -> u128 {
        let usd = self.buy_usds as f64 / USDS_ONE as f64;
        let btc = if price_usd > 0.0 { usd / price_usd } else { 0.0 };
        let sats = (btc * SATS_PER_BTC) as u128;
        self.total_spent_usds += self.buy_usds;
        self.total_acquired_sat += sats;
        self.last_buy_ts = now;
        sats
    }

    /// Average cost in USD per BTC across all buys so far.
    pub fn avg_cost_usd(&self) -> f64 {
        if self.total_acquired_sat == 0 {
            return 0.0;
        }
        let spent_usd = self.total_spent_usds as f64 / USDS_ONE as f64;
        let btc = self.total_acquired_sat as f64 / SATS_PER_BTC;
        spent_usd / btc
    }

    /// Unrealised PnL % at the current market price vs the average cost.
    pub fn pnl_pct(&self, market_usd: f64) -> f64 {
        let avg = self.avg_cost_usd();
        if avg == 0.0 {
            return 0.0;
        }
        (market_usd - avg) / avg * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dca_averages_cost_across_buys() {
        // $100/buy. Buy at 50k, then at 100k → avg should be ~66.7k.
        let mut p = DcaPlan::new("wBTC", 100 * USDS_ONE, Interval::Daily, 0);
        p.record_buy(50_000.0, Interval::Daily.secs());
        p.record_buy(100_000.0, 2 * Interval::Daily.secs());
        let avg = p.avg_cost_usd();
        assert!((avg - 66_666.0).abs() < 50.0, "avg {avg} should be ~66,666 (harmonic of equal-$ buys)");
        assert_eq!(p.total_spent_usds, 200 * USDS_ONE);
    }

    #[test]
    fn due_respects_interval() {
        let p = DcaPlan::new("wBTC", USDS_ONE, Interval::Hourly, 1000);
        assert!(!p.due(1000 + 3599));
        assert!(p.due(1000 + 3600));
    }

    #[test]
    fn pnl_tracks_market_vs_avg() {
        let mut p = DcaPlan::new("wBTC", 100 * USDS_ONE, Interval::Daily, 0);
        p.record_buy(50_000.0, 0);
        assert!((p.pnl_pct(100_000.0) - 100.0).abs() < 1.0, "bought at 50k, now 100k → +100%");
    }
}
