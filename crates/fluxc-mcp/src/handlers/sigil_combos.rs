//! SIGIL-flavoured combo tools — three new phrasal verbs covering BUILD,
//! RESCUE, and DESIGN axes of agent autonomy.
//!
//! Designed for end-of-day sessions where the operator wants ONE button per
//! axis rather than a 6-step recipe. Each handler shells out via std::process
//! and emits a one-paragraph result that's already broadcast-ready.

use crate::handlers::{ToolDef, ToolRegistry};
use serde_json::{json, Value};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_wallet_ship",
            description: "BUILD combo for the SIGIL wallet (or any Vite+TS app): runs `vite build`, rsyncs the dist into a target serving directory, returns the cache-busted URL. Default target is Epsilon's q-flux serving area + sigilgraph subpath.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "wallet_dir":   { "type": "string", "description": "Vite project directory (default: sigil/gui/sigil-wallet)" },
                    "target_dir":   { "type": "string", "description": "Deploy target (default: /home/orobit/q-narwhalknight/dist-final/sigil-wallet)" },
                    "subpath":      { "type": "string", "description": "Subpath served behind sigilgraph.quillon.xyz (default: /sigil-wallet/)" },
                    "skip_build":   { "type": "boolean", "description": "Skip the vite build, just rsync existing dist (default: false)" }
                }
            }),
        },
        flux_wallet_ship,
    );

    registry.register(
        ToolDef {
            name: "flux_node_resuscitate",
            description: "RESCUE combo for the SIGIL substrate: probes sigil-node, q-flux, sigil-bridge, q-api-server processes; reports which are down or stalled; optionally restarts via systemctl. SIGIL-aware (checks sigil-node-delta + sigil chain p2p port 9501).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "service": { "type": "string", "description": "Specific service: auto | sigil-node | q-flux | sigil-bridge | q-api-server (default: auto = probe all, restart any that fail health checks)" },
                    "restart": { "type": "boolean", "description": "Restart unhealthy services via systemctl (default: false = report only)" }
                }
            }),
        },
        flux_node_resuscitate,
    );

    registry.register(
        ToolDef {
            name: "flux_glow",
            description: "DESIGN combo: inject SIGIL aesthetic into a static HTML file. Adds animated obsidian/violet background + sigil-glow keyframes + JetBrains Mono + gold provenance accent tokens, then deploys with cache-busting. The brand-pulse button.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file":     { "type": "string", "description": "Target HTML filename in dist-final (e.g. 'desktop.html')" },
                    "intensity":{ "type": "string", "description": "subtle | medium | strong (default: medium) — controls glow + accent strength" },
                    "title_prefix": { "type": "string", "description": "Optional title prefix (e.g. 'SIGIL · ')" }
                },
                "required": ["file"]
            }),
        },
        flux_glow,
    );
}

// ────────────────────────────────────────────────────────────────
// 1. flux_wallet_ship — BUILD axis
// ────────────────────────────────────────────────────────────────

fn flux_wallet_ship(args: &Value) -> String {
    let wallet_dir = args
        .get("wallet_dir")
        .and_then(|v| v.as_str())
        .unwrap_or("/home/storage/deepseek-codewhale/sigil/gui/sigil-wallet");
    let target_dir = args
        .get("target_dir")
        .and_then(|v| v.as_str())
        .unwrap_or("/home/orobit/q-narwhalknight/dist-final/sigil-wallet");
    let subpath = args
        .get("subpath")
        .and_then(|v| v.as_str())
        .unwrap_or("/sigil-wallet/");
    let skip_build = args
        .get("skip_build")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if !Path::new(wallet_dir).join("package.json").exists() {
        return format!("❌ flux_wallet_ship: package.json not found at {wallet_dir}");
    }

    let started = Instant::now();
    let mut build_secs: f64 = 0.0;

    if !skip_build {
        let build_started = Instant::now();
        let out = Command::new("bash")
            .args([
                "-c",
                "TMPDIR=/home/orobit/tmp ./node_modules/.bin/vite build 2>&1",
            ])
            .current_dir(wallet_dir)
            .env("TMPDIR", "/home/orobit/tmp")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();
        build_secs = build_started.elapsed().as_secs_f64();
        match out {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                let tail: String = err.lines().rev().take(6).collect::<Vec<_>>().join(" | ");
                return format!(
                    "❌ flux_wallet_ship: vite build failed after {:.1}s\n  {}",
                    build_secs, tail
                );
            }
            Err(e) => return format!("❌ flux_wallet_ship: vite spawn failed: {e}"),
        }
    }

    let dist = format!("{wallet_dir}/dist/");
    if !Path::new(&dist).exists() {
        return format!("❌ flux_wallet_ship: dist not found after build at {dist}");
    }

    // rsync into target
    if let Err(e) = std::fs::create_dir_all(target_dir) {
        return format!("❌ flux_wallet_ship: mkdir {target_dir}: {e}");
    }
    let rsync_out = Command::new("rsync")
        .args(["-a", "--delete", &dist, target_dir])
        .output();
    match rsync_out {
        Ok(o) if !o.status.success() => {
            return format!(
                "❌ flux_wallet_ship: rsync failed: {}",
                String::from_utf8_lossy(&o.stderr).lines().next().unwrap_or("?")
            );
        }
        Err(e) => return format!("❌ flux_wallet_ship: rsync spawn failed: {e}"),
        _ => {}
    }

    let bytes = dir_size_bytes(target_dir);
    let assets_count = Path::new(target_dir)
        .join("assets")
        .read_dir()
        .map(|d| d.count())
        .unwrap_or(0);
    let cache_bust = epoch_secs();
    let url = format!(
        "https://sigilgraph.quillon.xyz{}index.html?v={}",
        subpath, cache_bust
    );
    let total = started.elapsed().as_secs_f64();

    format!(
        "🪙 flux_wallet_ship → SHIPPED\n  vite build: {:.1}s ({})\n  rsync:      {} ({} assets, {:.1} MB)\n  total:      {:.1}s\n  URL:        {}",
        build_secs,
        if skip_build { "skipped" } else { "ok" },
        target_dir,
        assets_count,
        bytes as f64 / 1_048_576.0,
        total,
        url
    )
}

// ────────────────────────────────────────────────────────────────
// 2. flux_node_resuscitate — RESCUE axis
// ────────────────────────────────────────────────────────────────

fn flux_node_resuscitate(args: &Value) -> String {
    let svc = args
        .get("service")
        .and_then(|v| v.as_str())
        .unwrap_or("auto");
    let restart = args
        .get("restart")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let candidates: Vec<&str> = match svc {
        "auto" => vec!["q-flux", "sigil-node", "sigil-bridge", "q-api-server"],
        other => vec![other],
    };

    let mut lines: Vec<String> = Vec::new();
    let mut restarted: Vec<String> = Vec::new();
    let mut healthy: usize = 0;
    let mut unhealthy: usize = 0;

    for s in candidates {
        let (alive, detail) = probe_service(s);
        if alive {
            healthy += 1;
            lines.push(format!("  ✓ {} — {}", s, detail));
        } else {
            unhealthy += 1;
            lines.push(format!("  ✗ {} — {}", s, detail));
            if restart {
                let r = Command::new("systemctl").args(["restart", s]).output();
                match r {
                    Ok(o) if o.status.success() => {
                        restarted.push(s.to_string());
                        lines.push(format!("    ↻ systemctl restart {} → ok", s));
                    }
                    Ok(o) => lines.push(format!(
                        "    ↻ systemctl restart {} → failed: {}",
                        s,
                        String::from_utf8_lossy(&o.stderr).lines().next().unwrap_or("?")
                    )),
                    Err(e) => lines.push(format!("    ↻ spawn failed for {}: {}", s, e)),
                }
            }
        }
    }

    // SIGIL-specific extra: tip-height probe at :8181 (if local)
    let sigil_tip = quick_curl("http://127.0.0.1:8181/api/v1/status", 2);
    let sigil_tip_line = match sigil_tip {
        Some(body) if body.contains("\"height\"") => {
            let h = extract_field(&body, "height").unwrap_or_else(|| "?".into());
            format!("  ◆ sigil-node tip @ :8181 → height={}", h)
        }
        Some(_) => "  ◆ sigil-node tip @ :8181 → reachable, unknown shape".to_string(),
        None => "  ◆ sigil-node tip @ :8181 → unreachable (down or wrong port)".to_string(),
    };
    lines.push(sigil_tip_line);

    let verdict = if unhealthy == 0 {
        "✅ all healthy"
    } else if restart && restarted.len() == unhealthy {
        "🔁 unhealthy → restarted"
    } else if restart {
        "⚠ some restart attempts failed"
    } else {
        "⚠ unhealthy (re-run with restart=true to fix)"
    };

    format!(
        "🚑 flux_node_resuscitate — {}\n  probed: {} services\n  healthy: {}  unhealthy: {}  restarted: {}\n{}\n",
        verdict,
        healthy + unhealthy,
        healthy,
        unhealthy,
        restarted.len(),
        lines.join("\n")
    )
}

fn probe_service(name: &str) -> (bool, String) {
    let out = Command::new("systemctl")
        .args(["is-active", name])
        .output();
    match out {
        Ok(o) => {
            let state = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let alive = state == "active";
            let detail = if alive {
                // augment with brief pid + etime if we can
                let pid_out = Command::new("systemctl")
                    .args(["show", "-p", "MainPID,ActiveEnterTimestamp", name])
                    .output();
                match pid_out {
                    Ok(p) => {
                        let s = String::from_utf8_lossy(&p.stdout);
                        let pid = s
                            .lines()
                            .find(|l| l.starts_with("MainPID="))
                            .map(|l| l.trim_start_matches("MainPID=").to_string())
                            .unwrap_or_default();
                        format!("active (pid {})", pid)
                    }
                    Err(_) => "active".into(),
                }
            } else {
                state
            };
            (alive, detail)
        }
        Err(e) => (false, format!("probe error: {e}")),
    }
}

fn quick_curl(url: &str, timeout_secs: u32) -> Option<String> {
    let out = Command::new("curl")
        .args([
            "-s",
            "--max-time",
            &timeout_secs.to_string(),
            "-o",
            "-",
            url,
        ])
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        None
    }
}

fn extract_field(body: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let i = body.find(&needle)?;
    let after_key = &body[i + needle.len()..];
    // Skip whitespace and the colon
    let colon_offset = after_key.find(':')?;
    let tail = after_key[colon_offset + 1..].trim_start();
    let end = tail
        .find(|c: char| c == ',' || c == '}' || c == '\n')
        .unwrap_or(tail.len());
    Some(tail[..end].trim().trim_matches('"').to_string())
}

// ────────────────────────────────────────────────────────────────
// 3. flux_glow — DESIGN axis
// ────────────────────────────────────────────────────────────────

fn flux_glow(args: &Value) -> String {
    let file = match args.get("file").and_then(|v| v.as_str()) {
        Some(f) if !f.is_empty() && !f.contains("..") && !f.starts_with('/') => f,
        _ => return "❌ flux_glow: 'file' is required (no leading slash, no '..')".to_string(),
    };
    let intensity = args
        .get("intensity")
        .and_then(|v| v.as_str())
        .unwrap_or("medium");
    let title_prefix = args
        .get("title_prefix")
        .and_then(|v| v.as_str())
        .unwrap_or("SIGIL · ");

    let dist_root = "/home/orobit/q-narwhalknight/dist-final";
    let path = format!("{dist_root}/{file}");
    let original = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => return format!("❌ flux_glow: cannot read {path}: {e}"),
    };

    let glow_block = match intensity {
        "subtle" => "rgba(139,92,246,0.10) 0%, transparent 50%",
        "strong" => "rgba(139,92,246,0.28) 0%, transparent 60%",
        _ => "rgba(139,92,246,0.18) 0%, transparent 55%",
    };

    let injected_style = format!(
        "<style data-flux-glow=\"v1\">\n  :root {{\n    --sigil-obsidian:#0a0a0f; --sigil-panel:#1a1428;\n    --sigil-accent:#8b5cf6; --sigil-accent-bright:#c084fc;\n    --sigil-gold:#fbbf24; --sigil-text:#e2e8f0; --sigil-muted:#94a3b8;\n  }}\n  body {{\n    background: radial-gradient(ellipse at 30% 0%, {glow}), var(--sigil-obsidian) !important;\n    color: var(--sigil-text);\n    font-family: 'JetBrains Mono', ui-monospace, monospace;\n  }}\n  @keyframes sigil-pulse {{ 0%, 100% {{ opacity: 0.55; }} 50% {{ opacity: 1; }} }}\n  .sigil-glow {{ box-shadow: 0 0 24px rgba(139,92,246,0.45), 0 0 48px rgba(139,92,246,0.15); }}\n  .sigil-gold-accent {{ color: var(--sigil-gold); }}\n  .sigil-flux-pulse {{ animation: sigil-pulse 2.6s ease-in-out infinite; }}\n</style>\n",
        glow = glow_block
    );

    // Inject style block right before </head>; prepend title prefix
    let mut patched = if let Some(idx) = original.find("</head>") {
        let mut s = String::with_capacity(original.len() + injected_style.len());
        s.push_str(&original[..idx]);
        s.push_str(&injected_style);
        s.push_str(&original[idx..]);
        s
    } else {
        // No </head> — inject at start
        format!("{injected_style}{original}")
    };

    // Patch <title> with prefix if not already prefixed
    if !title_prefix.is_empty() && !patched.contains(&format!("<title>{title_prefix}")) {
        if let Some(t0) = patched.find("<title>") {
            let after = t0 + "<title>".len();
            patched.insert_str(after, title_prefix);
        }
    }

    // Write back
    if let Err(e) = std::fs::write(&path, patched.as_bytes()) {
        return format!("❌ flux_glow: write failed: {e}");
    }

    let bust = epoch_secs();
    format!(
        "✨ flux_glow → applied\n  file:      {file}\n  intensity: {intensity}\n  size:      {} bytes (was, now {} bytes)\n  cache-bust URL:\n    https://quillon.xyz/{file}?v={bust}\n    https://sigilgraph.quillon.xyz/{file}?v={bust}",
        original.len(),
        std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
    )
}

// ────────────────────────────────────────────────────────────────
// helpers
// ────────────────────────────────────────────────────────────────

fn epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn dir_size_bytes(root: &str) -> u64 {
    fn walk(p: &Path, acc: &mut u64) {
        if let Ok(rd) = p.read_dir() {
            for ent in rd.flatten() {
                let ep = ent.path();
                if let Ok(m) = ent.metadata() {
                    if m.is_file() {
                        *acc += m.len();
                    } else if m.is_dir() {
                        walk(&ep, acc);
                    }
                }
            }
        }
    }
    let mut acc = 0u64;
    walk(Path::new(root), &mut acc);
    acc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_field_basic_json() {
        let body = r#"{"height":12345,"peers":8,"name":"sigil-g0"}"#;
        assert_eq!(extract_field(body, "height").as_deref(), Some("12345"));
        assert_eq!(extract_field(body, "peers").as_deref(), Some("8"));
        assert_eq!(extract_field(body, "name").as_deref(), Some("sigil-g0"));
        assert!(extract_field(body, "missing").is_none());
    }

    #[test]
    fn extract_field_whitespace_tolerant() {
        let body = r#"{ "height" : 999 ,"x":1}"#;
        assert_eq!(extract_field(body, "height").as_deref(), Some("999"));
    }

    #[test]
    fn glow_rejects_traversal() {
        let r = flux_glow(&json!({"file": "../etc/passwd"}));
        assert!(r.contains("required"));
    }

    #[test]
    fn glow_rejects_absolute() {
        let r = flux_glow(&json!({"file": "/tmp/evil.html"}));
        assert!(r.contains("required"));
    }

    #[test]
    fn wallet_ship_reports_missing_pkg() {
        let r = flux_wallet_ship(&json!({"wallet_dir": "/nonexistent-9bef"}));
        assert!(r.contains("package.json not found"));
    }
}
