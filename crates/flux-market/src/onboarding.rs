//! onboarding.rs — the Phase-1 first-customer flow (Claude Code Max engine).
//!
//! Ties the three pieces into one lifecycle: pick a [`crate::pricing`] tier →
//! meter tasks through the [`crate::governor`] 20% cap (so one $200 plan serves
//! ~5 customers on Opus 4.8) → bill with a CVR [`crate::invoice`]. Zero infra:
//! the engine is Claude Code, not a rented GPU. Propose-only: the agent does the
//! work + invoices; the customer pays the account directly.

use crate::governor::CustomerUsage;
use crate::invoice::{Invoice, LineItem, Party};
use crate::pricing::tiers;

#[derive(Debug, Clone)]
pub struct Customer {
    pub id: String,
    pub company: String,
    pub cvr: Option<String>,
    pub tier_name: String,
    pub price_mo: f64,
    pub usage: CustomerUsage,
    pub tasks_done: u32,
}

/// Sign up a customer on a tier. Returns None if the tier name is unknown.
pub fn onboard(id: &str, company: &str, cvr: Option<&str>, tier: &str) -> Option<Customer> {
    let t = tiers().into_iter().find(|x| x.name.eq_ignore_ascii_case(tier))?;
    Some(Customer {
        id: id.into(), company: company.into(), cvr: cvr.map(String::from),
        tier_name: t.name.into(), price_mo: t.price_mo,
        usage: CustomerUsage::new(id), tasks_done: 0,
    })
}

impl Customer {
    /// Accept a task only if it stays within the customer's 20% engine cap.
    pub fn intake(&mut self, task_tokens: u64) -> Result<u64, String> {
        if !self.usage.can_serve(task_tokens) {
            return Err(format!("over 20% engine cap ({:.0}% used) — upgrade or wait for monthly reset", self.usage.pct_used()));
        }
        self.usage.record(task_tokens);
        self.tasks_done += 1;
        Ok(self.usage.remaining())
    }

    /// The monthly invoice for this customer (tier price → one line, CVR + moms).
    pub fn monthly_invoice(&self, number: &str, date: &str, due: &str, seller: Party) -> Invoice {
        Invoice {
            number: number.into(), date: date.into(), due_date: due.into(),
            seller,
            buyer: Party { name: self.company.clone(), cvr: self.cvr.clone(), address: String::new() },
            items: vec![LineItem {
                description: format!("Flux/SIGIL agent — {} plan ({} tasks)", self.tier_name, self.tasks_done),
                qty: 1.0, unit_price_dkk: self.price_mo,
            }],
            pay_to: "MobilePay / IBAN (your account)".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn onboards_known_tier_rejects_unknown() {
        assert!(onboard("c1", "Acme ApS", Some("87654321"), "Pro").is_some());
        assert!(onboard("c1", "Acme", None, "Platinum").is_none());
    }

    #[test]
    fn intake_is_gated_by_the_20pct_cap() {
        let mut c = onboard("c1", "Acme", None, "Pro").unwrap();
        assert!(c.intake(19_000_000).is_ok());
        assert!(c.intake(1_000_000).is_ok());   // to the cap
        assert!(c.intake(2_000_000).is_err());  // over → rejected
        assert_eq!(c.tasks_done, 2);
    }

    #[test]
    fn invoice_carries_tier_price_and_moms() {
        let c = onboard("c1", "Acme ApS", Some("87654321"), "Business").unwrap();
        let seller = Party { name: "Flux/SIGIL".into(), cvr: Some("12345678".into()), address: "DK".into() };
        let inv = c.monthly_invoice("2026-001", "2026-06-01", "2026-06-15", seller);
        assert!((inv.subtotal() - 499.0).abs() < 1e-6);
        assert!((inv.total() - 623.75).abs() < 1e-6); // 499 × 1.25
        assert!(inv.render_html().contains("Business plan"));
    }
}
