//! The non-destructive migration: copy every record from a source `SwarmStore`
//! (the legacy [`JsonStore`](crate::JsonStore)) into a destination (the
//! [`FluxDbStore`](crate::FluxDbStore)) and PROVE the money ledger is preserved.
//!
//! Nothing is deleted here. The caller verifies [`ImportReport::ledger_ok`] is
//! true (completed count + Σ QUG match the source exactly) before archiving the
//! old JSON — per the balance-integrity discipline, the settlement ledger is
//! migrated and proven, never assumed.

use crate::{StoreError, SwarmStore};

/// Counts after import + the source/destination money-ledger comparison.
#[derive(Debug, Clone)]
pub struct ImportReport {
    pub agents: u64,
    pub claims: u64,
    pub completed: u64,
    pub completed_src: u64,
    pub sum_qug: f64,
    pub sum_qug_src: f64,
    pub messages: u64,
    pub activity: u64,
    pub files: u64,
    /// True iff completed count AND Σ QUG match the source exactly.
    pub ledger_ok: bool,
}

impl ImportReport {
    pub fn all_ok(&self) -> bool {
        self.ledger_ok
            && self.completed == self.completed_src
            && (self.sum_qug - self.sum_qug_src).abs() < 1e-9
    }
}

/// Copy src → dst. Order within each kind is preserved (records carry their own
/// timestamps; flux-db re-sorts by key).
pub fn import(src: &dyn SwarmStore, dst: &dyn SwarmStore) -> Result<ImportReport, StoreError> {
    for a in src.list_agents()? {
        dst.put_agent(&a)?;
    }
    for c in src.list_claims()? {
        dst.put_claim(&c)?;
    }
    for c in src.list_completed()? {
        dst.append_completed(&c)?;
    }
    for m in src.list_messages()? {
        dst.append_message(&m)?;
    }
    for a in src.list_activity()? {
        dst.append_activity(&a)?;
    }
    for f in src.list_file_claims()? {
        dst.put_file_claim(&f)?;
    }

    let completed = dst.completed_count()?;
    let completed_src = src.completed_count()?;
    let sum_qug = dst.sum_qug_earned()?;
    let sum_qug_src = src.sum_qug_earned()?;
    let ledger_ok = completed == completed_src && (sum_qug - sum_qug_src).abs() < 1e-9;

    Ok(ImportReport {
        agents: dst.list_agents()?.len() as u64,
        claims: dst.list_claims()?.len() as u64,
        completed,
        completed_src,
        sum_qug,
        sum_qug_src,
        messages: dst.message_count()?,
        activity: dst.activity_count()?,
        files: dst.list_file_claims()?.len() as u64,
        ledger_ok,
    })
}

/// Render a human-readable verify table comparing source and destination.
pub fn verify_table(src: &dyn SwarmStore, r: &ImportReport) -> Result<String, StoreError> {
    let row = |name: &str, s: u64, d: u64| {
        let mark = if s == d { "✓" } else { "✗ MISMATCH" };
        format!("  {name:<12} src {s:>6}   db {d:>6}   {mark}\n")
    };
    let mut out = String::from("flux-swarm-store migration — verify\n");
    out.push_str(&row("agents", src.list_agents()?.len() as u64, r.agents));
    out.push_str(&row("claims", src.list_claims()?.len() as u64, r.claims));
    out.push_str(&row("completed", r.completed_src, r.completed));
    out.push_str(&row("messages", src.message_count()?, r.messages));
    out.push_str(&row("activity", src.activity_count()?, r.activity));
    out.push_str(&row("files", src.list_file_claims()?.len() as u64, r.files));
    let qmark = if (r.sum_qug - r.sum_qug_src).abs() < 1e-9 { "✓" } else { "✗ MISMATCH" };
    out.push_str(&format!(
        "  {:<12} src {:>6.1}   db {:>6.1}   {}\n",
        "Σ QUG", r.sum_qug_src, r.sum_qug, qmark
    ));
    out.push_str(if r.all_ok() {
        "  → LEDGER PRESERVED — safe to archive the old JSON\n"
    } else {
        "  → ✗ DO NOT ARCHIVE — counts/Σ diverged\n"
    });
    Ok(out)
}
