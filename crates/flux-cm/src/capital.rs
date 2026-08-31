//! Human-capital analytics: who has capacity, who is overloaded, who has
//! actually been paid what. A roster you can read in ten seconds.

use crate::model::{CmProfile, CmStatus, WorkItem, WorkStatus};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CmCapital {
    pub id: String,
    pub handle: String,
    pub status: CmStatus,
    pub reputation: u32,
    pub hours_per_week: u32,
    pub committed_hours: u32,
    pub utilization_pct: u32,
    pub open_assignments: usize,
    pub delivered: usize,
    /// Human-readable lifetime payouts, e.g. ["950 QUG", "120 SIGIL"].
    pub paid_totals: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapitalReport {
    pub generated_ts_ms: u64,
    pub headcount_active: usize,
    pub headcount_total: usize,
    /// Sum of Active CMs' free hours — the org's spare capacity this week.
    pub free_hours_active: u32,
    pub rows: Vec<CmCapital>,
}

pub fn build_report<'a>(
    now_ms: u64,
    cms: impl Iterator<Item = &'a CmProfile>,
    work: &[&WorkItem],
) -> CapitalReport {
    let mut rows: Vec<CmCapital> = cms
        .map(|cm| {
            let open_assignments = work
                .iter()
                .filter(|w| {
                    w.assignee.as_deref() == Some(&cm.id)
                        && matches!(w.status, WorkStatus::Assigned | WorkStatus::Submitted)
                })
                .count();
            let delivered = work
                .iter()
                .filter(|w| {
                    w.assignee.as_deref() == Some(&cm.id)
                        && matches!(w.status, WorkStatus::Approved | WorkStatus::Paid)
                })
                .count();
            CmCapital {
                id: cm.id.clone(),
                handle: cm.handle.clone(),
                status: cm.status,
                reputation: cm.reputation,
                hours_per_week: cm.hours_per_week,
                committed_hours: cm.committed_hours,
                utilization_pct: cm.utilization_pct(),
                open_assignments,
                delivered,
                paid_totals: cm
                    .paid
                    .iter()
                    .filter(|(_, base)| **base > 0)
                    .map(|(cur, base)| cur.display(*base))
                    .collect(),
            }
        })
        .collect();
    // Most reputable first — the report doubles as a leaderboard.
    rows.sort_by(|a, b| b.reputation.cmp(&a.reputation));

    let headcount_total = rows.len();
    let headcount_active = rows.iter().filter(|r| r.status == CmStatus::Active).count();
    let free_hours_active = rows
        .iter()
        .filter(|r| r.status == CmStatus::Active)
        .map(|r| r.hours_per_week.saturating_sub(r.committed_hours))
        .sum();

    CapitalReport {
        generated_ts_ms: now_ms,
        headcount_active,
        headcount_total,
        free_hours_active,
        rows,
    }
}

impl CapitalReport {
    pub fn render_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "## Community roster — {} active / {} total · {}h free capacity\n\n",
            self.headcount_active, self.headcount_total, self.free_hours_active
        ));
        out.push_str("| CM | status | rep | load | assignments | delivered | lifetime paid |\n");
        out.push_str("|---|---|---|---|---|---|---|\n");
        for r in &self.rows {
            out.push_str(&format!(
                "| @{} | {:?} | {} | {}/{}h ({}%) | {} | {} | {} |\n",
                r.handle,
                r.status,
                r.reputation,
                r.committed_hours,
                r.hours_per_week,
                r.utilization_pct,
                r.open_assignments,
                r.delivered,
                if r.paid_totals.is_empty() {
                    "—".to_string()
                } else {
                    r.paid_totals.join(" + ")
                }
            ));
        }
        out
    }
}
