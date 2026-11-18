//! control/prince2.rs — PRINCE2 Agile control for a roadmap (Viktor 2026-11-15: "go for prince2").
//!
//! [`super`] (control.rs) already lets the operator approve / veto / reorder refactor tasks. A roadmap
//! needs the part PRINCE2 is good at and flux lacked: the 2026-06-24 roadmap in
//! `FLUX-MASTER-OVERVIEW.md` §8 was never re-checked, so five months later nobody could say which items
//! still mattered. This module adds exactly four PRINCE2 ideas and nothing heavier:
//!
//! | PRINCE2 principle | here |
//! |---|---|
//! | manage by stages | [`Stage`]; [`Board::end_stage`] refuses while an exception is undecided |
//! | manage by exception | [`Tolerances`] (board > stage > item); [`Board::check`] turns a forecast that breaks one into an [`ExceptionReport`] |
//! | continued business justification | every [`Item`] carries `benefit` + `justified_on`; [`Board::stale`] lists the ones not re-checked in N days |
//! | product-based planning | every item names its `done_check` (one command that proves it runs); the report flags items without one |
//! | learn from experience | [`Lesson`] log, shown in the highlight report |
//!
//! The board (the person who decides) is the operator. [`Board::decide`] RECORDS the operator's word;
//! an agent never calls it on its own, the same rule as `approve` in control.rs. Delivery inside a
//! stage stays lean/agile (lanes, small batches, measure first): PRINCE2 governs, it does not plan days.
//!
//! Pure + serde-persisted, no new dependencies. Dates are plain `YYYY-MM-DD` strings (no clock is read,
//! the caller passes `today`), so every function is deterministic and testable.

use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;

pub const BOARD_VERSION: u32 = 1;
/// A benefit not re-checked for this many days is stale (PRINCE2 checks it at every stage end).
pub const DEFAULT_STALE_DAYS: i64 = 30;

/// The limits inside which work goes on without asking the board ("manage by exception").
/// Defaults = what Viktor already rules on this box: a working day per item, the 12 GB build floor,
/// any money move and any production touch go to him.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Tolerances {
    /// working hours an item may take
    pub max_hours: f64,
    /// memory the box must keep AVAILABLE during the item's builds (GB). 12 general, 24 for a Quillon release.
    pub min_free_mem_gb: f64,
    /// files the item may touch (scope)
    pub max_files: u32,
    /// money the item may move without asking; 0 = every money move is an exception
    pub max_money_qug: f64,
    /// may the item deploy / restart / write live data? false = every production touch is an exception
    pub production_allowed: bool,
}

impl Default for Tolerances {
    fn default() -> Self {
        Self { max_hours: 8.0, min_free_mem_gb: 12.0, max_files: 20, max_money_qug: 0.0, production_allowed: false }
    }
}

/// What the worker expects an item to need, checked against its tolerances before (and during) work.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Forecast {
    pub hours: f64,
    /// the LOWEST available memory expected while it builds; `None` = no build
    pub min_free_mem_gb: Option<f64>,
    pub files: u32,
    pub money_qug: f64,
    pub touches_production: bool,
}

/// One tolerance a forecast breaks, in words the board can read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Breach {
    pub tolerance: String,
    pub limit: String,
    pub forecast: String,
}

/// Every tolerance `f` breaks under `t`. Empty = carry on without asking.
pub fn breaches(t: &Tolerances, f: &Forecast) -> Vec<Breach> {
    let mut v = Vec::new();
    let mut push = |tol: &str, limit: String, fc: String| v.push(Breach { tolerance: tol.into(), limit, forecast: fc });
    if f.hours > t.max_hours {
        push("time", format!("≤ {} h", t.max_hours), format!("{} h", f.hours));
    }
    if let Some(m) = f.min_free_mem_gb {
        if m < t.min_free_mem_gb {
            push("memory", format!("≥ {} GB available", t.min_free_mem_gb), format!("{m} GB available"));
        }
    }
    if f.files > t.max_files {
        push("scope", format!("≤ {} files", t.max_files), format!("{} files", f.files));
    }
    if f.money_qug > t.max_money_qug {
        push("money", format!("≤ {} QUG", t.max_money_qug), format!("{} QUG", f.money_qug));
    }
    if f.touches_production && !t.production_allowed {
        push("production", "no production touch".into(), "touches production".into());
    }
    v
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ItemStatus {
    #[default]
    Planned,
    Active,
    /// a tolerance is forecast to break; waits for the board
    Exception,
    Done,
    Dropped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum StageStatus {
    #[default]
    Open,
    Closed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stage {
    pub name: String,
    pub goal: String,
    #[serde(default)]
    pub status: StageStatus,
    /// overrides the board's tolerances for items in this stage
    #[serde(default)]
    pub tolerances: Option<Tolerances>,
    #[serde(default)]
    pub closed_on: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub stage: String,
    /// business justification: why this matters. An item without one is refused.
    pub benefit: String,
    /// last day the benefit was checked (YYYY-MM-DD)
    pub justified_on: String,
    /// one command that proves the item runs ("done = runs live, checkable with one command"); empty = not defined yet
    #[serde(default)]
    pub done_check: String,
    #[serde(default)]
    pub status: ItemStatus,
    #[serde(default)]
    pub tolerances: Option<Tolerances>,
    #[serde(default)]
    pub notes: Vec<String>,
}

/// What the board decides about an exception.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    /// accept the breach, work goes on
    Continue,
    /// back to planning: the worker brings a forecast inside tolerance
    Replan,
    /// stop: the benefit is not worth it
    Drop,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExceptionReport {
    pub seq: u64,
    pub item_id: String,
    pub raised_on: String,
    pub breaches: Vec<Breach>,
    pub forecast: Forecast,
    #[serde(default)]
    pub decision: Option<Decision>,
    #[serde(default)]
    pub decision_note: String,
    #[serde(default)]
    pub decided_on: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lesson {
    pub on: String,
    pub text: String,
}

/// What a closed stage hands to the board.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StageReport {
    pub stage: String,
    pub done: Vec<String>,
    pub carried: Vec<String>,
    pub dropped: Vec<String>,
    pub stale_justification: Vec<String>,
    pub without_done_check: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Board {
    pub version: u32,
    pub project: String,
    /// who decides (the PRINCE2 executive); the operator
    pub executive: String,
    #[serde(default)]
    pub tolerances: Tolerances,
    #[serde(default)]
    pub stages: Vec<Stage>,
    #[serde(default)]
    pub items: Vec<Item>,
    #[serde(default)]
    pub exceptions: Vec<ExceptionReport>,
    #[serde(default)]
    pub lessons: Vec<Lesson>,
    #[serde(default)]
    pub log: Vec<String>,
}

impl Board {
    pub fn new(project: impl Into<String>, executive: impl Into<String>) -> Self {
        Board {
            version: BOARD_VERSION,
            project: project.into(),
            executive: executive.into(),
            tolerances: Tolerances::default(),
            stages: Vec::new(),
            items: Vec::new(),
            exceptions: Vec::new(),
            lessons: Vec::new(),
            log: Vec::new(),
        }
    }

    pub fn add_stage(&mut self, name: &str, goal: &str) -> Result<(), String> {
        if self.stages.iter().any(|s| s.name == name) {
            return Err(format!("stage {name} already exists"));
        }
        self.stages.push(Stage { name: name.into(), goal: goal.into(), status: StageStatus::Open, tolerances: None, closed_on: None });
        self.log.push(format!("stage {name} opened"));
        Ok(())
    }

    /// PRINCE2: no business justification, no item.
    pub fn add_item(&mut self, item: Item) -> Result<(), String> {
        if item.benefit.trim().is_empty() {
            return Err(format!("{}: refused — no benefit stated (continued business justification)", item.id));
        }
        if days_from_ymd(&item.justified_on).is_none() {
            return Err(format!("{}: justified_on {:?} is not a YYYY-MM-DD date", item.id, item.justified_on));
        }
        if self.items.iter().any(|i| i.id == item.id) {
            return Err(format!("{}: duplicate item id", item.id));
        }
        match self.stages.iter().find(|s| s.name == item.stage) {
            None => return Err(format!("{}: unknown stage {}", item.id, item.stage)),
            Some(s) if s.status == StageStatus::Closed => return Err(format!("{}: stage {} is closed", item.id, item.stage)),
            Some(_) => {}
        }
        self.log.push(format!("item {} added to {}", item.id, item.stage));
        self.items.push(item);
        Ok(())
    }

    fn item_mut(&mut self, id: &str) -> Result<&mut Item, String> {
        self.items.iter_mut().find(|i| i.id == id).ok_or_else(|| format!("unknown item {id}"))
    }

    /// Item tolerances win over the stage's, the stage's over the board's.
    pub fn effective_tolerances(&self, item_id: &str) -> Option<Tolerances> {
        let item = self.items.iter().find(|i| i.id == item_id)?;
        let stage = self.stages.iter().find(|s| s.name == item.stage).and_then(|s| s.tolerances);
        Some(item.tolerances.or(stage).unwrap_or(self.tolerances))
    }

    /// Check a forecast. Inside tolerance → the item becomes Active (if it was Planned) and nothing is raised.
    /// A breach → the item goes to Exception and an [`ExceptionReport`] waits for the board.
    pub fn check(&mut self, item_id: &str, forecast: Forecast, today: &str) -> Result<Vec<Breach>, String> {
        let tol = self.effective_tolerances(item_id).ok_or_else(|| format!("unknown item {item_id}"))?;
        let found = breaches(&tol, &forecast);
        let seq = self.exceptions.len() as u64 + 1;
        let item = self.item_mut(item_id)?;
        if matches!(item.status, ItemStatus::Done | ItemStatus::Dropped) {
            return Err(format!("{item_id} is {:?}; nothing to check", item.status));
        }
        if found.is_empty() {
            if item.status == ItemStatus::Planned {
                item.status = ItemStatus::Active;
            }
            self.log.push(format!("{today} check {item_id}: inside tolerance"));
        } else {
            item.status = ItemStatus::Exception;
            let names: Vec<&str> = found.iter().map(|b| b.tolerance.as_str()).collect();
            self.log.push(format!("{today} check {item_id}: EXCEPTION #{seq} ({})", names.join(", ")));
            self.exceptions.push(ExceptionReport {
                seq,
                item_id: item_id.into(),
                raised_on: today.into(),
                breaches: found.clone(),
                forecast,
                decision: None,
                decision_note: String::new(),
                decided_on: None,
            });
        }
        Ok(found)
    }

    /// Record the BOARD's decision on an exception. Agents never call this on their own.
    pub fn decide(&mut self, seq: u64, decision: Decision, note: &str, today: &str) -> Result<(), String> {
        let ex = self.exceptions.iter_mut().find(|e| e.seq == seq).ok_or_else(|| format!("no exception #{seq}"))?;
        if ex.decision.is_some() {
            return Err(format!("exception #{seq} is already decided"));
        }
        ex.decision = Some(decision);
        ex.decision_note = note.into();
        ex.decided_on = Some(today.into());
        let id = ex.item_id.clone();
        let item = self.item_mut(&id)?;
        item.status = match decision {
            Decision::Continue => ItemStatus::Active,
            Decision::Replan => ItemStatus::Planned,
            Decision::Drop => ItemStatus::Dropped,
        };
        self.log.push(format!("{today} board decided #{seq} ({id}): {decision:?} — {note}"));
        Ok(())
    }

    pub fn mark_done(&mut self, item_id: &str, today: &str) -> Result<(), String> {
        let item = self.item_mut(item_id)?;
        if item.status == ItemStatus::Exception {
            return Err(format!("{item_id} has an undecided exception"));
        }
        item.status = ItemStatus::Done;
        self.log.push(format!("{today} {item_id} done"));
        Ok(())
    }

    /// The benefit was looked at again today (optionally restated).
    pub fn rejustify(&mut self, item_id: &str, today: &str, benefit: Option<&str>) -> Result<(), String> {
        if days_from_ymd(today).is_none() {
            return Err(format!("today {today:?} is not a YYYY-MM-DD date"));
        }
        let item = self.item_mut(item_id)?;
        if let Some(b) = benefit {
            if b.trim().is_empty() {
                return Err("an empty benefit is not a justification".into());
            }
            item.benefit = b.into();
        }
        item.justified_on = today.into();
        self.log.push(format!("{today} {item_id} re-justified"));
        Ok(())
    }

    pub fn add_lesson(&mut self, today: &str, text: &str) {
        self.lessons.push(Lesson { on: today.into(), text: text.into() });
    }

    /// Live items (not done, not dropped) whose benefit was last checked more than `max_age_days` ago,
    /// or whose date cannot be read.
    pub fn stale(&self, today: &str, max_age_days: i64) -> Vec<&Item> {
        let now = days_from_ymd(today);
        self.items
            .iter()
            .filter(|i| !matches!(i.status, ItemStatus::Done | ItemStatus::Dropped))
            .filter(|i| match (now, days_from_ymd(&i.justified_on)) {
                (Some(n), Some(d)) => n - d > max_age_days,
                _ => true,
            })
            .collect()
    }

    fn open_exceptions(&self) -> impl Iterator<Item = &ExceptionReport> {
        self.exceptions.iter().filter(|e| e.decision.is_none())
    }

    /// Close a stage at its gate. Refused while any of its items has an undecided exception.
    pub fn end_stage(&mut self, stage: &str, today: &str) -> Result<StageReport, String> {
        let st = self.stages.iter().find(|s| s.name == stage).ok_or_else(|| format!("unknown stage {stage}"))?;
        if st.status == StageStatus::Closed {
            return Err(format!("stage {stage} is already closed"));
        }
        let in_stage = |id: &str| self.items.iter().any(|i| i.id == id && i.stage == stage);
        let blocking: Vec<String> = self.open_exceptions().filter(|e| in_stage(&e.item_id)).map(|e| format!("#{} {}", e.seq, e.item_id)).collect();
        if !blocking.is_empty() {
            return Err(format!("stage {stage} cannot close: undecided exception(s) {}", blocking.join(", ")));
        }
        let stale: Vec<String> = self.stale(today, DEFAULT_STALE_DAYS).iter().filter(|i| i.stage == stage).map(|i| i.id.clone()).collect();
        let pick = |want: &dyn Fn(&Item) -> bool| -> Vec<String> {
            self.items.iter().filter(|i| i.stage == stage && want(i)).map(|i| i.id.clone()).collect()
        };
        let report = StageReport {
            stage: stage.into(),
            done: pick(&|i: &Item| i.status == ItemStatus::Done),
            carried: pick(&|i: &Item| matches!(i.status, ItemStatus::Planned | ItemStatus::Active)),
            dropped: pick(&|i: &Item| i.status == ItemStatus::Dropped),
            stale_justification: stale,
            without_done_check: pick(&|i: &Item| i.status != ItemStatus::Dropped && i.done_check.trim().is_empty()),
        };
        let st = self.stages.iter_mut().find(|s| s.name == stage).expect("checked above");
        st.status = StageStatus::Closed;
        st.closed_on = Some(today.into());
        self.log.push(format!("{today} stage {stage} closed: {} done, {} carried", report.done.len(), report.carried.len()));
        Ok(report)
    }

    /// The PRINCE2 highlight report: one screen, in plain words.
    pub fn highlight(&self, today: &str) -> String {
        let mut o = String::new();
        o.push_str(&format!("HIGHLIGHT REPORT — {} · {} · board: {}\n", self.project, today, self.executive));
        let t = &self.tolerances;
        o.push_str(&format!(
            "tolerances (board default): ≤ {} h · ≥ {} GB free · ≤ {} files · money ≤ {} QUG · production {}\n",
            t.max_hours, t.min_free_mem_gb, t.max_files, t.max_money_qug, if t.production_allowed { "allowed" } else { "asks the board" }
        ));
        for s in &self.stages {
            let items: Vec<&Item> = self.items.iter().filter(|i| i.stage == s.name).collect();
            let n = |st: ItemStatus| items.iter().filter(|i| i.status == st).count();
            o.push_str(&format!(
                "\nstage {} [{}] — {}\n  {} done · {} active · {} planned · {} exception · {} dropped\n",
                s.name,
                if s.status == StageStatus::Open { "open" } else { "closed" },
                s.goal,
                n(ItemStatus::Done),
                n(ItemStatus::Active),
                n(ItemStatus::Planned),
                n(ItemStatus::Exception),
                n(ItemStatus::Dropped)
            ));
        }
        let open: Vec<&ExceptionReport> = self.open_exceptions().collect();
        o.push_str(&format!("\nexceptions waiting for the board: {}\n", open.len()));
        for e in &open {
            let why: Vec<String> = e.breaches.iter().map(|b| format!("{}: {} vs {}", b.tolerance, b.forecast, b.limit)).collect();
            o.push_str(&format!("  #{} {} (raised {}): {}\n", e.seq, e.item_id, e.raised_on, why.join("; ")));
        }
        let stale = self.stale(today, DEFAULT_STALE_DAYS);
        o.push_str(&format!("\nbenefit not re-checked in {DEFAULT_STALE_DAYS} days: {}\n", stale.len()));
        for i in &stale {
            o.push_str(&format!("  {} — last checked {} — {}\n", i.id, i.justified_on, i.title));
        }
        let no_check: Vec<&Item> = self.items.iter().filter(|i| !matches!(i.status, ItemStatus::Dropped) && i.done_check.trim().is_empty()).collect();
        o.push_str(&format!("\nno done-check (cannot be proven done with one command): {}\n", no_check.len()));
        for i in &no_check {
            o.push_str(&format!("  {} — {}\n", i.id, i.title));
        }
        if !self.lessons.is_empty() {
            o.push_str("\nlatest lessons:\n");
            for l in self.lessons.iter().rev().take(3) {
                o.push_str(&format!("  {} — {}\n", l.on, l.text));
            }
        }
        o
    }

    /// Persist atomically (tmp-file + rename), like control.rs.
    pub fn save(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let path = path.as_ref();
        let json = serde_json::to_string_pretty(self).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)
    }

    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        let json = std::fs::read_to_string(path)?;
        serde_json::from_str(&json).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

/// One roadmap row read from a markdown table `| **N** | Step | What | Why | Size |`.
#[derive(Debug, Clone, PartialEq)]
pub struct OverviewRow {
    pub number: String,
    pub step: String,
    pub why: String,
    pub done: bool,
}

/// Read the numbered roadmap rows of FLUX-MASTER-OVERVIEW.md §8 (or any table of that shape) so an old
/// roadmap can be put in front of the board as items whose benefit must be re-checked.
pub fn parse_overview_rows(md: &str) -> Vec<OverviewRow> {
    let strip = |s: &str| s.replace("**", "").trim().to_string();
    md.lines()
        .filter(|l| l.starts_with("| **") && l.trim_end().ends_with('|'))
        .filter_map(|l| {
            let cells: Vec<&str> = l.trim().trim_matches('|').split('|').collect();
            if cells.len() < 5 {
                return None;
            }
            let number = strip(cells[0]);
            if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let raw_step = strip(cells[1]);
            let done = raw_step.contains('✅') || raw_step.trim_start().starts_with("DONE");
            // "✅ DONE (2026-06-24) — Fix the Phase-1 build" → "Fix the Phase-1 build"; a title that only
            // CONTAINS a dash ("Decide frontend posture — own it OR …") keeps it.
            let step = match (done, raw_step.split_once(" — ")) {
                (true, Some((_, rest))) => rest.trim().to_string(),
                _ => raw_step.clone(),
            };
            // Why = everything between "What" and "Size" (a cell may itself contain a '|')
            let why = strip(&cells[3..cells.len() - 1].join("|"));
            Some(OverviewRow { number, step, why, done })
        })
        .collect()
}

/// Days since 1970-01-01 for a `YYYY-MM-DD` string (proleptic Gregorian), `None` if it is not one.
pub fn days_from_ymd(s: &str) -> Option<i64> {
    let p: Vec<&str> = s.trim().split('-').collect();
    if p.len() != 3 || p[0].len() != 4 || p[1].len() != 2 || p[2].len() != 2 {
        return None;
    }
    let (y, m, d): (i64, i64, i64) = (p[0].parse().ok()?, p[1].parse().ok()?, p[2].parse().ok()?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // Howard Hinnant's days_from_civil
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, stage: &str, on: &str) -> Item {
        Item {
            id: id.into(),
            title: format!("title {id}"),
            stage: stage.into(),
            benefit: "users get X".into(),
            justified_on: on.into(),
            done_check: "flux-roadmap show".into(),
            status: ItemStatus::Planned,
            tolerances: None,
            notes: vec![],
        }
    }

    fn board() -> Board {
        let mut b = Board::new("flux", "Viktor");
        b.add_stage("s1", "first stage").unwrap();
        b.add_item(item("a", "s1", "2026-11-15")).unwrap();
        b
    }

    fn small() -> Forecast {
        Forecast { hours: 2.0, min_free_mem_gb: Some(20.0), files: 3, money_qug: 0.0, touches_production: false }
    }

    #[test]
    fn default_tolerances_are_the_rules_viktor_already_set() {
        let t = Tolerances::default();
        assert_eq!((t.max_hours, t.min_free_mem_gb, t.max_files), (8.0, 12.0, 20));
        assert_eq!(t.max_money_qug, 0.0);
        assert!(!t.production_allowed);
    }

    #[test]
    fn inside_tolerance_raises_nothing_and_starts_the_item() {
        let mut b = board();
        assert!(b.check("a", small(), "2026-11-15").unwrap().is_empty());
        assert_eq!(b.items[0].status, ItemStatus::Active);
        assert!(b.exceptions.is_empty());
    }

    #[test]
    fn any_money_move_is_an_exception_but_zero_is_not() {
        let mut b = board();
        assert!(b.check("a", small(), "2026-11-15").unwrap().is_empty(), "0 QUG moves nothing");
        let f = Forecast { money_qug: 0.0001, ..small() };
        let br = b.check("a", f, "2026-11-15").unwrap();
        assert_eq!(br.len(), 1);
        assert_eq!(br[0].tolerance, "money");
        assert_eq!(b.items[0].status, ItemStatus::Exception);
        assert_eq!(b.exceptions.len(), 1);
    }

    #[test]
    fn production_and_memory_and_scope_breaches_are_named() {
        let mut b = board();
        let f = Forecast { hours: 9.0, min_free_mem_gb: Some(11.5), files: 21, money_qug: 0.0, touches_production: true };
        let names: Vec<String> = b.check("a", f, "2026-11-15").unwrap().into_iter().map(|x| x.tolerance).collect();
        assert_eq!(names, vec!["time", "memory", "scope", "production"]);
        // a forecast with no build never breaks the memory floor
        let mut b2 = board();
        let nb = Forecast { min_free_mem_gb: None, ..small() };
        assert!(b2.check("a", nb, "2026-11-15").unwrap().is_empty());
    }

    #[test]
    fn item_tolerances_beat_stage_which_beat_board() {
        let mut b = board();
        b.stages[0].tolerances = Some(Tolerances { production_allowed: true, ..Tolerances::default() });
        assert!(b.effective_tolerances("a").unwrap().production_allowed, "stage beats board");
        b.items[0].tolerances = Some(Tolerances { max_hours: 1.0, ..Tolerances::default() });
        let t = b.effective_tolerances("a").unwrap();
        assert_eq!(t.max_hours, 1.0);
        assert!(!t.production_allowed, "item beats stage");
    }

    #[test]
    fn decide_moves_the_item_and_cannot_decide_twice() {
        let mut b = board();
        b.check("a", Forecast { touches_production: true, ..small() }, "2026-11-15").unwrap();
        b.decide(1, Decision::Continue, "deploy is Viktor's !", "2026-11-15").unwrap();
        assert_eq!(b.items[0].status, ItemStatus::Active);
        assert!(b.decide(1, Decision::Drop, "again", "2026-11-15").is_err());
        assert!(b.decide(9, Decision::Drop, "none", "2026-11-15").is_err());
    }

    #[test]
    fn stage_cannot_close_with_an_undecided_exception() {
        let mut b = board();
        b.add_item(item("b", "s1", "2026-11-15")).unwrap();
        b.check("a", Forecast { files: 99, ..small() }, "2026-11-15").unwrap();
        let err = b.end_stage("s1", "2026-11-16").unwrap_err();
        assert!(err.contains("#1 a"), "{err}");
        b.decide(1, Decision::Drop, "not worth 99 files", "2026-11-16").unwrap();
        b.mark_done("b", "2026-11-16").unwrap();
        let r = b.end_stage("s1", "2026-11-16").unwrap();
        assert_eq!(r.done, vec!["b"]);
        assert_eq!(r.dropped, vec!["a"]);
        assert!(r.carried.is_empty());
        assert!(b.add_item(item("c", "s1", "2026-11-16")).is_err(), "closed stage takes no new items");
        assert!(b.end_stage("s1", "2026-11-17").is_err(), "cannot close twice");
    }

    #[test]
    fn stale_justification_is_found_by_date() {
        let mut b = board();
        b.add_item(item("old", "s1", "2026-06-24")).unwrap();
        b.add_item(item("edge", "s1", "2026-10-16")).unwrap(); // exactly 30 days before 11-15
        let ids: Vec<&str> = b.stale("2026-11-15", 30).iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, vec!["old"]);
        b.rejustify("old", "2026-11-15", Some("still needed: X")).unwrap();
        assert!(b.stale("2026-11-15", 30).is_empty());
        assert!(b.rejustify("old", "2026-11-15", Some("  ")).is_err());
        // done items never count as stale
        b.items[0].justified_on = "2020-01-01".into();
        b.items[0].status = ItemStatus::Done;
        assert!(b.stale("2026-11-15", 30).is_empty());
    }

    #[test]
    fn items_without_a_benefit_or_with_a_bad_stage_are_refused() {
        let mut b = board();
        let mut no = item("x", "s1", "2026-11-15");
        no.benefit = " ".into();
        assert!(b.add_item(no).unwrap_err().contains("no benefit"));
        assert!(b.add_item(item("y", "nope", "2026-11-15")).unwrap_err().contains("unknown stage"));
        assert!(b.add_item(item("a", "s1", "2026-11-15")).unwrap_err().contains("duplicate"));
        assert!(b.add_item(item("z", "s1", "15-11-2026")).unwrap_err().contains("not a YYYY-MM-DD"));
    }

    #[test]
    fn highlight_names_exceptions_stale_items_and_missing_checks() {
        let mut b = board();
        let mut old = item("old", "s1", "2026-06-24");
        old.done_check.clear();
        b.add_item(old).unwrap();
        b.check("a", Forecast { money_qug: 5.0, ..small() }, "2026-11-15").unwrap();
        b.add_lesson("2026-11-15", "grade tests by N passed, N >= 1");
        let h = b.highlight("2026-11-15");
        assert!(h.contains("exceptions waiting for the board: 1"), "{h}");
        assert!(h.contains("#1 a (raised 2026-11-15): money: 5 QUG vs ≤ 0 QUG"), "{h}");
        assert!(h.contains("benefit not re-checked in 30 days: 1"), "{h}");
        assert!(h.contains("no done-check (cannot be proven done with one command): 1"), "{h}");
        assert!(h.contains("grade tests by N passed"), "{h}");
    }

    #[test]
    fn save_load_roundtrip_keeps_decisions() {
        let mut b = board();
        b.check("a", Forecast { touches_production: true, ..small() }, "2026-11-15").unwrap();
        b.decide(1, Decision::Replan, "stage it first", "2026-11-15").unwrap();
        let p = std::env::temp_dir().join(format!("flux-prince2-board-{}.json", std::process::id()));
        b.save(&p).unwrap();
        let back = Board::load(&p).unwrap();
        let _ = std::fs::remove_file(&p);
        assert_eq!(back.exceptions[0].decision, Some(Decision::Replan));
        assert_eq!(back.items[0].status, ItemStatus::Planned);
        assert_eq!(back.version, BOARD_VERSION);
    }

    #[test]
    fn overview_rows_parse_including_done_and_inner_pipes() {
        let md = "## 8. Roadmap\n| # | Step | What | Why | Size |\n|---|---|---|---|---|\n\
| **1** | ✅ **DONE (2026-06-24)** — Fix the Phase-1 build | Was a poisoned cache | Unblocks everything | **S** |\n\
| **2** | **Make the cache actually hit** | store bytes | Live 0% hit | a|b | **M** |\n\
| not a row |\n";
        let rows = parse_overview_rows(md);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], OverviewRow { number: "1".into(), step: "Fix the Phase-1 build".into(), why: "Unblocks everything".into(), done: true });
        assert_eq!(rows[1].step, "Make the cache actually hit");
        assert_eq!(rows[1].why, "Live 0% hit | a|b");
        assert!(!rows[1].done);
        let r7 = parse_overview_rows("| **7** | **Decide frontend posture — own it OR not** | w | y | **XL** |");
        assert_eq!(r7[0].step, "Decide frontend posture — own it OR not", "a dash inside a title is kept");
        let ab = parse_overview_rows("| **3** | **ABANDONED plan** | w | y | **S** |");
        assert!(!ab[0].done, "ABANDONED contains DONE but is not done");
    }

    #[test]
    fn dates_count_days_correctly() {
        assert_eq!(days_from_ymd("1970-01-01"), Some(0));
        assert_eq!(days_from_ymd("2026-11-15").unwrap() - days_from_ymd("2026-06-24").unwrap(), 144);
        assert_eq!(days_from_ymd("2024-03-01").unwrap() - days_from_ymd("2024-02-28").unwrap(), 2, "leap year");
        assert_eq!(days_from_ymd("2026-13-01"), None);
        assert_eq!(days_from_ymd("26-11-15"), None);
    }
}
