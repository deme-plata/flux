//! build.rs — headless `vite build` grader. The agent-workflow counterpart to
//! the live HMR engine: an agent builds (it doesn't hot-reload in a browser),
//! so this runs `vite build`, captures the real build telemetry, and grades it.
//!
//! Pairs with `verify.rs` (headless render check) so a deploy can be gated on
//! "built clean + rendered without console errors" instead of a human eyeball.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Instant;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

/// One built asset, as reported by Vite's output table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetInfo {
    pub name: String,
    pub bytes: u64,
    pub gzip_bytes: Option<u64>,
}

/// Composite 0-100 build-quality score (the build-time SAP analogue).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildScore {
    /// Faster builds score higher (100 @ ≤1.5s → 0 @ ≥30s).
    pub speed: u8,
    /// 100 if zero build errors, else 0.
    pub errors_clean: u8,
    /// Bundle weight health from total gzipped JS (100 @ ≤120KB → low @ >1.5MB).
    pub bundle_health: u8,
    /// 100 minus 12 per chunk/size warning, floored at 0.
    pub warnings_clean: u8,
    /// Weighted composite.
    pub composite: u8,
}

/// Full result of grading a `vite build`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildReport {
    pub ok: bool,
    pub build_ms: u64,
    pub assets: Vec<AssetInfo>,
    pub total_bytes: u64,
    pub total_gzip_bytes: u64,
    pub js_gzip_bytes: u64,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
    pub score: BuildScore,
    pub out_dir: PathBuf,
}

/// Resolve the vite binary for a project: prefer the project-local
/// `node_modules/.bin/vite`, then a `vite_bin` override, then `npx vite`.
fn vite_command(project: &Path, vite_bin: &Option<String>) -> (String, Vec<String>) {
    let local = project.join("node_modules/.bin/vite");
    if local.is_file() {
        return (local.to_string_lossy().into_owned(), vec!["build".into()]);
    }
    if let Some(b) = vite_bin {
        return (b.clone(), vec!["build".into()]);
    }
    ("npx".into(), vec!["vite".into(), "build".into()])
}

/// Run `vite build` in `cfg.project_path` and grade it. Captures stdout/stderr,
/// parses Vite's asset table, and never hangs (build has a 5-minute ceiling).
pub async fn build_project(cfg: &crate::ViteConfig) -> Result<BuildReport> {
    let project = cfg.project_path.clone();
    let (bin, args) = vite_command(&project, &cfg.vite_bin);

    let t0 = Instant::now();
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        Command::new(&bin)
            .args(&args)
            .current_dir(&project)
            .env("CI", "true")
            .env("NO_COLOR", "1")
            .env("FORCE_COLOR", "0")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .context("vite build timed out after 300s")?
    .with_context(|| format!("failed to spawn `{bin}`"))?;
    let build_ms = t0.elapsed().as_millis() as u64;

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{stdout}\n{stderr}");

    let assets = parse_assets(&combined);
    let mut warnings: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for line in combined.lines() {
        let l = line.trim();
        let low = l.to_lowercase();
        if low.contains("(!) ") || low.contains("warning") || low.contains("chunks are larger") {
            warnings.push(l.to_string());
        }
        if (low.contains("error") || low.contains("could not resolve") || low.contains("[vite]: rollup failed"))
            && !low.contains("0 error")
        {
            errors.push(l.to_string());
        }
    }
    let ok = out.status.success() && errors.is_empty();

    let total_bytes: u64 = assets.iter().map(|a| a.bytes).sum();
    let total_gzip_bytes: u64 = assets.iter().filter_map(|a| a.gzip_bytes).sum();
    let js_gzip_bytes: u64 = assets
        .iter()
        .filter(|a| a.name.ends_with(".js"))
        .filter_map(|a| a.gzip_bytes)
        .sum();

    let score = grade(build_ms, ok, js_gzip_bytes, warnings.len());

    Ok(BuildReport {
        ok,
        build_ms,
        assets,
        total_bytes,
        total_gzip_bytes,
        js_gzip_bytes,
        warnings,
        errors,
        score,
        out_dir: project.join("dist"),
    })
}

/// Parse Vite's asset table lines, e.g.
/// `dist/assets/index-u1xHAsva.js   156.72 kB │ gzip: 50.74 kB`
/// Strip ANSI SGR escape sequences (`\x1b[...m`) so colorized Vite output parses.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // consume until a letter (the SGR terminator, usually 'm')
            for n in chars.by_ref() {
                if n.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn parse_assets(text: &str) -> Vec<AssetInfo> {
    let mut v = Vec::new();
    for raw in text.lines() {
        let line = strip_ansi(raw);
        // Vite uses a box-drawing │ between size and gzip; the asset path is the
        // first whitespace token and contains a dot-extension.
        let Some((path_part, rest)) = line.trim().split_once(char::is_whitespace) else { continue };
        if !path_part.contains('.') || !path_part.starts_with("dist") {
            continue;
        }
        let Some(bytes) = parse_kb(rest) else { continue };
        let gzip_bytes = rest.split("gzip:").nth(1).and_then(parse_kb);
        let name = path_part.trim_start_matches("dist/").to_string();
        v.push(AssetInfo { name, bytes, gzip_bytes });
    }
    v
}

/// Pull the first `<num> kB` out of a fragment → bytes (Vite's kB = 1000).
fn parse_kb(frag: &str) -> Option<u64> {
    let idx = frag.find("kB")?;
    // trim the whitespace between the number and "kB" before scanning back.
    let head = frag[..idx].trim_end();
    let num: String = head
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    let kb: f64 = num.trim().parse().ok()?;
    Some((kb * 1000.0) as u64)
}

fn grade(build_ms: u64, ok: bool, js_gzip: u64, warns: usize) -> BuildScore {
    let speed = {
        let s = build_ms as f64;
        let v = if s <= 1500.0 { 100.0 } else { 100.0 * (1.0 - (s - 1500.0) / 28_500.0) };
        v.clamp(0.0, 100.0) as u8
    };
    let errors_clean = if ok { 100 } else { 0 };
    let bundle_health = {
        let kb = js_gzip as f64 / 1000.0;
        let v = if kb <= 120.0 { 100.0 } else { 100.0 * (1.0 - (kb - 120.0) / 1400.0) };
        v.clamp(0.0, 100.0) as u8
    };
    let warnings_clean = 100u8.saturating_sub((warns as u8).saturating_mul(12));
    // errors dominate; then bundle + speed + warnings.
    let composite = ((errors_clean as f64) * 0.40
        + (bundle_health as f64) * 0.25
        + (speed as f64) * 0.20
        + (warnings_clean as f64) * 0.15) as u8;
    BuildScore { speed, errors_clean, bundle_health, warnings_clean, composite }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vite_table() {
        let s = "\
vite v5.4.21 building for production...
dist/index.html                   0.90 kB │ gzip:  0.53 kB
dist/assets/index-D83qMKSw.css    6.22 kB │ gzip:  1.99 kB
dist/assets/index-u1xHAsva.js   156.72 kB │ gzip: 50.74 kB
✓ built in 1.43s";
        let a = parse_assets(s);
        assert_eq!(a.len(), 3);
        assert_eq!(a[2].name, "assets/index-u1xHAsva.js");
        assert_eq!(a[2].bytes, 156_720);
        assert_eq!(a[2].gzip_bytes, Some(50_740));
    }

    #[test]
    fn clean_build_scores_high() {
        let s = grade(1430, true, 50_740, 0);
        assert!(s.composite >= 90, "clean fast small build should score high, got {}", s.composite);
        assert_eq!(s.errors_clean, 100);
    }

    #[test]
    fn failed_build_tanks_score() {
        let s = grade(1430, false, 50_740, 0);
        assert_eq!(s.errors_clean, 0);
        assert!(s.composite < 65);
    }
}
