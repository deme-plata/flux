//! admit.rs — **the full clinical pathway for ONE patient.**
//!
//! Where `drive` (P8) runs the swarm over many targets and `autopilot` (P9) loops the whole node,
//! `admit` walks a SINGLE crate through the hospital end to end, human-readable, dry-run by default:
//!
//!   🏥 ADMISSION (triage)  → 🧠 PSYCH eval → 🩺 CONSULT (optional, DeepSeek) →
//!   🔪 SURGERY (split the worst god-file) → 🩹 RECOVERY (precheck gate) → 📋 DISCHARGE (sync plan)
//!
//! It only orchestrates the existing wards' public APIs and writes nothing (the discharge is the
//! branch it WOULD create; landing still needs `sync --confirm`).

use crate::triage::Acuity;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One step of the pathway.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stage {
    pub ward: String,
    pub ok: bool,
    pub finding: String,
}

/// The patient's full admission record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdmissionRecord {
    pub patient: String,
    pub acuity: Acuity,
    pub stages: Vec<Stage>,
    /// true = nothing more to do (healthy, or pathway complete to a clean discharge plan)
    pub discharged: bool,
    /// the branch a `sync --confirm` would create, if surgery is indicated
    pub discharge_branch: Option<String>,
}

/// What to do during admission.
#[derive(Debug, Clone)]
pub struct AdmitOpts {
    /// call the independent doctor (DeepSeek) — a network call + cost; off by default
    pub consult: bool,
    pub model: String,
    pub window: u32,
    pub timeout_s: u64,
}

impl Default for AdmitOpts {
    fn default() -> Self {
        AdmitOpts { consult: false, model: crate::ask::MODEL_FLASH.to_string(), window: 120_000, timeout_s: 240 }
    }
}

/// Walk one crate through the whole hospital. Dry-run: writes nothing.
pub fn admit(root: &str, crate_name: &str, opts: &AdmitOpts) -> AdmissionRecord {
    let report = crate::analyze_workspace_legacy(root);
    let ward = crate::triage::triage(&report);
    let patient = ward.patients.iter().find(|p| p.crate_name == crate_name).cloned();

    let mut stages = Vec::new();
    let acuity = patient.as_ref().map(|p| p.acuity).unwrap_or(Acuity::Healthy);

    // 🏥 ADMISSION (triage)
    match &patient {
        Some(p) => stages.push(Stage {
            ward: "🏥 admission".into(),
            ok: true,
            finding: format!("{} {} — {} · {}", p.acuity.icon(), p.acuity.label(), p.diagnosis, p.vitals.join(" · ")),
        }),
        None => {
            stages.push(Stage { ward: "🏥 admission".into(), ok: false, finding: format!("no such patient `{crate_name}` on the board") });
            return AdmissionRecord { patient: crate_name.into(), acuity, stages, discharged: true, discharge_branch: None };
        }
    }
    let patient = patient.unwrap();

    // 🧠 PSYCH eval
    let cdir = PathBuf::from(root).join("crates").join(crate_name);
    let src_dir = cdir.join("src");
    let mut episodes = 0usize;
    let mut worst: Option<(crate::psych::Disorder, usize)> = None;
    for f in walk_rs(&src_dir) {
        if let Ok(c) = std::fs::read_to_string(&f) {
            for d in crate::psych::evaluate_source("", &c) {
                episodes += d.episodes;
                if worst.map(|(_, n)| d.episodes > n).unwrap_or(true) {
                    worst = Some((d.disorder, d.episodes));
                }
            }
        }
    }
    let psych_finding = match worst {
        Some((d, n)) => format!("{episodes} episodes · worst: {} ({n}×) → 💊 {}", d.name(), d.medication()),
        None => format!("{episodes} episodes · ward calm"),
    };
    stages.push(Stage { ward: "🧠 psych".into(), ok: true, finding: psych_finding });

    // 🩺 CONSULT (optional, independent doctor)
    if opts.consult {
        match crate::consult::consult_crate(root, crate_name, &opts.model, opts.window, opts.timeout_s) {
            Ok(note) => {
                let severity = note.note.lines().find(|l| l.to_uppercase().contains("SEVERITY"))
                    .unwrap_or_else(|| note.note.lines().next().unwrap_or(""));
                stages.push(Stage { ward: "🩺 consult".into(), ok: true, finding: format!("{} ({} tok in) — {}", note.model, note.tokens_in, severity.trim()) });
            }
            Err(e) => stages.push(Stage { ward: "🩺 consult".into(), ok: false, finding: format!("specialist unavailable: {e}") }),
        }
    }

    // healthy patient → discharge, no surgery
    let god_file_rel = if patient.acuity == Acuity::Healthy || patient_biggest(&report, crate_name).1 < crate::GOD_FILE_LOC {
        stages.push(Stage { ward: "📋 discharge".into(), ok: true, finding: "no surgery indicated — discharge with monitoring".into() });
        return AdmissionRecord { patient: crate_name.into(), acuity, stages, discharged: true, discharge_branch: None };
    } else {
        patient_biggest(&report, crate_name).0
    };

    // 🔪 SURGERY (split the worst god-file)
    let god_abs = cdir.join(&god_file_rel);
    let src = std::fs::read_to_string(&god_abs).unwrap_or_default();
    if src.is_empty() {
        stages.push(Stage { ward: "🔪 surgery".into(), ok: false, finding: format!("couldn't open {god_file_rel}") });
        return AdmissionRecord { patient: crate_name.into(), acuity, stages, discharged: false, discharge_branch: None };
    }
    let patch = crate::split::plan_split(god_abs.to_str().unwrap_or(&god_file_rel), &src, 8);
    stages.push(Stage {
        ward: "🔪 surgery".into(),
        ok: true,
        finding: format!("split {} ({} items) → {} modules (dry-run)", short(&god_file_rel), patch.items_total, patch.modules.len()),
    });

    // 🩹 RECOVERY (precheck gate — the cheap pre-op check; full verify is `--verify` / P4)
    let pre = crate::precheck::precheck_split(&patch);
    let recovered = !matches!(pre.verdict, crate::precheck::Verdict::Unsafe);
    stages.push(Stage {
        ward: "🩹 recovery".into(),
        ok: recovered,
        finding: format!("precheck {:?} · {}/{} items placed (confidence {:.0}%)", pre.verdict, pre.items_placed, pre.items_total, pre.confidence * 100.0),
    });

    // 📋 DISCHARGE (the sync plan — what would land; never auto-confirmed)
    let crate_src_rel = format!("crates/{crate_name}/{}", parent_of(&god_file_rel));
    let _ = crate_src_rel; // (the bin builds the real edits; here we just name the branch)
    let opts_sync = crate::pipeline::SyncOpts::for_split(crate_name, short(&god_file_rel), "origin");
    let branch = opts_sync.branch.clone();
    let discharged = recovered;
    stages.push(Stage {
        ward: "📋 discharge".into(),
        ok: discharged,
        finding: if discharged {
            format!("ready: `flux-legacy sync {root} crates/{crate_name}/{god_file_rel} --confirm` → branch {branch} → hub → Beta")
        } else {
            "HELD in recovery — precheck Unsafe, do NOT discharge".into()
        },
    });

    AdmissionRecord { patient: crate_name.into(), acuity, stages, discharged, discharge_branch: Some(branch) }
}

/// The crate's biggest file (relative path within the crate dir) + its LOC.
fn patient_biggest(report: &crate::LegacyReport, crate_name: &str) -> (String, usize) {
    report.crates.iter().find(|c| c.name == crate_name)
        .map(|c| (c.biggest_file.clone(), c.biggest_file_loc))
        .unwrap_or_default()
}

/// Render the admission record as a patient pathway.
pub fn render_admission(r: &AdmissionRecord) -> String {
    let mut s = format!("🏥 ADMISSION RECORD — patient: {} {}\n\n", r.acuity.icon(), r.patient);
    for st in &r.stages {
        s.push_str(&format!("  {} {:<14} {}\n", if st.ok { "✓" } else { "✗" }, st.ward, st.finding));
    }
    s.push_str(&format!("\n  → {}\n", if r.discharged { "DISCHARGED (pathway complete · dry-run, nothing landed)" } else { "ADMITTED (held — see the held stage)" }));
    s
}

fn walk_rs(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(entries) = std::fs::read_dir(&d) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() { stack.push(p); }
                else if p.extension().map(|x| x == "rs").unwrap_or(false) { out.push(p); }
            }
        }
    }
    out
}

fn short(path: &str) -> &str { path.rsplit('/').next().unwrap_or(path) }
fn parent_of(path: &str) -> String {
    std::path::Path::new(path).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn synth(root: &PathBuf, crate_name: &str, god_loc: usize, tests: bool) {
        let s = root.join("crates").join(crate_name).join("src");
        fs::create_dir_all(&s).unwrap();
        fs::write(root.join("crates").join(crate_name).join("Cargo.toml"), format!("[package]\nname = \"{crate_name}\"\n")).unwrap();
        let body: String = (0..god_loc).map(|i| format!("pub fn handle_thing_{i}() {{ let _ = {i}; }}\n")).collect();
        fs::write(s.join("lib.rs"), body).unwrap();
        if tests { fs::write(s.join("t.rs"), "#[cfg(test)]\nmod t { #[test] fn x(){} }\n").unwrap(); }
    }

    #[test]
    fn a_sick_crate_walks_the_whole_pathway_to_a_discharge_branch() {
        let tmp = std::env::temp_dir().join(format!("flux-admit-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        synth(&tmp, "q-sick", 1200, false); // 1200-LOC god-file, no tests
        let rec = admit(tmp.to_str().unwrap(), "q-sick", &AdmitOpts::default()); // consult off (no network)
        let wards: Vec<&str> = rec.stages.iter().map(|s| s.ward.as_str()).collect();
        assert!(wards.contains(&"🏥 admission"));
        assert!(wards.contains(&"🧠 psych"));
        assert!(wards.contains(&"🔪 surgery"));
        assert!(wards.contains(&"🩹 recovery"));
        assert!(wards.contains(&"📋 discharge"));
        assert!(rec.discharge_branch.as_deref().unwrap_or("").starts_with("refactor/q-sick-"));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_healthy_crate_is_discharged_without_surgery() {
        let tmp = std::env::temp_dir().join(format!("flux-admit-h-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        synth(&tmp, "q-fit", 50, true); // tiny, tested → healthy
        let rec = admit(tmp.to_str().unwrap(), "q-fit", &AdmitOpts::default());
        assert!(rec.discharged);
        assert!(rec.discharge_branch.is_none());
        assert!(!rec.stages.iter().any(|s| s.ward == "🔪 surgery"), "no surgery for a healthy patient");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn unknown_patient_is_handled() {
        let tmp = std::env::temp_dir().join(format!("flux-admit-u-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("crates")).unwrap();
        let rec = admit(tmp.to_str().unwrap(), "ghost", &AdmitOpts::default());
        assert!(rec.discharged);
        assert!(rec.stages[0].finding.contains("no such patient"));
        let _ = fs::remove_dir_all(&tmp);
    }
}
