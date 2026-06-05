//! strategy.rs — the profitability GATE the MCP webhook runs each tick.
//!
//! Hard truth (measured): GPU mining on a RENTED Vast box loses money — a GTX
//! 1660S does ~21 MH/s ETC (≈$0.01–0.02/hr) but rents for $0.1435/hr → ~10× net
//! loss. So the agent compares mining revenue vs the Vast cost, and when mining
//! loses (it does), it pivots to **arbitrage + heavy Carl-Runefelt DCA**
//! (accumulate, never sell). Owned GPUs (electricity-only) flip the math — the
//! gate handles both by comparing whatever cost you feed it.

/// A mining-vs-rental check for one GPU.
#[derive(Debug, Clone)]
pub struct RentalMiningCheck {
    pub gpu: String,
    pub hashrate_mhs: f64,
    /// Estimated coin revenue (USD/hr) at current price+difficulty.
    pub est_revenue_usd_hr: f64,
    /// What this GPU costs per hour (Vast rental, or your electricity).
    pub cost_usd_hr: f64,
}

impl RentalMiningCheck {
    pub fn net_usd_hr(&self) -> f64 {
        self.est_revenue_usd_hr - self.cost_usd_hr
    }
    /// revenue / cost. >1 = profitable, <1 = burning money.
    pub fn roi(&self) -> f64 {
        if self.cost_usd_hr > 0.0 {
            self.est_revenue_usd_hr / self.cost_usd_hr
        } else {
            f64::INFINITY
        }
    }
    pub fn profitable(&self) -> bool {
        self.net_usd_hr() > 0.0
    }
}

/// What the agent should DO with its compute/capital this tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Mining clears its cost → mine, then DCA the profit (Carl rule 4).
    MineThenDca,
    /// Mining loses → DON'T mine; harvest arbitrage + DCA hard (Carl rules 1,4,5).
    ArbThenDca,
}

/// The decision: mine only if it beats its own cost; otherwise arb + accumulate.
pub fn decide(c: &RentalMiningCheck) -> Action {
    if c.profitable() {
        Action::MineThenDca
    } else {
        Action::ArbThenDca
    }
}

/// One-line rationale for the webhook payload / agent report.
pub fn rationale(c: &RentalMiningCheck) -> String {
    let a = decide(c);
    format!(
        "{}: {:.1} MH/s · rev ${:.3}/hr vs cost ${:.3}/hr → net ${:+.3}/hr (ROI {:.2}×) ⇒ {}",
        c.gpu, c.hashrate_mhs, c.est_revenue_usd_hr, c.cost_usd_hr, c.net_usd_hr(), c.roi(),
        match a { Action::MineThenDca => "MINE + DCA profit", Action::ArbThenDca => "STOP mining → ARB + DCA hard (Runefelt)" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rented_gpu_mining_loses_and_pivots_to_arb() {
        // The REAL measured case: GTX 1660S, 21.36 MH/s ETC, ~$0.015/hr revenue,
        // $0.1435/hr Vast rental.
        let c = RentalMiningCheck { gpu: "GTX 1660S (Vast)".into(), hashrate_mhs: 21.36, est_revenue_usd_hr: 0.015, cost_usd_hr: 0.1435 };
        assert!(!c.profitable());
        assert!(c.roi() < 0.2, "rented mining burns ~10×, roi {}", c.roi());
        assert_eq!(decide(&c), Action::ArbThenDca);
    }

    #[test]
    fn owned_gpu_electricity_only_can_mine() {
        // Owned card, electricity-only (~$0.005/hr at cheap power) → mining clears.
        let c = RentalMiningCheck { gpu: "RTX 4090 (owned)".into(), hashrate_mhs: 130.0, est_revenue_usd_hr: 0.12, cost_usd_hr: 0.05 };
        assert!(c.profitable());
        assert_eq!(decide(&c), Action::MineThenDca);
    }
}
