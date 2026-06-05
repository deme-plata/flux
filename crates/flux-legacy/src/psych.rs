//! psych.rs — the **PSYCHIATRY clinic** of Legacy Health.
//!
//! [`triage`](crate::triage) treats PHYSICAL illness (size, missing tests, coupling). Psych treats
//! BEHAVIORAL pathology — "wicked" code whose disorder shows in *how* it's written. Each pattern is
//! a real, grep-able code smell mapped to a (tongue-in-cheek) DSM diagnosis, a medication, and the
//! REAL refactor that heals it back to normal. The wickedness score is a weighted sum.
//!
//! It's a joke with teeth: the patterns it flags (swallowed errors, `unwrap` spam, `unsafe`,
//! unfinished `todo!`) are exactly the ones that cause the 2am pages.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// A behavioral disorder a file can present with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Disorder {
    /// `unsafe` blocks — ignores the safety rules
    Antisocial,
    /// `.unwrap()` / `.expect()` / `panic!` spam — every call site is a cliff edge
    SelfHarm,
    /// swallowed errors (`let _ =`, `.ok();`, `Err(_) =>`, `unwrap_or_default`) — denial
    Avoidant,
    /// `todo!` / `unimplemented!` / `unreachable!` — learned helplessness
    Depression,
    /// `// TODO` / `FIXME` / `HACK` / `XXX` — intrusive thoughts left on the walls
    Intrusive,
    /// very long functions / deep nesting — disorganized thinking
    Dissociative,
}

impl Disorder {
    pub fn name(self) -> &'static str {
        match self {
            Disorder::Antisocial => "Antisocial Personality Disorder",
            Disorder::SelfHarm => "Self-Harm / Acute Anxiety",
            Disorder::Avoidant => "Avoidant Denial",
            Disorder::Depression => "Major Depression (learned helplessness)",
            Disorder::Intrusive => "Intrusive Thoughts (OCD)",
            Disorder::Dissociative => "Dissociative Identity (disorganized thinking)",
        }
    }
    /// the prescribed "medication" 💊 (the joke) — paired with the real therapy below
    pub fn medication(self) -> &'static str {
        match self {
            Disorder::Antisocial => "Risperdal 4mg — calm the unsafe acting-out",
            Disorder::SelfHarm => "Sertraline — stop self-harming at every call site",
            Disorder::Avoidant => "Abilify 10mg — face the errors instead of suppressing them",
            Disorder::Depression => "Bupropion — finish what was started",
            Disorder::Intrusive => "CBT — resolve or ticket the intrusive notes",
            Disorder::Dissociative => "Abilify 15mg — reintegrate the split personalities",
        }
    }
    /// the REAL refactor that heals it
    pub fn therapy(self) -> &'static str {
        match self {
            Disorder::Antisocial => "audit every `unsafe`, document the invariant it upholds, shrink the block",
            Disorder::SelfHarm => "replace `.unwrap()`/`panic!` with `?` and typed errors",
            Disorder::Avoidant => "handle (or log) the error — never `let _ =` a Result silently",
            Disorder::Depression => "implement the `todo!`/`unimplemented!` or delete the dead path",
            Disorder::Intrusive => "convert TODO/FIXME/HACK into tracked issues, then remove",
            Disorder::Dissociative => "split the long function into named, single-purpose steps",
        }
    }
    fn weight(self) -> u32 {
        match self {
            Disorder::Antisocial => 5, // wicked
            Disorder::Avoidant => 4,
            Disorder::SelfHarm => 3,
            Disorder::Depression => 2,
            Disorder::Dissociative => 2,
            Disorder::Intrusive => 1,
        }
    }
}

/// One diagnosis for one file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PsychFinding {
    pub file: String,
    pub disorder: Disorder,
    /// number of occurrences (the symptom count)
    pub episodes: usize,
    pub evidence: String,
}

/// The clinic's report over a crate or workspace.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PsychReport {
    pub scope: String,
    pub files_seen: usize,
    pub findings: Vec<PsychFinding>,
    /// weighted total — how "wicked" the patient is
    pub wickedness: u32,
}

/// Diagnose one source string. Pure — give it code, get disorders.
pub fn evaluate_source(file: &str, src: &str) -> Vec<PsychFinding> {
    // count occurrences ignoring obvious string/comment noise is overkill for a screen; we count
    // line-level signatures, which is what a clinician eyeballs anyway.
    let mut out = Vec::new();
    let mut push = |d: Disorder, n: usize, ev: &str| {
        if n > 0 {
            out.push(PsychFinding { file: file.to_string(), disorder: d, episodes: n, evidence: ev.to_string() });
        }
    };

    let count = |needles: &[&str]| -> usize {
        src.lines()
            .map(|l| {
                let t = l.trim_start();
                if t.starts_with("//") { return 0; }
                needles.iter().filter(|n| l.contains(**n)).count()
            })
            .sum()
    };

    push(Disorder::Antisocial, count(&["unsafe {", "unsafe{", "unsafe fn"]), "`unsafe` blocks");
    push(Disorder::SelfHarm, count(&[".unwrap()", ".expect(", "panic!("]), "`.unwrap()`/`.expect()`/`panic!`");
    push(Disorder::Avoidant, count(&["let _ =", ".ok();", "Err(_) =>", "unwrap_or_default()", "= Ok(());"]), "swallowed errors");
    push(Disorder::Depression, count(&["todo!", "unimplemented!", "unreachable!"]), "`todo!`/`unimplemented!`");
    // intrusive notes: only inside comments
    let notes = src.lines().filter(|l| {
        let t = l.trim_start();
        t.starts_with("//") && ["TODO", "FIXME", "HACK", "XXX"].iter().any(|m| t.contains(m))
    }).count();
    push(Disorder::Intrusive, notes, "TODO/FIXME/HACK notes");
    // dissociative: a single fn body longer than ~120 lines (disorganized)
    push(Disorder::Dissociative, long_functions(src, 120), "functions over 120 lines");

    out
}

/// Count top-level-ish functions whose body exceeds `limit` lines (brace-depth heuristic).
fn long_functions(src: &str, limit: usize) -> usize {
    let lines: Vec<&str> = src.lines().collect();
    let mut count = 0;
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim_start();
        let is_fn = (t.starts_with("fn ") || t.starts_with("pub fn ") || t.contains(" fn "))
            && lines[i].contains('(');
        if is_fn {
            // find the body span by brace depth from this line
            let mut depth = 0i32;
            let mut started = false;
            let mut j = i;
            while j < lines.len() {
                for c in lines[j].chars() {
                    if c == '{' { depth += 1; started = true; }
                    else if c == '}' { depth -= 1; }
                }
                if started && depth <= 0 { break; }
                j += 1;
            }
            if j > i && (j - i) > limit {
                count += 1;
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    count
}

/// Walk a workspace (`<root>/crates/*/src`) and diagnose every file. Worst (most episodes) first.
pub fn evaluate_workspace(root: &str) -> PsychReport {
    let crates_dir = PathBuf::from(root).join("crates");
    let mut findings = Vec::new();
    let mut files_seen = 0usize;
    if let Ok(entries) = fs::read_dir(&crates_dir) {
        for e in entries.flatten() {
            let src = e.path().join("src");
            if !src.is_dir() {
                continue;
            }
            let cname = e.path().file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string();
            for f in walk_rs(&src) {
                if let Ok(content) = fs::read_to_string(&f) {
                    files_seen += 1;
                    let rel = format!("{cname}/{}", f.strip_prefix(&e.path()).unwrap_or(&f).to_string_lossy());
                    findings.extend(evaluate_source(&rel, &content));
                }
            }
        }
    }
    findings.sort_by(|a, b| (b.disorder.weight() * b.episodes as u32).cmp(&(a.disorder.weight() * a.episodes as u32)));
    let wickedness = findings.iter().map(|f| f.disorder.weight() * f.episodes as u32).sum();
    PsychReport {
        scope: PathBuf::from(root).file_name().and_then(|n| n.to_str()).unwrap_or("workspace").to_string(),
        files_seen,
        findings,
        wickedness,
    }
}

/// Render the psych ward — the most disturbed files first, with their meds.
pub fn render_psych(r: &PsychReport) -> String {
    let mut s = format!(
        "🧠 LEGACY HEALTH — PSYCHIATRY · {}\n   {} files screened · wickedness score {} · {} diagnoses\n\n",
        r.scope, r.files_seen, r.wickedness, r.findings.len(),
    );
    for f in r.findings.iter().take(20) {
        s.push_str(&format!("  🛋  {}  ({} episodes)\n", f.file, f.episodes));
        s.push_str(&format!("      dx: {} — {}\n", f.disorder.name(), f.evidence));
        s.push_str(&format!("      💊 {}\n", f.disorder.medication()));
        s.push_str(&format!("      therapy: {}\n", f.disorder.therapy()));
    }
    if r.findings.is_empty() {
        s.push_str("  🟢 no behavioral disorders — the ward is calm.\n");
    }
    s
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

#[cfg(test)]
mod tests {
    use super::*;

    const WICKED: &str = r#"
// TODO: fix this someday
pub fn handle(x: u8) -> u32 {
    let raw = unsafe { std::mem::transmute::<u8, i8>(x) };
    let v = some_call().unwrap();
    let _ = risky_write(); // swallow it
    if v > 0 { todo!() }
    raw as u32
}
"#;

    #[test]
    fn wicked_code_gets_the_full_workup() {
        let f = evaluate_source("evil.rs", WICKED);
        let has = |d: Disorder| f.iter().any(|x| x.disorder == d);
        assert!(has(Disorder::Antisocial), "unsafe → antisocial");
        assert!(has(Disorder::SelfHarm), "unwrap → self-harm");
        assert!(has(Disorder::Avoidant), "let _ = → avoidant");
        assert!(has(Disorder::Depression), "todo! → depression");
        assert!(has(Disorder::Intrusive), "TODO comment → intrusive");
        // antisocial is the wicked one — heaviest weight
        let worst = f.iter().max_by_key(|x| x.disorder.weight()).unwrap();
        assert_eq!(worst.disorder, Disorder::Antisocial);
        assert!(worst.disorder.medication().contains("Risperdal"));
    }

    #[test]
    fn calm_code_leaves_the_ward_empty() {
        let calm = "pub fn add(a: u32, b: u32) -> u32 { a + b }\n";
        let f = evaluate_source("good.rs", calm);
        assert!(f.is_empty(), "no disorders in calm code: {f:?}");
    }

    #[test]
    fn comments_dont_trigger_self_harm() {
        // an `.unwrap()` mentioned in a comment is not an episode
        let src = "// never call .unwrap() here\npub fn ok() -> u32 { 1 }\n";
        let f = evaluate_source("c.rs", src);
        assert!(!f.iter().any(|x| x.disorder == Disorder::SelfHarm));
    }
}
