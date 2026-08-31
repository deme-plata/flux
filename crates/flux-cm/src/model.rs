//! Core data model: chains, currencies, CM profiles, work items.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Which network a CM serves or a work item belongs to.
///
/// `Polygon` covers the external-venue work around wSIGIL — the Uniswap
/// wSIGIL/USDC pool and NFT drops live there, not on a centralized broker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Chain {
    Quillon,
    Sigil,
    Polygon,
}

impl Chain {
    pub fn name(&self) -> &'static str {
        match self {
            Chain::Quillon => "Quillon Graph",
            Chain::Sigil => "SIGIL",
            Chain::Polygon => "Polygon",
        }
    }
}

/// Settlement currency for bounties. Amounts are always carried in BASE units
/// (u128) because the two chains disagree wildly about what "1" means:
/// QUG is 24-decimal, SIGIL g2 is 10-decimal (base unit: glyph).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Currency {
    Qug,
    Sigil,
}

impl Currency {
    pub fn decimals(&self) -> u32 {
        match self {
            Currency::Qug => 24,
            Currency::Sigil => 10,
        }
    }

    pub fn symbol(&self) -> &'static str {
        match self {
            Currency::Qug => "QUG",
            Currency::Sigil => "SIGIL",
        }
    }

    /// Whole-token amount → base units.
    pub fn whole(&self, tokens: u64) -> u128 {
        (tokens as u128) * 10u128.pow(self.decimals())
    }

    /// Render a base-unit amount as a human string, trailing zeros trimmed.
    pub fn display(&self, base: u128) -> String {
        let scale = 10u128.pow(self.decimals());
        let int = base / scale;
        let frac = base % scale;
        if frac == 0 {
            return format!("{} {}", int, self.symbol());
        }
        let frac_str = format!("{:0width$}", frac, width = self.decimals() as usize);
        let trimmed = frac_str.trim_end_matches('0');
        format!("{}.{} {}", int, trimmed, self.symbol())
    }
}

/// Where a CM currently stands in the org.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CmStatus {
    /// Interested / vetted but not yet working.
    Candidate,
    /// Active and assignable.
    Active,
    /// Temporarily unavailable — kept on the roster, not assignable.
    OnLeave,
    /// No longer working with us — history retained.
    Alumni,
}

/// The human-capital record for one community manager.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CmProfile {
    pub id: String,
    /// Short handle used everywhere (timeline, reports).
    pub handle: String,
    pub display_name: String,
    /// Which chains this CM can serve.
    pub chains: Vec<Chain>,
    /// Free-form skill tags, lowercase ("discord", "danish", "memes", "onboarding").
    pub skills: Vec<String>,
    pub languages: Vec<String>,
    /// Quillon settlement address (qnk…), if they want QUG pay.
    pub wallet_qnk: Option<String>,
    /// SIGIL address, if they want SIGIL pay.
    pub wallet_sigil: Option<String>,
    /// Weekly availability they committed to.
    pub hours_per_week: u32,
    /// Hours currently locked into open assignments.
    pub committed_hours: u32,
    pub status: CmStatus,
    /// 0–1000. Everyone starts at 500; delivery moves it.
    pub reputation: u32,
    pub joined_ts_ms: u64,
    /// Lifetime payouts in base units, per currency.
    pub paid: BTreeMap<Currency, u128>,
}

impl CmProfile {
    pub fn free_hours(&self) -> u32 {
        self.hours_per_week.saturating_sub(self.committed_hours)
    }

    pub fn utilization_pct(&self) -> u32 {
        if self.hours_per_week == 0 {
            return 0;
        }
        (self.committed_hours * 100) / self.hours_per_week
    }
}

/// Lifecycle of a work item. The order is the contract:
/// terms are fixed at `Open`, payment only ever happens after `Approved`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkStatus {
    Open,
    Assigned,
    Submitted,
    Approved,
    Paid,
    Cancelled,
}

/// One unit of CM work with its terms.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkItem {
    pub id: String,
    pub title: String,
    pub brief: String,
    pub chain: Chain,
    pub skills_required: Vec<String>,
    /// Bounty in base units of `currency`, agreed BEFORE assignment.
    pub bounty_base: u128,
    pub currency: Currency,
    pub hours_estimate: u32,
    /// Unix ms. None = no deadline.
    pub deadline_ts_ms: Option<u64>,
    pub status: WorkStatus,
    pub assignee: Option<String>,
    pub created_ts_ms: u64,
    pub submitted_ts_ms: Option<u64>,
    pub approved_ts_ms: Option<u64>,
    /// Tx hash / reference once paid. A payment without a reference is a rumor.
    pub paid_tx: Option<String>,
}

/// Spec used to post new work (everything the terms need, nothing derived).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkSpec {
    pub title: String,
    pub brief: String,
    pub chain: Chain,
    pub skills_required: Vec<String>,
    pub bounty_base: u128,
    pub currency: Currency,
    pub hours_estimate: u32,
    pub deadline_ts_ms: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn currency_display_trims_and_scales() {
        // 1.5 QUG in 24-dp base units.
        let base = Currency::Qug.whole(1) + Currency::Qug.whole(1) / 2;
        assert_eq!(Currency::Qug.display(base), "1.5 QUG");
        // 300 QUG on the wire is 3e26 — the classic misread.
        assert_eq!(Currency::Qug.display(Currency::Qug.whole(300)), "300 QUG");
        // SIGIL g2: 10 decimals, base unit glyph.
        assert_eq!(Currency::Sigil.whole(1), 10_000_000_000);
        assert_eq!(Currency::Sigil.display(12_500_000_000), "1.25 SIGIL");
    }

    #[test]
    fn free_hours_saturate() {
        let cm = CmProfile {
            id: "cm-x".into(),
            handle: "x".into(),
            display_name: "X".into(),
            chains: vec![Chain::Quillon],
            skills: vec![],
            languages: vec![],
            wallet_qnk: None,
            wallet_sigil: None,
            hours_per_week: 10,
            committed_hours: 25,
            status: CmStatus::Active,
            reputation: 500,
            joined_ts_ms: 0,
            paid: BTreeMap::new(),
        };
        assert_eq!(cm.free_hours(), 0);
        assert_eq!(cm.utilization_pct(), 250);
    }
}
