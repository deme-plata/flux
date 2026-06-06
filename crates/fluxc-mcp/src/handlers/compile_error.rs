//! Compiler error finding combo — rustc diagnostics + source snippets + webhooks.

use crate::handlers::{fluxc_cmd, ws, ToolDef, ToolRegistry};
use crate::handlers::platform_webhook;
use fluxc_core::webhook;
use regex::Regex;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

lazy_static::lazy_static! {
    static ref ERROR_LOC: Regex = Regex::new(r"-->\s+([^:]+):(\d+):(\d+)").unwrap();
    static ref ERROR_MSG: Regex = Regex::new(r"^(error|warning)\[?(E\d+)?\]?:\s*(.+)$").unwrap();
}

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_compile_error_combo",
            description: "Compile/check a package; on failure parse rustc errors, read source snippets at each error site, POST compile_error webhook with full context. Agent gets code at fault lines immediately.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "package": { "type": "string", "description": "Cargo package name" },
                    "release": { "type": "boolean" },
                    "context_lines": { "type": "integer", "description": "Lines before/after error (default 6)" },
                    "webhook_url": { "type": "string", "description": "Optional extra POST target (e.g. Aether control surface :4178/api/mcp-webhook)" }
                },
                "required": ["package"],
                "additionalProperties": false
            }),
        },
        flux_compile_error_combo,
    );
}

#[derive(serde::Serialize)]
struct ErrorSnippet {
    message: String,
    code: Option<String>,
    file: String,
    line: u32,
    col: u32,
    snippet_start: u32,
    snippet_lines: Vec<String>,
    highlight_line: u32,
}

fn resolve_path(file: &str) -> PathBuf {
    let p = Path::new(file);
    if p.is_absolute() {
        return p.to_path_buf();
    }
    ws().join(file)
}

fn read_snippet(path: &Path, line: u32, ctx: u32) -> Option<(u32, Vec<String>)> {
    let content = std::fs::read_to_string(path).ok()?;
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return None;
    }
    let idx = line.saturating_sub(1) as usize;
    let start = idx.saturating_sub(ctx as usize);
    let end = (idx + ctx as usize + 1).min(lines.len());
    let snippet: Vec<String> = lines[start..end]
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let ln = start + i + 1;
            if ln == line as usize {
                format!(">>> {ln:4} | {l}")
            } else {
                format!("    {ln:4} | {l}")
            }
        })
        .collect();
    Some((start as u32 + 1, snippet))
}


fn output_has_compile_errors(combined: &str) -> bool {
    combined.lines().any(|l| {
        let t = l.trim_start();
        t.starts_with("error:")
            || t.starts_with("error[")
            || t.starts_with("error ")
    })
}

fn fallback_errors(combined: &str) -> Vec<ErrorSnippet> {
    combined
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with("error:") || t.starts_with("error[") || t.starts_with("error ")
        })
        .map(|l| ErrorSnippet {
            message: l.trim().to_string(),
            code: None,
            file: "(fluxc)".into(),
            line: 0,
            col: 0,
            snippet_start: 0,
            snippet_lines: vec![l.trim().to_string()],
            highlight_line: 0,
        })
        .collect()
}

fn parse_errors(combined: &str, ctx: u32) -> Vec<ErrorSnippet> {
    let lines: Vec<&str> = combined.lines().collect();
    let mut out = Vec::new();
    let mut last_msg = String::new();
    let mut last_code: Option<String> = None;

    for (i, line) in lines.iter().enumerate() {
        if let Some(cap) = ERROR_MSG.captures(line) {
            last_code = cap.get(2).map(|m| m.as_str().to_string());
            last_msg = cap
                .get(3)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
        }
        if let Some(cap) = ERROR_LOC.captures(line) {
            let file = cap.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            let line_no: u32 = cap.get(2).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
            let col: u32 = cap.get(3).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
            let path = resolve_path(&file);
            let (snippet_start, snippet_lines) = read_snippet(&path, line_no, ctx)
                .unwrap_or((line_no, vec![format!("    (could not read {})", path.display())]));
            out.push(ErrorSnippet {
                message: if last_msg.is_empty() {
                    lines
                        .get(i.saturating_sub(1))
                        .unwrap_or(&"")
                        .to_string()
                } else {
                    last_msg.clone()
                },
                code: last_code.clone(),
                file: file.clone(),
                line: line_no,
                col,
                snippet_start,
                snippet_lines,
                highlight_line: line_no,
            });
        }
    }
    out
}

fn post_extra_webhook(url: &str, payload: &Value) {
    let body = serde_json::to_string(payload).unwrap_or_default();
    let _ = std::process::Command::new("curl")
        .args([
            "-sS",
            "-X",
            "POST",
            "-H",
            "Content-Type: application/json",
            "-d",
            &body,
            url,
        ])
        .output();
}

fn snapshot_path() -> PathBuf {
    ws().join("target/flux-compile-errors/latest.json")
}

fn flux_compile_error_combo(args: &Value) -> String {
    let package = args.get("package").and_then(|v| v.as_str()).unwrap_or("");
    if package.is_empty() || !package
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return "✗ flux_compile_error_combo: invalid package name".into();
    }
    let release = args.get("release").and_then(|v| v.as_bool()).unwrap_or(false);
    let ctx = args
        .get("context_lines")
        .and_then(|v| v.as_u64())
        .unwrap_or(6) as u32;
    let webhook_url = args.get("webhook_url").and_then(|v| v.as_str());

    let start = std::time::Instant::now();
    let mut cmd = fluxc_cmd();
    cmd.args(["build", "--package", package]);
    if release {
        cmd.arg("--release");
    }
    let output = cmd.output();
    let elapsed_ms = start.elapsed().as_millis();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let combined = format!("{stdout}\n{stderr}");
            if out.status.success() && !output_has_compile_errors(&combined) {
                webhook::auto_dispatch(
                    "build_complete",
                    webhook::build_event_data(package, true, elapsed_ms, 0, 0),
                );
                return format!(
                    "✓ flux_compile_error_combo — {} compiles clean ({}ms)",
                    package, elapsed_ms
                );
            }
            let mut errors = parse_errors(&combined, ctx);
            if errors.is_empty() && output_has_compile_errors(&combined) {
                errors = fallback_errors(&combined);
            }
            let payload = json!({
                "event": "compile_error",
                "package": package,
                "elapsed_ms": elapsed_ms,
                "error_count": errors.len(),
                "errors": errors,
            });

            webhook::auto_dispatch("compile_error", payload.clone());
            platform_webhook::dispatch("flux_compile_error_combo", "compile_error", payload.clone());
            webhook::auto_dispatch(
                "build_failed",
                webhook::build_event_data(package, false, elapsed_ms, 0, 0),
            );

            if let Some(url) = webhook_url {
                post_extra_webhook(url, &payload);
            }

            let snap = snapshot_path();
            if let Some(parent) = snap.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&snap, serde_json::to_string_pretty(&payload).unwrap_or_default());

            let mut header = format!(
                "✗ flux_compile_error_combo — {} error site(s) in {} ({}ms)\n  snapshot: {}\n",
                errors.len(),
                package,
                elapsed_ms,
                snap.display()
            );
            for (i, e) in errors.iter().take(5).enumerate() {
                header.push_str(&format!(
                    "\n── {}. {}:{}:{} {} ──\n",
                    i + 1,
                    e.file,
                    e.line,
                    e.col,
                    e.message
                ));
                for ln in &e.snippet_lines {
                    header.push_str(&format!("{ln}\n"));
                }
            }
            if errors.len() > 5 {
                header.push_str(&format!("\n  ... and {} more (see snapshot JSON)\n", errors.len() - 5));
            }
            header
        }
        Err(e) => format!("✗ flux_compile_error_combo: failed to run cargo: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_loc_line() {
        let sample = "error[E0425]: cannot find value `x`\n  --> crates/foo/src/lib.rs:10:5\n";
        let errs = parse_errors(sample, 2);
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].line, 10);
        assert!(errs[0].file.contains("lib.rs"));
    }
}