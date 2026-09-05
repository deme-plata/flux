//! flux-study — a study loop you can trust.
//!
//! Three ideas, kept deliberately small:
//!
//! 1. **A goal has a deadline and a "why".** Without the date it is a wish; without the why it
//!    is a chore. Both are stored, both are printed every time.
//! 2. **The loop picks the next thing.** `today` orders every open task by how close its goal's
//!    deadline is, fills a time budget, and hands back a plan. Run it once, or let `loop` run it
//!    on a cadence. Nothing is hidden: the ordering is the deadline, in days.
//! 3. **Progress is what you marked done, nothing else.** No streaks, no estimates dressed up as
//!    achievements. `progress` counts done tasks over all tasks, per goal, plus days left.
//!
//! The store is one JSON file. Dates are `YYYY-MM-DD` strings compared as day numbers, so a
//! goal due tomorrow sorts ahead of one due next month even across a year boundary.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Task {
    pub id: u32,
    pub title: String,
    /// Honest estimate, in minutes. Used only to fill a day's budget.
    pub minutes: u32,
    pub done: bool,
    /// `YYYY-MM-DD` when marked done, else empty.
    pub done_on: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Goal {
    pub id: u32,
    pub title: String,
    /// `YYYY-MM-DD`.
    pub deadline: String,
    pub why: String,
    pub tasks: Vec<Task>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Study {
    pub goals: Vec<Goal>,
    /// The SIGIL University side: what has been studied, in the order it was, with the day.
    pub university: Vec<Credit>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Credit {
    pub day: String,
    pub module: String,
    pub points: u32,
    pub evidence: String,
}

// ── dates as day numbers (Howard Hinnant's civil-from-days, no chrono needed) ──

pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = (m as u64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

/// Parse `YYYY-MM-DD` to a day number; `None` if malformed.
pub fn day_number(s: &str) -> Option<i64> {
    let mut it = s.trim().split('-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: u32 = it.next()?.parse().ok()?;
    let d: u32 = it.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) { return None; }
    Some(days_from_civil(y, m, d))
}

/// Today as `YYYY-MM-DD` (UTC). Overridable with `FLUX_STUDY_TODAY` so tests and a
/// planning session on the evening before can pin the day.
pub fn today() -> String {
    if let Ok(t) = std::env::var("FLUX_STUDY_TODAY") { if day_number(&t).is_some() { return t; } }
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    civil_from_days(secs.div_euclid(86_400))
}

pub fn civil_from_days(z: i64) -> String {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

// ── the store ────────────────────────────────────────────────────────────────

pub fn default_path() -> PathBuf {
    if let Ok(p) = std::env::var("FLUX_STUDY_FILE") { return PathBuf::from(p); }
    // Off the 40 GB root partition on Epsilon; anywhere else, next to the user's home.
    let big = Path::new("/home/storage/claude-code/study");
    if big.parent().is_some_and(|p| p.is_dir()) { return big.join("study.json"); }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".flux").join("study.json")
}

impl Study {
    pub fn load(path: &Path) -> Result<Study, String> {
        if !path.exists() { return Ok(Study::default()); }
        let s = std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        serde_json::from_str(&s).map_err(|e| format!("parse {}: {e}", path.display()))
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() { std::fs::create_dir_all(dir).map_err(|e| e.to_string())?; }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    fn next_goal_id(&self) -> u32 { self.goals.iter().map(|g| g.id).max().unwrap_or(0) + 1 }

    pub fn add_goal(&mut self, title: &str, deadline: &str, why: &str) -> Result<u32, String> {
        day_number(deadline).ok_or_else(|| format!("deadline must be YYYY-MM-DD, got {deadline:?}"))?;
        let id = self.next_goal_id();
        self.goals.push(Goal { id, title: title.into(), deadline: deadline.into(), why: why.into(), tasks: vec![] });
        Ok(id)
    }

    pub fn add_task(&mut self, goal_id: u32, title: &str, minutes: u32) -> Result<u32, String> {
        let g = self.goals.iter_mut().find(|g| g.id == goal_id).ok_or(format!("no goal {goal_id}"))?;
        let id = g.tasks.iter().map(|t| t.id).max().unwrap_or(0) + 1;
        g.tasks.push(Task { id, title: title.into(), minutes, done: false, done_on: String::new() });
        Ok(id)
    }

    /// Mark `goal.task` done on `day`. Idempotent.
    pub fn done(&mut self, goal_id: u32, task_id: u32, day: &str) -> Result<(), String> {
        let g = self.goals.iter_mut().find(|g| g.id == goal_id).ok_or(format!("no goal {goal_id}"))?;
        let t = g.tasks.iter_mut().find(|t| t.id == task_id).ok_or(format!("no task {goal_id}.{task_id}"))?;
        t.done = true;
        t.done_on = day.into();
        Ok(())
    }

    /// Open tasks in the order the loop works them: nearest goal deadline first, then task id.
    /// Returns `(days_left, goal, task)`.
    pub fn queue(&self, day: &str) -> Vec<(i64, &Goal, &Task)> {
        let now = day_number(day).unwrap_or(0);
        let mut out: Vec<(i64, &Goal, &Task)> = self.goals.iter().flat_map(|g| {
            let left = day_number(&g.deadline).map(|d| d - now).unwrap_or(i64::MAX);
            g.tasks.iter().filter(|t| !t.done).map(move |t| (left, g, t))
        }).collect();
        out.sort_by_key(|(left, g, t)| (*left, g.id, t.id));
        out
    }

    /// Today's plan: the front of the queue until `budget_minutes` is used. Always at least
    /// one task if any is open, so a short day still moves the nearest deadline.
    pub fn plan(&self, day: &str, budget_minutes: u32) -> Vec<(i64, &Goal, &Task)> {
        let mut used = 0u32;
        let mut plan = Vec::new();
        for item in self.queue(day) {
            if !plan.is_empty() && used + item.2.minutes > budget_minutes { break; }
            used += item.2.minutes;
            plan.push(item);
        }
        plan
    }

    /// `(goal, done, total, days_left)` per goal.
    pub fn progress(&self, day: &str) -> Vec<(&Goal, usize, usize, i64)> {
        let now = day_number(day).unwrap_or(0);
        self.goals.iter().map(|g| {
            let done = g.tasks.iter().filter(|t| t.done).count();
            (g, done, g.tasks.len(), day_number(&g.deadline).map(|d| d - now).unwrap_or(0))
        }).collect()
    }

    pub fn credit(&mut self, day: &str, module: &str, points: u32, evidence: &str) {
        self.university.push(Credit { day: day.into(), module: module.into(), points, evidence: evidence.into() });
    }

    pub fn university_points(&self) -> u32 { self.university.iter().map(|c| c.points).sum() }

    /// Viktor's actual goals, with the dates verified on the schools' pages on 2026-09-05.
    /// Nothing here is invented: every deadline is the one the school publishes, and the first
    /// goal is the one that gates all the others.
    pub fn seed_viktor(&mut self) {
        if !self.goals.is_empty() { return; }
        let g = self.add_goal("Færdiggør HA(it.)-bachelorprojektet (CBS, startet 2009)", "2027-06-30",
            "Uden bacheloren kan hverken CBS-MBA eller Stanford søges — og en bachelor færdig i 2026-27 opfylder Knight-Hennessys krav om eksamen fra jan 2020 eller senere.").unwrap();
        for (t, m) in [
            ("Skriv til CBS Student Affairs: genindskrivning på HA(it.) fra 2009-årgangen — hvad mangler, hvilke regler om studietid gælder", 60),
            ("Vælg emne og skriv en side problemformulering (forslag: agentisk økonomi på Quillon/SIGIL, eller K-parameteren som konsensus-mål)", 120),
            ("Find vejleder: send problemformulering + link til artiklerne til to mulige vejledere", 60),
            ("Litteraturliste: 15 kilder, halvdelen peer-reviewed", 180),
            ("Kapitel 1–2 udkast (indledning, metode)", 480),
            ("Kapitel 3–4 udkast (analyse, diskussion)", 600),
            ("Aflever", 60),
        ] { self.add_task(g, t, m).unwrap(); }

        let g = self.add_goal("GMAT Focus ≥ 655 (CBS-klassens snit; gulv 555)", "2026-11-30",
            "Én test åbner begge døre: CBS kræver den, Stanford kræver GMAT eller GRE.").unwrap();
        for (t, m) in [
            ("Book GMAT Focus til sidst i november (gmac.com), og noter datoen her", 30),
            ("Diagnostisk prøve — skriv de tre svageste områder ned", 150),
            ("Quant: 2 timer, tre gange om ugen i fire uger — log resultatet hver gang", 1440),
            ("Data Insights: 1 time, tre gange om ugen i fire uger", 720),
            ("Verbal: 1 time, to gange om ugen i fire uger", 480),
            ("Fuld prøve 1 under tidspres", 180),
            ("Fuld prøve 2 under tidspres — beslut retake-dato hvis under 655", 180),
        ] { self.add_task(g, t, m).unwrap(); }

        let g = self.add_goal("IELTS Academic ≥ 7.0", "2026-12-10",
            "HA(it.) er undervist på dansk, så Stanford kræver en engelsktest; CBS kræver IELTS 7.0.").unwrap();
        for (t, m) in [("Book IELTS (British Council / IDP, København eller Aalborg)", 30), ("To fulde prøver: writing task 2 rettes af en anden", 300)] {
            self.add_task(g, t, m).unwrap();
        }

        let g = self.add_goal("Stanford GSB MBA — Round 2 (frist 6. jan 2027, 16:00 PT; svar 1. apr 2027)", "2027-01-06",
            "Runde 1 lukker 9. sep 2026 — for tidligt. Runde 2 er den ærlige chance. Pris 2026-27: $89.187 tuition, ~$140.940/år alt inkl.; behovsbaserede GSB-legater er åbne for internationale.").unwrap();
        for (t, m) in [
            ("Vælg to anbefalere (én der så dig bygge, én der så dig lede) og send dem briefingen", 90),
            ("Essay A: 'What matters most to you, and why?' — første udkast i egne ord", 240),
            ("Essay B: 'Why Stanford?' — første udkast", 180),
            ("CV i admissions-sprog (to live kæder drevet alene, artikler, open source-toolchain)", 180),
            ("Transcripts fra CBS bestilles", 30),
            ("Indsend", 60),
        ] { self.add_task(g, t, m).unwrap(); }

        let g = self.add_goal("CBS Full-time MBA — runde 2 (frist 10. jan 2027; start okt 2027)", "2027-01-10",
            "Ét år i København, DKK 380.000. Legater: CBS Excellence, DSEB 40 %, Blue MBA op til 40 %. Runde 1 er 10. nov 2026, hvis alt er klar før.").unwrap();
        for (t, m) in [
            ("Registrér interesse for 2027-28 hos CBS MBA admissions", 15),
            ("Ansøgning + motivationsbrev", 240),
            ("DSEB-legat: ansøgning (40 % af tuition)", 120),
            ("Indsend", 60),
        ] { self.add_task(g, t, m).unwrap(); }

        let g = self.add_goal("SIGIL University — grundmoduler", "2027-03-31",
            "Kredit for det, der faktisk er lært om vores egen kæde — målt, ikke påstået.").unwrap();
        for (t, m) in [
            ("Modul 1: SIGIL_GENESIS_v0.md — forklar de fire state roots på én side", 120),
            ("Modul 2: kør chronos-harnessen selv og aflæs bytes/blok", 120),
            ("Modul 3: K* — udled hvorfor K*≈1 er en grænse (Margolus–Levitin) uden at kigge", 180),
            ("Modul 4: en shielded betaling ende-til-ende: note, anchor, nullifier — tegn den", 120),
        ] { self.add_task(g, t, m).unwrap(); }
    }
}
