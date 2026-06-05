//! governor.rs — the engine-fusion usage governor.
//!
//! One Claude Code Max plan ($200/mo) serves the FIRST WAVE by capping each
//! customer at **20% of the plan's monthly token budget** → up to ~5 customers
//! share it fairly, none can hog it. Model is fixed to **Opus 4.8 at max effort**
//! (best quality for paying customers). Propose-only — the governor meters
//! tokens, it never spends money.

/// Effective monthly token budget of the Max plan (Opus 4.8). Tunable.
pub const PLAN_MONTHLY_TOKENS: u64 = 100_000_000;
/// Per-customer share of the plan (the "only use 20%" rule).
pub const PER_CUSTOMER_FRACTION: f64 = 0.20;
pub const MODEL: &str = "claude-opus-4-8";
pub const EFFORT: &str = "max";

pub fn per_customer_cap() -> u64 { (PLAN_MONTHLY_TOKENS as f64 * PER_CUSTOMER_FRACTION) as u64 }
/// How many customers one plan can serve at the 20% cap.
pub fn max_customers() -> u32 { (1.0 / PER_CUSTOMER_FRACTION) as u32 }

#[derive(Debug, Clone)]
pub struct CustomerUsage { pub id: String, pub used_tokens: u64 }
impl CustomerUsage {
    pub fn new(id: impl Into<String>) -> Self { Self { id: id.into(), used_tokens: 0 } }
    pub fn remaining(&self) -> u64 { per_customer_cap().saturating_sub(self.used_tokens) }
    pub fn pct_used(&self) -> f64 { self.used_tokens as f64 / per_customer_cap() as f64 * 100.0 }
    /// Gate a request: only serve if it stays within the customer's 20% cap.
    pub fn can_serve(&self, request_tokens: u64) -> bool { self.used_tokens + request_tokens <= per_customer_cap() }
    /// Record usage after a served request.
    pub fn record(&mut self, tokens: u64) { self.used_tokens += tokens; }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twenty_percent_each_five_customers() {
        assert_eq!(per_customer_cap(), 20_000_000);
        assert_eq!(max_customers(), 5);
    }

    #[test]
    fn gate_blocks_over_cap_serves_under() {
        let mut c = CustomerUsage::new("acme");
        assert!(c.can_serve(19_000_000));
        c.record(19_000_000);
        assert!(c.can_serve(1_000_000));     // exactly to the cap
        assert!(!c.can_serve(2_000_000));    // over the 20% cap → blocked
        assert!((c.pct_used() - 95.0).abs() < 1e-6);
    }

    #[test]
    fn opus_max_effort_is_the_engine() {
        assert_eq!(MODEL, "claude-opus-4-8");
        assert_eq!(EFFORT, "max");
    }
}
