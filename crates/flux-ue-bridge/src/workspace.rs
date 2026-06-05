//! Workspace topology generator.
//!
//! When the user doesn't provide an explicit `--workspace-json` we
//! synthesize one by walking `crates/*/Cargo.toml` relative to the
//! current working directory. Each crate's LOC is the line count of
//! `src/**/*.rs`. The architecture score isn't computable from disk
//! alone — we use a heuristic (test-file ratio + doc-string density)
//! that approximates what `flux_quantum_architect` reports, so the
//! browser viewer still gets coloured towers even when fluxc-mcp isn't
//! running.

use serde::Serialize;
use std::fs;
use std::path::PathBuf;

#[derive(Serialize)]
pub struct Workspace {
    pub crates: Vec<Crate>,
    pub generated_at: u64,
}

#[derive(Serialize)]
pub struct Crate {
    pub name: String,
    pub loc: usize,
    /// Architecture score, 0..100. Heuristic when fluxc-mcp isn't
    /// available; replaced by the real number once a webhook delivers
    /// fresh stats.
    pub score: f64,
    pub test_loc: usize,
    pub doc_loc: usize,
}

pub fn synthesize(workspace_root: &std::path::Path) -> Workspace {
    let crates_dir = workspace_root.join("crates");
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(&crates_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            let manifest = path.join("Cargo.toml");
            if !manifest.exists() {
                continue;
            }
            let (loc, test_loc, doc_loc) = count_rust(&path.join("src"));
            // Heuristic score:
            //   60 base + min(20, test_loc * 200 / loc) test coverage credit
            //          + min(10, doc_loc * 100 / loc) docs credit
            //          - bonus -5 if no tests at all (sub-30% LOC)
            let test_ratio = if loc > 0 { test_loc as f64 * 100.0 / loc as f64 } else { 0.0 };
            let doc_ratio = if loc > 0 { doc_loc as f64 * 100.0 / loc as f64 } else { 0.0 };
            let mut score = 60.0
                + (test_ratio * 0.40).min(20.0)
                + (doc_ratio * 0.30).min(10.0);
            if test_loc == 0 && loc > 100 { score -= 5.0; }
            score = score.clamp(35.0, 95.0);

            out.push(Crate { name, loc, score, test_loc, doc_loc });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Workspace {
        crates: out,
        generated_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    }
}

fn count_rust(src_dir: &std::path::Path) -> (usize, usize, usize) {
    let mut total = 0usize;
    let mut test = 0usize;
    let mut doc = 0usize;
    walk(src_dir, &mut |file: &PathBuf| {
        let Ok(text) = fs::read_to_string(file) else { return; };
        let mut in_tests = false;
        for line in text.lines() {
            total += 1;
            let trimmed = line.trim_start();
            if trimmed.starts_with("///") || trimmed.starts_with("//!") {
                doc += 1;
            }
            // crude "are we inside a #[cfg(test)] mod" tracker: any line
            // mentioning #[cfg(test)] flips the bit; closing brace at zero
            // indent un-flips. Approximate; good enough for a heuristic.
            if trimmed.contains("#[cfg(test)]") || trimmed.contains("#[test]") {
                in_tests = true;
            }
            if in_tests {
                test += 1;
                if line.starts_with('}') {
                    in_tests = false;
                }
            }
        }
    });
    (total, test, doc)
}

fn walk(dir: &std::path::Path, visit: &mut dyn FnMut(&PathBuf)) {
    let Ok(entries) = fs::read_dir(dir) else { return; };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, visit);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            visit(&path);
        }
    }
}
