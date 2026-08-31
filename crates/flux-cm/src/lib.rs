//! # flux-cm — Flux Community Manager
//!
//! Human-capital tooling for the community managers (CMs) of Quillon Graph
//! and SIGIL. Three ideas, kept deliberately simple:
//!
//! 1. **A CM is capital, not a contact.** The profile carries what they can
//!    do (skills, chains, languages), what they can give (hours/week), what
//!    they've earned (lifetime payouts per currency) and how reliably they
//!    deliver (reputation 0–1000, moved only by outcomes).
//! 2. **Work follows the agentic-money contract**: terms (bounty, hours,
//!    deadline) are fixed when the item is posted — BEFORE anyone is
//!    assigned — and payment is only recordable after approval, with a tx
//!    reference. Open → Assigned → Submitted → Approved → Paid.
//! 3. **Everything lands on one append-only timeline.** Per-CM or org-wide,
//!    the history is a query, never a reconstruction.
//!
//! In-memory + JSON persistence (`save`/`load`). No async, no network — this
//! is the ledger-shaped core that MCP tools / UIs wrap later.

pub mod capital;
pub mod model;
pub mod store;
pub mod timeline;

pub use capital::{build_report, CapitalReport, CmCapital};
pub use model::{Chain, CmProfile, CmStatus, Currency, WorkItem, WorkSpec, WorkStatus};
pub use timeline::{EventKind, Timeline, TimelineEvent};

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

/// Reputation constants — outcomes move the number, nothing else does.
pub const REP_START: u32 = 500;
pub const REP_MAX: u32 = 1000;
pub const REP_APPROVED_ON_TIME: u32 = 25;
pub const REP_APPROVED_LATE: u32 = 10;
pub const REP_DROPPED: u32 = 40;
pub const REP_KUDOS: u32 = 5;

#[derive(Debug, Error)]
pub enum CmError {
    #[error("unknown CM id: {0}")]
    UnknownCm(String),
    #[error("unknown work id: {0}")]
    UnknownWork(String),
    #[error("handle already taken: {0}")]
    HandleTaken(String),
    #[error("CM {0} is not Active (status: {1:?})")]
    NotActive(String, CmStatus),
    #[error("CM {cm} does not serve chain {chain}")]
    WrongChain { cm: String, chain: String },
    #[error("work {0} is {1:?}, expected {2:?}")]
    WrongState(String, WorkStatus, WorkStatus),
    #[error("work {0} has no assignee")]
    NoAssignee(String),
    #[error("bounty must be > 0")]
    ZeroBounty,
}

/// The whole community desk: roster + work board + timeline.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CommunityDesk {
    cms: BTreeMap<String, CmProfile>,
    work: BTreeMap<String, WorkItem>,
    timeline: Timeline,
    seq: u64,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl CommunityDesk {
    fn make_id(&mut self, prefix: &str, seed: &str) -> String {
        self.seq += 1;
        let h = blake3::hash(format!("{seed}|{}", self.seq).as_bytes());
        format!("{prefix}-{}", hex::encode(&h.as_bytes()[..4]))
    }

    // ── Roster ────────────────────────────────────────────────────────────

    /// Add a CM as Candidate. Handles are unique.
    #[allow(clippy::too_many_arguments)]
    pub fn onboard(
        &mut self,
        handle: &str,
        display_name: &str,
        chains: Vec<Chain>,
        skills: Vec<String>,
        languages: Vec<String>,
        wallet_qnk: Option<String>,
        wallet_sigil: Option<String>,
        hours_per_week: u32,
    ) -> Result<String, CmError> {
        if self.cms.values().any(|c| c.handle == handle) {
            return Err(CmError::HandleTaken(handle.to_string()));
        }
        let ts = now_ms();
        let id = self.make_id("cm", handle);
        let skills = skills.iter().map(|s| s.to_lowercase()).collect();
        self.cms.insert(
            id.clone(),
            CmProfile {
                id: id.clone(),
                handle: handle.to_string(),
                display_name: display_name.to_string(),
                chains,
                skills,
                languages,
                wallet_qnk,
                wallet_sigil,
                hours_per_week,
                committed_hours: 0,
                status: CmStatus::Candidate,
                reputation: REP_START,
                joined_ts_ms: ts,
                paid: BTreeMap::new(),
            },
        );
        self.timeline.record(
            ts,
            Some(&id),
            None,
            EventKind::Onboarded,
            format!("@{handle} joined the roster ({hours_per_week}h/week)"),
        );
        Ok(id)
    }

    pub fn set_status(&mut self, cm_id: &str, status: CmStatus) -> Result<(), CmError> {
        let cm = self
            .cms
            .get_mut(cm_id)
            .ok_or_else(|| CmError::UnknownCm(cm_id.to_string()))?;
        let old = cm.status;
        cm.status = status;
        let handle = cm.handle.clone();
        self.timeline.record(
            now_ms(),
            Some(cm_id),
            None,
            EventKind::StatusChanged,
            format!("@{handle}: {old:?} → {status:?}"),
        );
        Ok(())
    }

    /// Shorthand: Candidate → Active.
    pub fn activate(&mut self, cm_id: &str) -> Result<(), CmError> {
        self.set_status(cm_id, CmStatus::Active)
    }

    pub fn cm(&self, cm_id: &str) -> Option<&CmProfile> {
        self.cms.get(cm_id)
    }

    pub fn cm_by_handle(&self, handle: &str) -> Option<&CmProfile> {
        self.cms.values().find(|c| c.handle == handle)
    }

    pub fn roster(&self) -> impl Iterator<Item = &CmProfile> {
        self.cms.values()
    }

    // ── Work board ────────────────────────────────────────────────────────

    /// Post work with its full terms. This is the contract moment: bounty,
    /// hours and deadline are set here and never renegotiated mid-flight.
    pub fn post_work(&mut self, spec: WorkSpec) -> Result<String, CmError> {
        if spec.bounty_base == 0 {
            return Err(CmError::ZeroBounty);
        }
        let ts = now_ms();
        let id = self.make_id("wk", &spec.title);
        let detail = format!(
            "\"{}\" on {} · {} · est {}h",
            spec.title,
            spec.chain.name(),
            spec.currency.display(spec.bounty_base),
            spec.hours_estimate
        );
        self.work.insert(
            id.clone(),
            WorkItem {
                id: id.clone(),
                title: spec.title,
                brief: spec.brief,
                chain: spec.chain,
                skills_required: spec.skills_required.iter().map(|s| s.to_lowercase()).collect(),
                bounty_base: spec.bounty_base,
                currency: spec.currency,
                hours_estimate: spec.hours_estimate,
                deadline_ts_ms: spec.deadline_ts_ms,
                status: WorkStatus::Open,
                assignee: None,
                created_ts_ms: ts,
                submitted_ts_ms: None,
                approved_ts_ms: None,
                paid_tx: None,
            },
        );
        self.timeline
            .record(ts, None, Some(&id), EventKind::WorkPosted, detail);
        Ok(id)
    }

    pub fn work_item(&self, work_id: &str) -> Option<&WorkItem> {
        self.work.get(work_id)
    }

    pub fn board(&self) -> impl Iterator<Item = &WorkItem> {
        self.work.values()
    }

    /// Rank Active CMs for an Open work item.
    /// Score = 50% skill overlap + 30% reputation + 20% free capacity.
    /// CMs on the wrong chain or without the free hours are excluded.
    pub fn suggest(&self, work_id: &str) -> Result<Vec<(String, f64)>, CmError> {
        let w = self
            .work
            .get(work_id)
            .ok_or_else(|| CmError::UnknownWork(work_id.to_string()))?;
        let mut ranked: Vec<(String, f64)> = self
            .cms
            .values()
            .filter(|cm| {
                cm.status == CmStatus::Active
                    && cm.chains.contains(&w.chain)
                    && cm.free_hours() >= w.hours_estimate
            })
            .map(|cm| {
                let overlap = if w.skills_required.is_empty() {
                    1.0
                } else {
                    let hits = w
                        .skills_required
                        .iter()
                        .filter(|s| cm.skills.contains(s))
                        .count();
                    hits as f64 / w.skills_required.len() as f64
                };
                let rep = cm.reputation as f64 / REP_MAX as f64;
                let capacity = if cm.hours_per_week == 0 {
                    0.0
                } else {
                    cm.free_hours() as f64 / cm.hours_per_week as f64
                };
                (cm.id.clone(), 0.5 * overlap + 0.3 * rep + 0.2 * capacity)
            })
            .collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(ranked)
    }

    /// Assign Open work to an Active CM on the right chain. Locks their hours.
    pub fn assign(&mut self, work_id: &str, cm_id: &str) -> Result<(), CmError> {
        let w = self
            .work
            .get(work_id)
            .ok_or_else(|| CmError::UnknownWork(work_id.to_string()))?;
        if w.status != WorkStatus::Open {
            return Err(CmError::WrongState(w.id.clone(), w.status, WorkStatus::Open));
        }
        let chain = w.chain;
        let hours = w.hours_estimate;
        let title = w.title.clone();
        let cm = self
            .cms
            .get_mut(cm_id)
            .ok_or_else(|| CmError::UnknownCm(cm_id.to_string()))?;
        if cm.status != CmStatus::Active {
            return Err(CmError::NotActive(cm.id.clone(), cm.status));
        }
        if !cm.chains.contains(&chain) {
            return Err(CmError::WrongChain {
                cm: cm.id.clone(),
                chain: chain.name().to_string(),
            });
        }
        cm.committed_hours += hours;
        let handle = cm.handle.clone();
        let w = self.work.get_mut(work_id).unwrap();
        w.status = WorkStatus::Assigned;
        w.assignee = Some(cm_id.to_string());
        self.timeline.record(
            now_ms(),
            Some(cm_id),
            Some(work_id),
            EventKind::Assigned,
            format!("@{handle} took \"{title}\""),
        );
        Ok(())
    }

    /// The CM hands in the work.
    pub fn submit(&mut self, work_id: &str) -> Result<(), CmError> {
        let w = self
            .work
            .get_mut(work_id)
            .ok_or_else(|| CmError::UnknownWork(work_id.to_string()))?;
        if w.status != WorkStatus::Assigned {
            return Err(CmError::WrongState(w.id.clone(), w.status, WorkStatus::Assigned));
        }
        let cm_id = w.assignee.clone().ok_or_else(|| CmError::NoAssignee(w.id.clone()))?;
        let ts = now_ms();
        w.status = WorkStatus::Submitted;
        w.submitted_ts_ms = Some(ts);
        let title = w.title.clone();
        self.timeline.record(
            ts,
            Some(&cm_id),
            Some(work_id),
            EventKind::Submitted,
            format!("\"{title}\" submitted for review"),
        );
        Ok(())
    }

    /// Approve submitted work: frees the CM's hours, moves reputation by
    /// whether the SUBMISSION beat the deadline.
    pub fn approve(&mut self, work_id: &str) -> Result<(), CmError> {
        let w = self
            .work
            .get_mut(work_id)
            .ok_or_else(|| CmError::UnknownWork(work_id.to_string()))?;
        if w.status != WorkStatus::Submitted {
            return Err(CmError::WrongState(w.id.clone(), w.status, WorkStatus::Submitted));
        }
        let cm_id = w.assignee.clone().ok_or_else(|| CmError::NoAssignee(w.id.clone()))?;
        let ts = now_ms();
        w.status = WorkStatus::Approved;
        w.approved_ts_ms = Some(ts);
        let on_time = match (w.deadline_ts_ms, w.submitted_ts_ms) {
            (Some(deadline), Some(submitted)) => submitted <= deadline,
            _ => true,
        };
        let hours = w.hours_estimate;
        let title = w.title.clone();
        let cm = self.cms.get_mut(&cm_id).unwrap();
        cm.committed_hours = cm.committed_hours.saturating_sub(hours);
        let delta = if on_time { REP_APPROVED_ON_TIME } else { REP_APPROVED_LATE };
        cm.reputation = (cm.reputation + delta).min(REP_MAX);
        let handle = cm.handle.clone();
        self.timeline.record(
            ts,
            Some(&cm_id),
            Some(work_id),
            EventKind::Approved,
            format!(
                "\"{title}\" approved ({}) · @{handle} rep +{delta}",
                if on_time { "on time" } else { "late" }
            ),
        );
        Ok(())
    }

    /// Record the on-chain payment for approved work. `tx_ref` is required —
    /// a payment without a transaction reference is not a payment.
    pub fn record_payment(&mut self, work_id: &str, tx_ref: &str) -> Result<(), CmError> {
        let w = self
            .work
            .get_mut(work_id)
            .ok_or_else(|| CmError::UnknownWork(work_id.to_string()))?;
        if w.status != WorkStatus::Approved {
            return Err(CmError::WrongState(w.id.clone(), w.status, WorkStatus::Approved));
        }
        let cm_id = w.assignee.clone().ok_or_else(|| CmError::NoAssignee(w.id.clone()))?;
        w.status = WorkStatus::Paid;
        w.paid_tx = Some(tx_ref.to_string());
        let amount = w.bounty_base;
        let currency = w.currency;
        let title = w.title.clone();
        let cm = self.cms.get_mut(&cm_id).unwrap();
        *cm.paid.entry(currency).or_insert(0) += amount;
        let handle = cm.handle.clone();
        self.timeline.record(
            now_ms(),
            Some(&cm_id),
            Some(work_id),
            EventKind::Paid,
            format!(
                "{} to @{handle} for \"{title}\" (tx {tx_ref})",
                currency.display(amount)
            ),
        );
        Ok(())
    }

    /// Cancel Open or Assigned work. Dropping assigned work frees the CM's
    /// hours but costs reputation — commitments matter.
    pub fn cancel(&mut self, work_id: &str, reason: &str) -> Result<(), CmError> {
        let w = self
            .work
            .get_mut(work_id)
            .ok_or_else(|| CmError::UnknownWork(work_id.to_string()))?;
        if !matches!(w.status, WorkStatus::Open | WorkStatus::Assigned) {
            return Err(CmError::WrongState(w.id.clone(), w.status, WorkStatus::Open));
        }
        let was_assigned = w.status == WorkStatus::Assigned;
        let cm_id = w.assignee.clone();
        let hours = w.hours_estimate;
        let title = w.title.clone();
        w.status = WorkStatus::Cancelled;
        if let (true, Some(cm_id)) = (was_assigned, cm_id.as_deref()) {
            if let Some(cm) = self.cms.get_mut(cm_id) {
                cm.committed_hours = cm.committed_hours.saturating_sub(hours);
                cm.reputation = cm.reputation.saturating_sub(REP_DROPPED);
            }
        }
        self.timeline.record(
            now_ms(),
            cm_id.as_deref(),
            Some(work_id),
            EventKind::Cancelled,
            format!("\"{title}\" cancelled: {reason}"),
        );
        Ok(())
    }

    // ── Notes, kudos, reporting ───────────────────────────────────────────

    pub fn note(&mut self, cm_id: &str, text: &str) -> Result<(), CmError> {
        if !self.cms.contains_key(cm_id) {
            return Err(CmError::UnknownCm(cm_id.to_string()));
        }
        self.timeline
            .record(now_ms(), Some(cm_id), None, EventKind::Note, text.to_string());
        Ok(())
    }

    /// Public praise; small reputation bump.
    pub fn kudos(&mut self, cm_id: &str, text: &str) -> Result<(), CmError> {
        let cm = self
            .cms
            .get_mut(cm_id)
            .ok_or_else(|| CmError::UnknownCm(cm_id.to_string()))?;
        cm.reputation = (cm.reputation + REP_KUDOS).min(REP_MAX);
        self.timeline
            .record(now_ms(), Some(cm_id), None, EventKind::Kudos, text.to_string());
        Ok(())
    }

    pub fn timeline(&self) -> &Timeline {
        &self.timeline
    }

    /// Markdown timeline for one CM.
    pub fn cm_timeline_markdown(&self, cm_id: &str) -> String {
        Timeline::render_markdown(&self.timeline.for_cm(cm_id))
    }

    /// The human-capital report over the whole roster.
    pub fn capital_report(&self) -> CapitalReport {
        let work: Vec<&WorkItem> = self.work.values().collect();
        build_report(now_ms(), self.cms.values(), &work)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desk_with_two_cms() -> (CommunityDesk, String, String) {
        let mut desk = CommunityDesk::default();
        let mira = desk
            .onboard(
                "mira",
                "Mira K",
                vec![Chain::Quillon, Chain::Sigil],
                vec!["Discord".into(), "onboarding".into()],
                vec!["english".into(), "danish".into()],
                Some("qnk_mira".into()),
                Some("sigil_mira".into()),
                12,
            )
            .unwrap();
        let ole = desk
            .onboard(
                "ole",
                "Ole B",
                vec![Chain::Quillon],
                vec!["memes".into()],
                vec!["danish".into()],
                Some("qnk_ole".into()),
                None,
                6,
            )
            .unwrap();
        desk.activate(&mira).unwrap();
        desk.activate(&ole).unwrap();
        (desk, mira, ole)
    }

    #[test]
    fn full_lifecycle_lands_on_timeline_and_pays() {
        let (mut desk, mira, _ole) = desk_with_two_cms();
        let wk = desk
            .post_work(WorkSpec {
                title: "SIGIL launch AMA".into(),
                brief: "Host + recap the g2 launch AMA".into(),
                chain: Chain::Sigil,
                skills_required: vec!["discord".into()],
                bounty_base: Currency::Sigil.whole(120),
                currency: Currency::Sigil,
                hours_estimate: 4,
                deadline_ts_ms: Some(now_ms() + 86_400_000),
            })
            .unwrap();

        desk.assign(&wk, &mira).unwrap();
        assert_eq!(desk.cm(&mira).unwrap().committed_hours, 4);
        desk.submit(&wk).unwrap();
        desk.approve(&wk).unwrap();
        let cm = desk.cm(&mira).unwrap();
        assert_eq!(cm.committed_hours, 0);
        assert_eq!(cm.reputation, REP_START + REP_APPROVED_ON_TIME);

        desk.record_payment(&wk, "0xsigil_tx_abc").unwrap();
        let cm = desk.cm(&mira).unwrap();
        assert_eq!(cm.paid[&Currency::Sigil], Currency::Sigil.whole(120));
        assert_eq!(desk.work_item(&wk).unwrap().status, WorkStatus::Paid);

        // Timeline carries the whole story: onboard, activate, assign,
        // submit, approve, paid = 6 events for mira.
        let events = desk.timeline().for_cm(&mira);
        assert_eq!(events.len(), 6);
        let md = desk.cm_timeline_markdown(&mira);
        assert!(md.contains("💰 Paid · 120 SIGIL to @mira"));
        // Payment can't be recorded twice.
        assert!(desk.record_payment(&wk, "0xagain").is_err());
    }

    #[test]
    fn late_submission_earns_less_reputation() {
        let (mut desk, mira, _) = desk_with_two_cms();
        let wk = desk
            .post_work(WorkSpec {
                title: "Overdue digest".into(),
                brief: "".into(),
                chain: Chain::Quillon,
                skills_required: vec![],
                bounty_base: Currency::Qug.whole(10),
                currency: Currency::Qug,
                hours_estimate: 1,
                deadline_ts_ms: Some(1), // deadline in 1970 — everything is late
            })
            .unwrap();
        desk.assign(&wk, &mira).unwrap();
        desk.submit(&wk).unwrap();
        desk.approve(&wk).unwrap();
        assert_eq!(desk.cm(&mira).unwrap().reputation, REP_START + REP_APPROVED_LATE);
    }

    #[test]
    fn suggest_ranks_by_skill_chain_and_capacity() {
        let (desk, mira, ole) = {
            let (mut desk, mira, ole) = desk_with_two_cms();
            let _ = desk
                .post_work(WorkSpec {
                    title: "Discord onboarding revamp".into(),
                    brief: "".into(),
                    chain: Chain::Quillon,
                    skills_required: vec!["discord".into(), "onboarding".into()],
                    bounty_base: Currency::Qug.whole(40),
                    currency: Currency::Qug,
                    hours_estimate: 5,
                    deadline_ts_ms: None,
                })
                .unwrap();
            (desk, mira, ole)
        };
        let wk_id = desk.board().next().unwrap().id.clone();
        let ranked = desk.suggest(&wk_id).unwrap();
        // Mira matches both skills; Ole matches none but is still eligible.
        assert_eq!(ranked[0].0, mira);
        assert!(ranked[0].1 > ranked[1].1);
        assert_eq!(ranked[1].0, ole);

        // A SIGIL-only item excludes Ole (wrong chain).
        let mut desk2 = CommunityDesk::default();
        let solo = desk2
            .onboard("solo", "S", vec![Chain::Quillon], vec![], vec![], None, None, 2)
            .unwrap();
        desk2.activate(&solo).unwrap();
        let wk2 = desk2
            .post_work(WorkSpec {
                title: "Big task".into(),
                brief: "".into(),
                chain: Chain::Quillon,
                skills_required: vec![],
                bounty_base: Currency::Qug.whole(1),
                currency: Currency::Qug,
                hours_estimate: 10, // more than solo's 2h/week
                deadline_ts_ms: None,
            })
            .unwrap();
        assert!(desk2.suggest(&wk2).unwrap().is_empty());
    }

    #[test]
    fn dropping_assigned_work_costs_reputation_and_frees_hours() {
        let (mut desk, mira, _) = desk_with_two_cms();
        let wk = desk
            .post_work(WorkSpec {
                title: "Abandoned thread".into(),
                brief: "".into(),
                chain: Chain::Quillon,
                skills_required: vec![],
                bounty_base: Currency::Qug.whole(5),
                currency: Currency::Qug,
                hours_estimate: 3,
                deadline_ts_ms: None,
            })
            .unwrap();
        desk.assign(&wk, &mira).unwrap();
        desk.cancel(&wk, "CM unavailable").unwrap();
        let cm = desk.cm(&mira).unwrap();
        assert_eq!(cm.committed_hours, 0);
        assert_eq!(cm.reputation, REP_START - REP_DROPPED);
    }

    #[test]
    fn guardrails_hold() {
        let (mut desk, mira, _) = desk_with_two_cms();
        // Duplicate handle refused.
        assert!(matches!(
            desk.onboard("mira", "Imposter", vec![], vec![], vec![], None, None, 1),
            Err(CmError::HandleTaken(_))
        ));
        // Zero bounty refused — terms must be real.
        assert!(matches!(
            desk.post_work(WorkSpec {
                title: "Free work".into(),
                brief: "".into(),
                chain: Chain::Quillon,
                skills_required: vec![],
                bounty_base: 0,
                currency: Currency::Qug,
                hours_estimate: 1,
                deadline_ts_ms: None,
            }),
            Err(CmError::ZeroBounty)
        ));
        // OnLeave CM can't be assigned.
        desk.set_status(&mira, CmStatus::OnLeave).unwrap();
        let wk = desk
            .post_work(WorkSpec {
                title: "T".into(),
                brief: "".into(),
                chain: Chain::Quillon,
                skills_required: vec![],
                bounty_base: 1,
                currency: Currency::Qug,
                hours_estimate: 1,
                deadline_ts_ms: None,
            })
            .unwrap();
        assert!(matches!(desk.assign(&wk, &mira), Err(CmError::NotActive(_, _))));
    }

    #[test]
    fn capital_report_renders_roster() {
        let (mut desk, mira, _) = desk_with_two_cms();
        desk.kudos(&mira, "great AMA energy").unwrap();
        let report = desk.capital_report();
        assert_eq!(report.headcount_active, 2);
        assert_eq!(report.headcount_total, 2);
        assert_eq!(report.free_hours_active, 18); // 12 + 6, nothing committed
        // Kudos put mira on top.
        assert_eq!(report.rows[0].handle, "mira");
        let md = report.render_markdown();
        assert!(md.contains("| @mira |"));
        assert!(md.contains("2 active / 2 total"));
    }
}
