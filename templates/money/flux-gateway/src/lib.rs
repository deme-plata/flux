//! flux-gateway — the **+10% reseller** mold.
//!
//! Route a compute/API job to the cheapest healthy provider, skim the red line,
//! settle in SIGIL. Built on [`flux_money_kit`]. This is the proven revenue rail:
//! buy work wholesale from a provider, sell it at +10%, keep the spread.

use flux_money_kit::{skim, MoneyError, Result, Treasury, NATIVE};

/// House wallet that collects the gateway's skim.
pub const HOUSE: &str = "gateway-house";

/// A backend that can execute units of work for a per-unit price (µQUG).
#[derive(Debug, Clone)]
pub struct Provider {
    pub id: String,
    pub price_per_unit: u128,
    pub region: String,
    pub healthy: bool,
}

/// A unit of work a customer wants routed.
#[derive(Debug, Clone)]
pub struct Job {
    pub id: String,
    pub customer: String,
    pub units: u128,
    /// Optional region affinity; `None` = any region.
    pub region: Option<String>,
}

/// The priced result of routing a job: gross split into provider + gateway cuts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invoice {
    pub job_id: String,
    pub provider_id: String,
    pub gross: u128,
    pub provider_cut: u128,
    pub gateway_cut: u128,
}

/// The reseller. Holds an escrow [`Treasury`], a skim rate, and a provider pool.
pub struct Gateway {
    pub treasury: Treasury,
    /// Skim in basis points (1000 = 10%).
    pub fee_bps: u32,
    providers: Vec<Provider>,
}

impl Gateway {
    pub fn new(fee_bps: u32) -> Self {
        Self { treasury: Treasury::new(), fee_bps, providers: Vec::new() }
    }

    pub fn register_provider(&mut self, p: Provider) {
        self.providers.push(p);
    }

    /// Pick the cheapest healthy provider honoring the job's region affinity.
    pub fn route(&self, job: &Job) -> Option<&Provider> {
        self.providers.iter()
            .filter(|p| p.healthy)
            .filter(|p| job.region.as_ref().map_or(true, |r| &p.region == r))
            .min_by_key(|p| p.price_per_unit)
    }

    /// Quote a job: route to the best provider + compute the +fee skim split.
    pub fn quote(&self, job: &Job) -> Result<Invoice> {
        let p = self.route(job).ok_or(MoneyError::Unroutable)?;
        let gross = p.price_per_unit.checked_mul(job.units).ok_or(MoneyError::Overflow)?;
        let (provider_cut, gateway_cut) = skim(gross, self.fee_bps)?;
        Ok(Invoice {
            job_id: job.id.clone(), provider_id: p.id.clone(),
            gross, provider_cut, gateway_cut,
        })
    }

    /// Settle: customer pays `gross`, provider receives the net, house keeps the
    /// skim. Customer must be pre-funded in the treasury (escrow model).
    pub fn settle_job(&mut self, job: &Job, inv: &Invoice) -> Result<()> {
        self.treasury.debit(&job.customer, NATIVE, inv.gross)?;
        self.treasury.credit(&inv.provider_id, NATIVE, inv.provider_cut)?;
        self.treasury.credit(HOUSE, NATIVE, inv.gateway_cut)?;
        Ok(())
    }

    /// Lifetime red-line earnings of the house.
    pub fn earnings(&self) -> u128 {
        self.treasury.balance(HOUSE, NATIVE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux_money_kit::MICRO;

    fn gw() -> Gateway {
        let mut g = Gateway::new(1000); // 10%
        g.register_provider(Provider { id: "vast-a".into(), price_per_unit: 5 * MICRO, region: "us".into(), healthy: true });
        g.register_provider(Provider { id: "peer-b".into(), price_per_unit: 3 * MICRO, region: "eu".into(), healthy: true });
        g.register_provider(Provider { id: "down-c".into(), price_per_unit: 1 * MICRO, region: "eu".into(), healthy: false });
        g
    }

    #[test]
    fn routes_cheapest_healthy() {
        let g = gw();
        let job = Job { id: "j1".into(), customer: "cust".into(), units: 10, region: None };
        assert_eq!(g.route(&job).unwrap().id, "peer-b"); // 3µ < 5µ; down-c (1µ) skipped — unhealthy
    }

    #[test]
    fn region_affinity_filters() {
        let g = gw();
        let job = Job { id: "j2".into(), customer: "cust".into(), units: 1, region: Some("us".into()) };
        assert_eq!(g.route(&job).unwrap().id, "vast-a");
    }

    #[test]
    fn quote_skims_ten_percent() {
        let g = gw();
        let job = Job { id: "j3".into(), customer: "cust".into(), units: 10, region: None };
        let inv = g.quote(&job).unwrap();
        assert_eq!(inv.gross, 30 * MICRO);        // 3µ × 10
        assert_eq!(inv.gateway_cut, 3 * MICRO);   // 10%
        assert_eq!(inv.provider_cut, 27 * MICRO);
    }

    #[test]
    fn settle_conserves_and_pays_house() {
        let mut g = gw();
        let job = Job { id: "j4".into(), customer: "cust".into(), units: 10, region: None };
        g.treasury.credit("cust", NATIVE, 30 * MICRO).unwrap();
        let inv = g.quote(&job).unwrap();
        g.settle_job(&job, &inv).unwrap();
        assert_eq!(g.treasury.balance("cust", NATIVE), 0);
        assert_eq!(g.treasury.balance("peer-b", NATIVE), 27 * MICRO);
        assert_eq!(g.earnings(), 3 * MICRO);
    }

    #[test]
    fn unroutable_when_all_down() {
        let mut g = Gateway::new(1000);
        g.register_provider(Provider { id: "x".into(), price_per_unit: MICRO, region: "us".into(), healthy: false });
        let job = Job { id: "j".into(), customer: "c".into(), units: 1, region: None };
        assert_eq!(g.quote(&job), Err(MoneyError::Unroutable));
    }
}
