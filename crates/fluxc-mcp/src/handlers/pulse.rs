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
            description: "Compile-cost consequences, measured on every fluxc build. view=last (default): the newest build's verdict — steady/heavier/lighter, CPU vs that work's rolling baseline, regressions with source growth, NEW crates (= new dependencies), blast (local crates rebuilt), peak rustc RSS. view=drift: units getting slower over days (the creep). view=tax: where development compile CPU actually went over N days (rebuild count × cost — finds the cheap-but-hot crate). view=blast crate=X: what editing X rebuilds + CPU + critical-path wall floor, BEFORE you edit. view=deps: Cargo.lock vs git rev — added packages and their known compile cost. view=cost dep=<crate>[@ver]: what ADDING a dependency would bring in and cost, before you add it. Works for any workspace built through fluxc (flux, sigil, quillon).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "view": {"type": "string", "enum": ["last", "drift", "tax", "blast", "deps", "cost"], "description": "Which question (default last). tax = where development CPU went (builds × cost per unit)"},
                    "crate": {"type": "string", "description": "view=blast: the crate you are about to edit"},
                    "workspace": {"type": "string", "description": "view=blast|deps: workspace root (default: the flux workspace)"},
                    "mode": {"type": "string", "enum": ["build", "check"], "description": "view=blast: cost of codegen builds (default) or cargo check"},
                    "opt": {"type": "string", "description": "view=blast: restrict to one opt-level, e.g. \"0\", \"1\", \"3+lto\""},
                    "days": {"type": "integer", "description": "view=drift: history window in days (default 28)"},
                    "recent": {"type": "integer", "description": "view=drift: 'recent' window in days (default 7)"},
                    "top": {"type": "integer", "description": "view=drift: rows per direction (default 12)"},
                    "rev": {"type": "string", "description": "view=deps: git rev to diff Cargo.lock against (default HEAD)"},
                    "all": {"type": "boolean", "description": "view=tax: include registry dependencies, not just your crates"},
                    "dep": {"type": "string", "description": "view=cost: crate to price, e.g. \"reqwest\" or \"wgpu@0.17\""},
                    "features": {"type": "array", "items": {"type": "string"}, "description": "view=cost: features to enable on the candidate"},
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
        "tax" => {
            let all = args.get("all").and_then(|v| v.as_bool()).unwrap_or(false);
            let r = pulse::rebuild_tax(num("days", 7), num("top", 15) as usize, !all);
            if as_json { serde_json::to_string_pretty(&r).unwrap_or_default() } else { pulse::render_tax(&r) }
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
        "cost" => {
            let Some(dep) = args.get("dep").and_then(|v| v.as_str()) else {
                return "✗ view=cost needs dep=<crate>[@version]".into();
            };
            let feats: Vec<String> = args.get("features").and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
            match pulse::dep_cost(&ws, dep, &feats) {
                Ok(c) if as_json => serde_json::to_string_pretty(&c).unwrap_or_default(),
                Ok(c) => pulse::render_dep_cost(&c),
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

/// Workspaces the pre-edit blast footer knows how to resolve, tried in order.
fn blast_workspaces() -> Vec<std::path::PathBuf> {
    let mut v = vec![crate::handlers::ws()];
    for extra in ["/home/storage/deepseek-codewhale/sigil"] {
        let p = std::path::PathBuf::from(extra);
        if p.join("Cargo.toml").exists() && !v.contains(&p) {
            v.push(p);
        }
    }
    v
}

/// One-line consequence for a planning/editing tool that names a crate:
/// how much an edit to it rebuilds, judged from measured baselines.
pub fn blast_footer(crate_name: &str) -> Option<String> {
    for ws in blast_workspaces() {
        if let Ok(b) = pulse::blast_radius(&ws, crate_name, "build", None) {
            if b.known == 0 {
                if let Ok(c) = pulse::blast_radius(&ws, crate_name, "check", None) {
                    if c.known > 0 {
                        return Some(footer_line(&c));
                    }
                }
            }
            return Some(footer_line(&b));
        }
    }
    None
}

fn footer_line(b: &pulse::Blast) -> String {
    format!(
        "💥 consequence: editing {} rebuilds {} crate(s) ≈ {} CPU, critical path ≈ {} [{} mode, {}/{} measured] — flux_compile_pulse view=blast crate={}",
        b.target,
        b.crates.len(),
        pulse::fmt_ms(b.cpu_ms),
        pulse::fmt_ms(b.critical_path_ms),
        b.mode,
        b.known,
        b.crates.len(),
        b.target
    )
}

/// Measured compile pressure for the cortex: the tax table + the newest
/// verdict, so planning starts from what builds actually cost.
pub fn measured_compile_pressure() -> serde_json::Value {
    let tax = pulse::rebuild_tax(7, 8, true);
    json!({
        "source": "fluxc pulse — measured rustc CPU per unit (getrusage), not estimated",
        "window_days": tax.days,
        "units_compiled": tax.units_compiled,
        "cpu_total_ms": tax.cpu_total_ms,
        "cpu_local_ms": tax.cpu_local_ms,
        "top_local_units": tax.rows.iter().map(|r| json!({
            "unit": r.unit, "builds": r.builds, "median_ms": r.median_ms,
            "total_ms": r.cpu_total_ms, "src_bytes": r.src_bytes
        })).collect::<Vec<_>>(),
        "last_verdict": pulse::read_last().map(|p| p.headline),
    })
}
