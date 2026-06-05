//! consult.rs — **DeepSeek as the INDEPENDENT consulting physician.**
//!
//! [`triage`](crate::triage) and [`psych`](crate::psych) are the in-house resident's read — static,
//! fast, free. `consult` calls in an outside specialist: it packs ONE patient's chart (a crate's
//! source, outlined when it won't fit) and sends it to DeepSeek **cold** — examined without being
//! told our diagnosis — for an independent consult note. [`second_opinion`] then puts the two
//! side by side: agreement builds confidence, divergence flags a re-read.
//!
//! Independence matters. When the whole node was sent at 1M, DeepSeek called the zk-crates "mocks";
//! old-school reading proved them 17K LOC of real code. A consult is a second opinion, not gospel.

use crate::ask::{ask_deepseek, AskResult, MODEL_FLASH};
use crate::context::outline;
use flux_context::est_tokens;
use std::fs;
use std::path::{Path, PathBuf};

/// The independent specialist's prompt — examines COLD, prescribes, no prior diagnosis assumed.
pub const ATTENDING_SYSTEM: &str = "You are an INDEPENDENT consulting software physician doing a \
    cold chart review of ONE Rust crate. You have NOT been told any prior diagnosis — form your own. \
    Reply in this structure: \
    DIAGNOSIS: the conditions you see (god-files, coupling, error-handling, unsafe, dead code, \
    correctness/safety smells), citing file names. \
    SEVERITY: one of CRITICAL / URGENT / STABLE / HEALTHY, with one line of why. \
    TREATMENT: the 3 highest-value fixes, most important first. \
    Be concrete and do not invent code that isn't shown.";

/// The consulting doctor's note.
#[derive(Debug, Clone)]
pub struct ConsultNote {
    pub patient: String,
    pub model: String,
    pub note: String,
    pub files_sent: usize,
    pub tokens_in: u64,
    pub tokens_out: u64,
}

/// Pack one crate's source into a bounded chart: whole files while they fit `window` tokens, then
/// signature-outlines so more of the crate is still visible. Returns (bundle, files_included).
pub fn crate_chart(root: &str, crate_name: &str, window: u32) -> Result<(String, usize), String> {
    let cdir = PathBuf::from(root).join("crates").join(crate_name);
    let src = cdir.join("src");
    if !src.is_dir() {
        return Err(format!("no such crate src: {}", src.display()));
    }
    // biggest files first so the chart leads with the body of the patient, outline the tail
    let mut files: Vec<(PathBuf, String, u32)> = walk_rs(&src)
        .into_iter()
        .filter_map(|f| fs::read_to_string(&f).ok().map(|c| {
            let t = est_tokens(&c);
            (f, c, t)
        }))
        .collect();
    files.sort_by(|a, b| b.2.cmp(&a.2));

    let mut bundle = format!("// crate: {crate_name} ({} files)\n\n", files.len());
    let mut used: u32 = 0;
    let mut included = 0usize;
    for (path, content, toks) in &files {
        let rel = path.strip_prefix(&cdir).unwrap_or(path).to_string_lossy();
        if used + toks <= window {
            bundle.push_str(&format!("// ==== {rel} ====\n{content}\n\n"));
            used += toks;
            included += 1;
        } else {
            let o = outline(content, 1500);
            let ot = est_tokens(&o);
            if used + ot <= window {
                bundle.push_str(&format!("// ==== {rel} (OUTLINE) ====\n{o}\n\n"));
                used += ot;
                included += 1;
            }
        }
    }
    Ok((bundle, included))
}

/// Send a crate to the independent doctor and return the consult note.
pub fn consult_crate(
    root: &str,
    crate_name: &str,
    model: &str,
    window: u32,
    timeout_s: u64,
) -> Result<ConsultNote, String> {
    let (chart, files_sent) = crate_chart(root, crate_name, window)?;
    let user = format!("Patient: crate `{crate_name}`. Chart follows.\n\n{chart}");
    let AskResult { model, answer, prompt_tokens, completion_tokens } =
        ask_deepseek(model, ATTENDING_SYSTEM, &user, timeout_s)?;
    Ok(ConsultNote {
        patient: crate_name.to_string(),
        model,
        note: answer,
        files_sent,
        tokens_in: prompt_tokens,
        tokens_out: completion_tokens,
    })
}

/// In-house resident's read of one crate (triage acuity + psych episode count), as a short string.
pub fn in_house_read(root: &str, crate_name: &str) -> String {
    let report = crate::analyze_workspace_legacy(root);
    let ward = crate::triage::triage(&report);
    let acuity = ward
        .patients
        .iter()
        .find(|p| p.crate_name == crate_name)
        .map(|p| format!("{} {} — {}", p.acuity.icon(), p.acuity.label(), p.diagnosis))
        .unwrap_or_else(|| "(not on the board)".into());

    // psych: total episodes for this crate
    let cdir = PathBuf::from(root).join("crates").join(crate_name).join("src");
    let mut episodes = 0usize;
    for f in walk_rs(&cdir) {
        if let Ok(c) = fs::read_to_string(&f) {
            episodes += crate::psych::evaluate_source("", &c).iter().map(|x| x.episodes).sum::<usize>();
        }
    }
    format!("triage: {acuity}\n   psych: {episodes} behavioral episodes")
}

/// Put the in-house read and the independent consult side by side.
pub fn second_opinion(note: &ConsultNote, in_house: &str) -> String {
    format!(
        "🏥 SECOND OPINION — patient: {}\n\n── IN-HOUSE (static resident) ──\n   {in_house}\n\n\
         ── INDEPENDENT CONSULT ({}, {} files, {} in / {} out tok) ──\n{}\n\n\
         ⚖  Reconcile: where they AGREE, act with confidence; where the consult claims something the \
         resident didn't flag (or vice-versa), READ THE CODE before trusting either.",
        note.patient, note.model, note.files_sent, note.tokens_in, note.tokens_out, note.note,
    )
}

fn walk_rs(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(entries) = fs::read_dir(&d) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().map(|x| x == "rs").unwrap_or(false) {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// Default consult window (leaves room for the prompt + answer inside a long context).
pub const DEFAULT_CONSULT_WINDOW: u32 = 200_000;
/// re-export so the bin doesn't need to import ask just for the default model
pub const DEFAULT_MODEL: &str = MODEL_FLASH;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chart_packs_a_crate_and_outlines_when_tight() {
        let tmp = std::env::temp_dir().join(format!("flux-consult-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let s = tmp.join("crates/q-demo/src");
        fs::create_dir_all(&s).unwrap();
        fs::write(s.join("lib.rs"), "pub fn a() {}\n".repeat(50)).unwrap();
        fs::write(s.join("big.rs"), "pub fn helper() { let _ = 1; }\n".repeat(400)).unwrap();

        // tiny window forces the big file to outline (or drop), small file to fit
        let (chart, n) = crate_chart(tmp.to_str().unwrap(), "q-demo", 200).unwrap();
        assert!(chart.contains("crate: q-demo"));
        assert!(n >= 1);
        assert!(est_tokens(&chart) <= 220, "respects the window-ish");

        // generous window includes everything verbatim
        let (chart2, n2) = crate_chart(tmp.to_str().unwrap(), "q-demo", 1_000_000).unwrap();
        assert_eq!(n2, 2);
        assert!(chart2.contains("lib.rs") && chart2.contains("big.rs"));
        assert!(!chart2.contains("(OUTLINE)"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn missing_crate_is_an_error_not_a_panic() {
        assert!(crate_chart("/nonexistent-root", "nope", 1000).is_err());
    }

    #[test]
    fn second_opinion_shows_both_reads() {
        let note = ConsultNote {
            patient: "q-types".into(),
            model: "deepseek-v4-flash".into(),
            note: "DIAGNOSIS: god-file lib.rs.\nSEVERITY: CRITICAL\nTREATMENT: split it.".into(),
            files_sent: 27,
            tokens_in: 49000,
            tokens_out: 800,
        };
        let out = second_opinion(&note, "triage: 🔴 CRITICAL — malignant god-file\n   psych: 40 episodes");
        assert!(out.contains("IN-HOUSE"));
        assert!(out.contains("INDEPENDENT CONSULT"));
        assert!(out.contains("Reconcile"));
        assert!(out.contains("q-types"));
    }
}
