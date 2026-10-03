// flux_compile_pulse — the compile-cost sense, as an MCP tool.
//
// Every fluxc build already judges itself (fluxc_core::pulse); this exposes
// the four questions an agent should be able to ask BEFORE compile time has
// crept up for a month:
//   last  — what did the build I just ran cost, vs what that work usually costs?
//   drift — what has been getting slower over days (the creep)?
//   blast — if I edit crate X, how much rebuilds, and what's the wall floor?
//   deps  — what does my Cargo.lock change bring in, and what does it cost?

use crate::handlers::{ToolDef, ToolRegistry};
use fluxc_core::pulse;
use serde_json::{json, Value};

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_compile_pulse",
            description: "Compile-cost consequences, measured on every fluxc build. view=last (default): the newest build's verdict — steady/heavier/lighter, CPU vs that work's rolling baseline, regressions with source growth, NEW crates (= new dependencies), blast (local crates rebuilt), peak rustc RSS. view=drift: units getting slower over days (the creep). view=blast crate=X: what editing X rebuilds + CPU + critical-path wall floor, BEFORE you edit. view=deps: Cargo.lock vs git rev — added packages and their known compile cost. Works for any workspace built through fluxc (flux, sigil, quillon).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "view": {"type": "string", "enum": ["last", "drift", "blast", "deps"], "description": "Which question (default last)"},
                    "crate": {"type": "string", "description": "view=blast: the crate you are about to edit"},
                    "workspace": {"type": "string", "description": "view=blast|deps: workspace root (default: the flux workspace)"},
                    "mode": {"type": "string", "enum": ["build", "check"], "description": "view=blast: cost of codegen builds (default) or cargo check"},
                    "opt": {"type": "string", "description": "view=blast: restrict to one opt-level, e.g. \"0\", \"1\", \"3+lto\""},
                    "days": {"type": "integer", "description": "view=drift: history window in days (default 28)"},
                    "recent": {"type": "integer", "description": "view=drift: 'recent' window in days (default 7)"},
                    "top": {"type": "integer", "description": "view=drift: rows per direction (default 12)"},
                    "rev": {"type": "string", "description": "view=deps: git rev to diff Cargo.lock against (default HEAD)"},
                    "json": {"type": "boolean", "description": "Raw JSON instead of the rendered report"}
                }
            }),
        },
        flux_compile_pulse,
    );
}

fn default_workspace() -> std::path::PathBuf {
    std::env::var("FLUX_WORKSPACE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/home/storage/deepseek-codewhale/flux"))
}

fn flux_compile_pulse(args: &Value) -> String {
    let view = args.get("view").and_then(|v| v.as_str()).unwrap_or("last");
    let as_json = args.get("json").and_then(|v| v.as_bool()).unwrap_or(false);
    let ws = args
        .get("workspace")
        .and_then(|v| v.as_str())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(default_workspace);
    let num = |k: &str, d: u64| args.get(k).and_then(|v| v.as_u64()).unwrap_or(d);
    match view {
        "drift" => {
            let r = pulse::drift(num("days", 28), num("recent", 7), num("top", 12) as usize);
            if as_json { serde_json::to_string_pretty(&r).unwrap_or_default() } else { pulse::render_drift(&r) }
        }
        "blast" => {
            let Some(c) = args.get("crate").and_then(|v| v.as_str()) else {
                return "✗ view=blast needs crate=<name> (the crate you are about to edit)".into();
            };
            let mode = args.get("mode").and_then(|v| v.as_str()).unwrap_or("build");
            let opt = args.get("opt").and_then(|v| v.as_str());
            match pulse::blast_radius(&ws, c, mode, opt) {
                Ok(b) if as_json => serde_json::to_string_pretty(&b).unwrap_or_default(),
                Ok(b) => pulse::render_blast(&b),
                Err(e) => format!("✗ {}", e),
            }
        }
        "deps" => {
            let rev = args.get("rev").and_then(|v| v.as_str()).unwrap_or("HEAD");
            match pulse::dep_diff(&ws, rev) {
                Ok(d) if as_json => serde_json::to_string_pretty(&d).unwrap_or_default(),
                Ok(d) => pulse::render_dep_diff(&d, rev),
                Err(e) => format!("✗ {}", e),
            }
        }
        _ => match pulse::read_last() {
            Some(p) if as_json => serde_json::to_string_pretty(&p).unwrap_or_default(),
            Some(p) => {
                let age = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
                    .saturating_sub(p.ts);
                let mut out = vec![
                    format!("⏱ {}", p.headline),
                    format!("  {} {} · {} · {}s ago · inv {}", p.cmd, p.package,
                        if p.release { "release" } else { "debug" }, age, p.inv),
                ];
                out.extend(pulse::render_rows(&p));
                if !p.top.is_empty() {
                    out.push("  heaviest units:".into());
                    for t in &p.top {
                        out.push(format!(
                            "    {:<36} {:>7}{}",
                            t.unit,
                            pulse::fmt_ms(t.cpu_ms),
                            if t.typical_ms > 0 { format!("  (typ {})", pulse::fmt_ms(t.typical_ms)) } else { String::new() }
                        ));
                    }
                }
                out.join("\n")
            }
            None => "⏱ no pulse yet — it appears after the first fluxc build/check/test with a pulse-aware binary".into(),
        },
    }
}

/// Combo integration: the rows a combo panel appends, plus a NEXT hint when
/// the verdict says something got heavier.
pub fn combo_rows(p: &pulse::Pulse) -> Vec<String> {
    let mut rows = pulse::render_rows(p);
    if p.verdict == "heavier" {
        let who = p
            .regressions
            .first()
            .map(|r| r.unit.split([' ', ':']).next().unwrap_or("").to_string())
            .or_else(|| p.local_rebuilt.first().map(|c| c.unit.split(' ').next().unwrap_or("").to_string()))
            .unwrap_or_default();
        rows.push(if who.is_empty() {
            " NEXT      flux_compile_pulse view=drift".to_string()
        } else {
            format!(" NEXT      flux_compile_pulse view=blast crate={}", who)
        });
    }
    rows
}
