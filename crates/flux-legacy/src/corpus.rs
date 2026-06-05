//! corpus.rs — pack the highest-value code of a legacy repo into a token-budgeted LLM analysis
//! bundle (owner: rocky-vision). The "get DeepSeek + Claude up to 1M with all the Quillon Graph
//! code we can" lane.
//!
//! Integrates three flux crates so flux-legacy isn't an island:
//!   * [`flux_context`]   — `est_tokens` + `DEFAULT_WINDOW_TOKENS` (1M) + `ContextBudget` exact math
//!   * [`crate::context`]::outline — signature-only compression for files that won't fit verbatim
//!   * [`crate::LegacyReport`] — rank by crate fan-in + file role (lib/main/_api first)
//!
//! A 100-crate node is ~8M tokens — it does NOT fit in 1M. The value is choosing the right 1M:
//! the API surface + the most-depended-on crates go in VERBATIM; the next tier is OUTLINED
//! (signatures only) so more subsystems are *visible*; the rest is named-but-skipped (never
//! silently dropped — the manifest lists every file's fate).

use crate::context::outline;
use crate::LegacyReport;
use flux_context::{est_tokens, kind_from_name, ContextBudget, ContextSource, DEFAULT_WINDOW_TOKENS};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// How a file made it into (or out of) the bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PackMode {
    /// included verbatim
    Full,
    /// included as signatures only (context::outline)
    Outline,
    /// named in the manifest but not included (budget exhausted)
    Skip,
}

/// Per-file packing decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusFile {
    pub path: String,
    pub crate_name: String,
    pub mode: PackMode,
    pub tokens: u32,
    pub priority: f64,
}

/// The packed corpus plan + the flux-context budget that scored it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Corpus {
    pub root: String,
    pub window_tokens: u32,
    pub files_total: usize,
    pub full_count: usize,
    pub outline_count: usize,
    pub skipped_count: usize,
    pub tokens_packed: u32,
    pub fill_ratio: f64,
    pub files: Vec<CorpusFile>,
    pub budget: ContextBudget,
}

/// chars budget per outlined file (~500 tokens) — enough for a module's signatures.
pub const OUTLINE_CHARS: usize = 2000;

/// Build the corpus plan: rank every `.rs` under `<root>/crates`, then greedily pack into
/// `window_tokens` (Full → Outline → Skip). Reads each file once.
pub fn build_corpus(report: &LegacyReport, window_tokens: u32) -> Corpus {
    let root = report.root.clone();
    let fan_in: BTreeMap<&str, usize> =
        report.crates.iter().map(|c| (c.name.as_str(), c.dependents.len())).collect();

    // gather candidates with content tokens + priority
    struct Cand {
        path: String,
        crate_name: String,
        full_tokens: u32,
        outline_tokens: u32,
        priority: f64,
    }
    let mut cands: Vec<Cand> = Vec::new();
    let crates_dir = PathBuf::from(&root).join("crates");
    if let Ok(entries) = fs::read_dir(&crates_dir) {
        for e in entries.flatten() {
            let cdir = e.path();
            let src = cdir.join("src");
            if !src.is_dir() {
                continue;
            }
            let cname = cdir.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string();
            let fi = *fan_in.get(cname.as_str()).unwrap_or(&0);
            for f in walk_rs(&src) {
                let content = match fs::read_to_string(&f) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                let rel = f.strip_prefix(&cdir).unwrap_or(&f).to_string_lossy().to_string();
                let full_tokens = est_tokens(&content);
                let outline_tokens = est_tokens(&outline(&content, OUTLINE_CHARS));
                cands.push(Cand {
                    path: format!("{cname}/{rel}"),
                    crate_name: cname.clone(),
                    full_tokens,
                    outline_tokens,
                    priority: file_priority(&rel, fi),
                });
            }
        }
    }

    // highest priority first; tiebreak: cheaper (smaller) file first so we fit more
    cands.sort_by(|a, b| {
        b.priority
            .partial_cmp(&a.priority)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.full_tokens.cmp(&b.full_tokens))
    });

    let mut budget = ContextBudget::new(window_tokens);
    let mut files = Vec::with_capacity(cands.len());
    let mut packed: u32 = 0;
    let (mut full_count, mut outline_count, mut skipped_count) = (0usize, 0usize, 0usize);

    for c in &cands {
        let (mode, tokens) = if packed + c.full_tokens <= window_tokens {
            (PackMode::Full, c.full_tokens)
        } else if packed + c.outline_tokens <= window_tokens {
            (PackMode::Outline, c.outline_tokens)
        } else {
            (PackMode::Skip, 0)
        };
        match mode {
            PackMode::Full => full_count += 1,
            PackMode::Outline => outline_count += 1,
            PackMode::Skip => skipped_count += 1,
        }
        if mode != PackMode::Skip {
            packed += tokens;
            budget.add(ContextSource::new(c.path.clone(), kind_from_name(&c.path), tokens));
        }
        files.push(CorpusFile {
            path: c.path.clone(),
            crate_name: c.crate_name.clone(),
            mode,
            tokens,
            priority: c.priority,
        });
    }

    let fill_ratio = budget.fill_ratio();
    Corpus {
        root,
        window_tokens,
        files_total: cands.len(),
        full_count,
        outline_count,
        skipped_count,
        tokens_packed: packed,
        fill_ratio,
        files,
        budget,
    }
}

/// Build the concatenated bundle as a String (what you stream to DeepSeek/Claude). Each file is
/// fenced with a path header; outlined files are clearly marked.
pub fn bundle_string(corpus: &Corpus) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "// ===================================================================\n\
         // flux-legacy corpus — {} · {} tok / {} window ({:.0}% full)\n\
         // {} files: {} full · {} outline · {} skipped\n\
         // ===================================================================\n\n",
        corpus.root, corpus.tokens_packed, corpus.window_tokens, corpus.fill_ratio * 100.0,
        corpus.files_total, corpus.full_count, corpus.outline_count, corpus.skipped_count,
    ));
    let crates_dir = PathBuf::from(&corpus.root).join("crates");
    for f in &corpus.files {
        if f.mode == PackMode::Skip {
            continue;
        }
        // path is "<crate>/<rel under src>" → crates/<crate>/<rel> (rel already includes src/)
        let abs = crates_dir.join(&f.path);
        let content = fs::read_to_string(&abs).unwrap_or_default();
        let body = match f.mode {
            PackMode::Outline => outline(&content, OUTLINE_CHARS),
            _ => content,
        };
        let tag = if f.mode == PackMode::Outline { " (OUTLINE — signatures only)" } else { "" };
        out.push_str(&format!("// ==== {}{} ====\n{}\n\n", f.path, tag, body));
    }
    out
}

/// Write the bundle to a file (what you paste / stream to DeepSeek/Claude). Returns tokens packed.
pub fn write_bundle(corpus: &Corpus, out_path: &str) -> std::io::Result<u32> {
    fs::write(out_path, bundle_string(corpus))?;
    Ok(corpus.tokens_packed)
}

/// Human summary of the pack.
pub fn render_corpus(corpus: &Corpus) -> String {
    let mut s = format!(
        "📚 CORPUS — {}\n   window {} tok · packed {} tok ({:.1}% full)\n   {} files → {} full · {} outline · {} skipped\n\n",
        corpus.root, corpus.window_tokens, corpus.tokens_packed, corpus.fill_ratio * 100.0,
        corpus.files_total, corpus.full_count, corpus.outline_count, corpus.skipped_count,
    );
    s.push_str("  top 20 included (priority · mode · tok · file):\n");
    for f in corpus.files.iter().filter(|f| f.mode != PackMode::Skip).take(20) {
        let m = match f.mode {
            PackMode::Full => "full",
            PackMode::Outline => "outl",
            PackMode::Skip => "skip",
        };
        s.push_str(&format!("  {:>5.1}  {:<5} {:>6}  {}\n", f.priority, m, f.tokens, f.path));
    }
    s
}

// ───────────────────────── ranking ─────────────────────────

/// Priority of a file given its path (relative, e.g. `src/handlers.rs`) and its crate's fan-in.
fn file_priority(rel: &str, fan_in: usize) -> f64 {
    let name = Path::new(rel).file_name().and_then(|n| n.to_str()).unwrap_or("");
    let mut p = 1.0 + fan_in as f64 * 0.6; // most-depended-on crates first
    if name == "lib.rs" {
        p += 6.0; // the public API of the crate — highest signal for analysis
    } else if name == "main.rs" {
        p += 3.5; // the entrypoint
    } else if name == "mod.rs" {
        p += 2.0;
    }
    if name.ends_with("_api.rs") || name.contains("handler") {
        p += 2.0; // surface / dispatch
    }
    if name == "types.rs" || name.contains("config") {
        p += 1.0;
    }
    p
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
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LegacyCrate;
    use std::fs;

    fn write(p: &Path, s: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, s).unwrap();
    }

    #[test]
    fn packs_high_priority_first_and_outlines_when_tight() {
        let tmp = std::env::temp_dir().join(format!("flux-legacy-corpus-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        // a hub crate `core` with a lib.rs (high priority) + a big internal file
        write(&tmp.join("crates/core/src/lib.rs"), &"pub fn api() {}\n".repeat(20));
        write(&tmp.join("crates/core/src/internal.rs"), &"fn helper() { let _ = 1; }\n".repeat(400));
        write(&tmp.join("crates/leaf/src/lib.rs"), "pub fn small() {}\n");

        let report = LegacyReport {
            root: tmp.to_string_lossy().to_string(),
            crates: vec![
                LegacyCrate { name: "core".into(), dependents: vec!["leaf".into(), "x".into(), "y".into()], ..Default::default() },
                LegacyCrate { name: "leaf".into(), ..Default::default() },
            ],
            ..Default::default()
        };

        // tiny window forces Full→Outline→Skip transitions
        let corpus = build_corpus(&report, 200);
        assert_eq!(corpus.files_total, 3);
        assert!(corpus.tokens_packed <= 200, "must respect the window");
        // core/lib.rs (highest priority: hub + lib.rs) is the first included file
        let first_included = corpus.files.iter().find(|f| f.mode != PackMode::Skip).unwrap();
        assert_eq!(first_included.path, "core/src/lib.rs");
        // the big internal.rs can't fit verbatim in 200 tok → outlined or skipped, never full
        let internal = corpus.files.iter().find(|f| f.path == "core/src/internal.rs").unwrap();
        assert_ne!(internal.mode, PackMode::Full);

        // a generous window includes everything verbatim
        let big = build_corpus(&report, DEFAULT_WINDOW_TOKENS);
        assert_eq!(big.skipped_count, 0);
        assert_eq!(big.outline_count, 0);
        assert_eq!(big.full_count, 3);

        let _ = fs::remove_dir_all(&tmp);
    }
}
