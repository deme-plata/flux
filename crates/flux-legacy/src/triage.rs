//! triage.rs — **Legacy Health: a hospital for sick code.**
//!
//! flux-legacy is a hospital, and a brownfield workspace is its waiting room. This module is the ER
//! triage desk: it reframes the [`LegacyReport`](crate::LegacyReport) as a **patient board** — every
//! crate is a patient with vital signs and a triage **acuity**, the sickest seen first. Each patient
//! gets a one-line **diagnosis** and a **prescription** (the refactor that treats it).
//!
//! The metaphor maps the whole prototype ladder onto a hospital:
//!   * TRIAGE      — `analyze` (P1) + this module: vitals + acuity, who's sickest
//!   * DIAGNOSIS   — `precheck` (P5) + `ask` (corpus → DeepSeek 1M): the specialist consult
//!   * SURGERY     — `split` (P2): excise the god-file tumor into clean modules
//!   * RECOVERY    — `verify` (P4) + `shadow` (P6): did the patient survive (build/test/state-roots)
//!   * DISCHARGE   — `pipeline` (P6): send the healed code home (branch → hub → Beta)
//!   * THE ICU     — `stability` (P10): the LIVE node on a monitor; WATCH CLOSELY / code-blue
//!
//! Vital signs (from observed metrics, no guessing):
//!   * weight       = LOC (obese crates are hard to move)
//!   * tumor        = a god-file (single .rs over the threshold)
//!   * immune system= tests (none → immunocompromised, any refactor can infect it silently)
//!   * contagious   = fan-in (many dependents → a change spreads)

use crate::{LegacyReport, GOD_FILE_LOC};
use serde::{Deserialize, Serialize};

/// ER triage acuity — who gets seen first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Acuity {
    /// life-threatening — operate now
    Critical,
    /// serious — schedule soon
    Urgent,
    /// chronic but contained — monitor
    Stable,
    /// no presenting condition
    Healthy,
}

impl Acuity {
    pub fn icon(self) -> &'static str {
        match self {
            Acuity::Critical => "🔴",
            Acuity::Urgent => "🟠",
            Acuity::Stable => "🟡",
            Acuity::Healthy => "🟢",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Acuity::Critical => "CRITICAL",
            Acuity::Urgent => "URGENT",
            Acuity::Stable => "STABLE",
            Acuity::Healthy => "HEALTHY",
        }
    }
    /// sort weight, sickest first
    fn rank(self) -> u8 {
        match self {
            Acuity::Critical => 0,
            Acuity::Urgent => 1,
            Acuity::Stable => 2,
            Acuity::Healthy => 3,
        }
    }
}

/// One crate, seen as a patient.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Patient {
    pub crate_name: String,
    pub acuity: Acuity,
    pub vitals: Vec<String>,
    pub diagnosis: String,
    pub prescription: String,
}

/// The whole ward.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Ward {
    pub workspace: String,
    pub patients: Vec<Patient>,
    pub critical: usize,
    pub urgent: usize,
    pub stable: usize,
    pub healthy: usize,
}

// ── triage thresholds (a god-file is GOD_FILE_LOC=800) ──
const TUMOR_LARGE: usize = 5000; // a malignant god-file
const TUMOR_MED: usize = 2000;
const CONTAGIOUS: usize = 8; // fan-in that makes a change spread

/// Triage the whole workspace into a patient board.
pub fn triage(report: &LegacyReport) -> Ward {
    let mut patients: Vec<Patient> = report.crates.iter().map(assess_crate).collect();
    patients.sort_by(|a, b| {
        a.acuity.rank().cmp(&b.acuity.rank())
            .then(b.crate_name.len().cmp(&a.crate_name.len())) // stable-ish tiebreak
    });
    // recompute order properly: sickest, then by weight within acuity
    patients.sort_by(|a, b| a.acuity.rank().cmp(&b.acuity.rank()));

    let count = |ac: Acuity| patients.iter().filter(|p| p.acuity == ac).count();
    Ward {
        workspace: report.workspace_name.clone(),
        critical: count(Acuity::Critical),
        urgent: count(Acuity::Urgent),
        stable: count(Acuity::Stable),
        healthy: count(Acuity::Healthy),
        patients,
    }
}

fn assess_crate(c: &crate::LegacyCrate) -> Patient {
    let tumor = c.biggest_file_loc >= GOD_FILE_LOC;
    let immunocompromised = !c.has_tests && c.loc >= 200;
    let contagious = c.dependents.len() >= CONTAGIOUS;

    // acuity
    let acuity = if c.biggest_file_loc >= TUMOR_LARGE || (tumor && immunocompromised && contagious) {
        Acuity::Critical
    } else if c.biggest_file_loc >= TUMOR_MED || (immunocompromised && contagious) || (tumor && contagious) {
        Acuity::Urgent
    } else if tumor || immunocompromised {
        Acuity::Stable
    } else {
        Acuity::Healthy
    };

    // vitals
    let mut vitals = vec![format!("weight {} LOC", commas(c.loc))];
    if tumor {
        vitals.push(format!("🩻 tumor: {} ({} LOC)", short(&c.biggest_file), commas(c.biggest_file_loc)));
    }
    vitals.push(if c.has_tests { "immune ✓".into() } else { "⚠ no immune system (0 tests)".into() });
    if !c.dependents.is_empty() {
        vitals.push(format!("contagious: {} contacts", c.dependents.len()));
    }

    // diagnosis
    let mut dx: Vec<String> = Vec::new();
    if c.biggest_file_loc >= TUMOR_LARGE {
        dx.push(format!("malignant god-file ({} LOC)", commas(c.biggest_file_loc)));
    } else if tumor {
        dx.push(format!("god-file hypertrophy ({} LOC)", commas(c.biggest_file_loc)));
    }
    if immunocompromised {
        dx.push("immunocompromised (no tests)".into());
    }
    if contagious {
        dx.push(format!("highly contagious ({} dependents)", c.dependents.len()));
    }
    let diagnosis = if dx.is_empty() { "no presenting condition".into() } else { dx.join("; ") };

    // prescription
    let mut rx: Vec<String> = Vec::new();
    if tumor {
        rx.push(format!("excise {} → focused modules (split)", short(&c.biggest_file)));
    }
    if immunocompromised {
        rx.push("immunize: add a test module before any operation".into());
    }
    if contagious && tumor {
        rx.push("isolate: extract a thin trait/types crate so the change doesn't spread".into());
    }
    let prescription = if rx.is_empty() { "discharge — no treatment needed".into() } else { rx.join(" · ") };

    Patient { crate_name: c.name.clone(), acuity, vitals, diagnosis, prescription }
}

/// Render the patient board, sickest first.
pub fn render_ward(w: &Ward) -> String {
    let mut s = format!(
        "🏥 LEGACY HEALTH — {} · ER board\n   {} 🔴 critical · {} 🟠 urgent · {} 🟡 stable · {} 🟢 healthy  (of {} patients)\n\n",
        if w.workspace.is_empty() { "(workspace)" } else { &w.workspace },
        w.critical, w.urgent, w.stable, w.healthy, w.patients.len(),
    );
    // show every non-healthy patient, then a healthy tally
    let sick: Vec<&Patient> = w.patients.iter().filter(|p| p.acuity != Acuity::Healthy).collect();
    for p in sick.iter().take(25) {
        s.push_str(&format!("{} {} {}\n", p.acuity.icon(), p.acuity.label(), p.crate_name));
        s.push_str(&format!("    vitals: {}\n", p.vitals.join(" · ")));
        s.push_str(&format!("    dx: {}\n", p.diagnosis));
        s.push_str(&format!("    rx: {}\n", p.prescription));
    }
    if w.healthy > 0 {
        let names: Vec<&str> = w.patients.iter().filter(|p| p.acuity == Acuity::Healthy).map(|p| p.crate_name.as_str()).take(12).collect();
        s.push_str(&format!("\n🟢 discharged ({} healthy): {}{}\n", w.healthy, names.join(", "), if w.healthy > names.len() { ", …" } else { "" }));
    }
    s
}

fn commas(n: usize) -> String {
    let s = n.to_string();
    let b = s.as_bytes();
    let mut out = String::new();
    for (i, c) in b.iter().enumerate() {
        if i > 0 && (b.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*c as char);
    }
    out
}

fn short(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LegacyCrate;

    fn crat(name: &str, loc: usize, biggest: usize, tests: bool, deps: usize) -> LegacyCrate {
        LegacyCrate {
            name: name.into(),
            loc,
            biggest_file: format!("src/{name}.rs"),
            biggest_file_loc: biggest,
            has_tests: tests,
            dependents: (0..deps).map(|i| format!("d{i}")).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn the_sickest_crate_is_critical_and_sorted_first() {
        let report = LegacyReport {
            workspace_name: "q-narwhalknight".into(),
            crates: vec![
                crat("q-leaf", 120, 120, true, 0),               // healthy
                crat("q-api-server", 146_000, 15_481, true, 1),  // malignant god-file → critical
                crat("q-util", 1500, 700, false, 2),             // immunocompromised, small → stable
                crat("q-types", 17_000, 5_066, true, 64),        // malignant + contagious → critical
            ],
            ..Default::default()
        };
        let w = triage(&report);
        assert_eq!(w.patients[0].acuity, Acuity::Critical, "sickest first: {:?}", w.patients[0]);
        assert!(w.critical >= 2);
        // the leaf is discharged healthy with no treatment
        let leaf = w.patients.iter().find(|p| p.crate_name == "q-leaf").unwrap();
        assert_eq!(leaf.acuity, Acuity::Healthy);
        assert!(leaf.prescription.contains("discharge"));
        // a tumor crate is prescribed an excision (split)
        let api = w.patients.iter().find(|p| p.crate_name == "q-api-server").unwrap();
        assert!(api.prescription.contains("excise"), "{}", api.prescription);
    }

    #[test]
    fn no_tests_plus_high_fan_in_is_urgent() {
        // no tumor, but immunocompromised AND contagious → urgent
        let report = LegacyReport {
            crates: vec![crat("q-hub", 600, 400, false, 10)],
            ..Default::default()
        };
        let w = triage(&report);
        assert_eq!(w.patients[0].acuity, Acuity::Urgent, "{:?}", w.patients[0]);
        assert!(w.patients[0].prescription.contains("immunize"));
    }

    #[test]
    fn a_clean_crate_is_healthy() {
        let report = LegacyReport { crates: vec![crat("q-clean", 300, 250, true, 3)], ..Default::default() };
        let w = triage(&report);
        assert_eq!(w.patients[0].acuity, Acuity::Healthy);
        assert_eq!(w.healthy, 1);
    }
}
