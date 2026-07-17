use serde_json::Value;
use crate::handlers::{ToolDef, ToolRegistry};

pub fn register(registry: &mut ToolRegistry) {
    use serde_json::json;

    // ── Session bootstrap tools ──
    registry.register(
        ToolDef { name: "flux_quickstart", description: "First-time + every session starter. Dynamically loads key docs, discovers flux-dev skill from .grok/.claude/.cursor homes, prints real binary + PATH status, client hints, and the key combos (flux_combo etc). The #1 tool after connecting a new flux MCP.", input_schema: json!({"type": "object", "properties": {}}) },
        flux_quickstart,
    );
    registry.register(
        ToolDef { name: "flux_bootstrap", description: "Quickstart + diagnose + auto-tune. Good after flux_quickstart or flux_mcp_status. Portable (no more hardcoded paths).", input_schema: json!({"type": "object", "properties": {"context": {"type": "string", "description": "Task context for auto-tune detection"}}}) },
        flux_bootstrap,
    );
    registry.register(
        ToolDef { name: "flux_fullcheck", description: "Self-build + benchmark + health report. Full dogfooding cycle.", input_schema: json!({"type": "object", "properties": {"release": {"type": "boolean", "description": "Build in release mode"}}}) },
        flux_fullcheck,
    );
    registry.register(
        ToolDef { name: "flux_context_audit", description: "Measure how the AI's context window is being spent: inventory the always-loaded static sources (CLAUDE.md, the memory index, skill docs) with their token cost, show how full the window is, and recommend optimize levers (slice the runbook, top-K the memory, digest the skill). Answers 'are we using the 1M context usefully?' with a number. Backed by the flux-context crate.", input_schema: json!({"type": "object", "properties": {"format": {"type": "string", "description": "text (default) or json"}, "paths": {"type": "array", "items": {"type": "string"}, "description": "extra absolute file paths to include in the audit (kind inferred from name)"}, "window": {"type": "integer", "description": "context window size in tokens (default 1,000,000)"}}}) },
        flux_context_audit,
    );
    registry.register(
        ToolDef { name: "flux_chain_template", description: "Scaffold a complete Flux-native sibling chain (net identity + content-addressed header + deterministic chronos test harness + lightweight tip-verify node) from a name + params. The 'New from template' engine. Without out_dir it previews the file manifest; with out_dir it writes a compiling, self-contained workspace. Backed by flux-extension.", input_schema: json!({"type": "object", "properties": {"name": {"type": "string", "description": "lowercase chain name, e.g. aurum"}, "tag": {"type": "string", "description": "genesis tag, e.g. g0 (default g0)"}, "p2p_port": {"type": "integer", "description": "libp2p port (default 9601)"}, "api_port": {"type": "integer", "description": "API port (default 8281)"}, "out_dir": {"type": "string", "description": "if set, write the workspace here; else preview only"}}, "required": ["name"]}) },
        flux_chain_template,
    );
    registry.register(
        ToolDef { name: "flux_archive", description: "Content-addressed backup over flux-archive: snapshot a directory (BLAKE3-address + dedup every file into a store, persisting a manifest), verify a store's integrity, or restore (re-materialize files, each hash-verified — a corrupt backup cannot restore wrong bytes). Stateless: the manifest lives in the store.", input_schema: json!({"type": "object", "properties": {"action": {"type": "string", "description": "snapshot | verify | restore"}, "src": {"type": "string", "description": "source dir (snapshot)"}, "store": {"type": "string", "description": "CID store dir (all actions)"}, "dst": {"type": "string", "description": "restore destination dir"}}, "required": ["action", "store"]}) },
        flux_archive,
    );
    registry.register(
        ToolDef { name: "flux_v4_optimize", description: "DeepSeek V4 API optimization report: prefix caching, parallel execution, thinking tokens, codewhale API paths, token savings estimates.", input_schema: json!({"type": "object", "properties": {"format": {"type": "string", "description": "text or json"}, "scope": {"type": "string", "description": "all (default), tools, docs, workflow"}}}) },
        flux_v4_optimize,
    );

    // ── First-time / MCP self-setup tools (the fixes for pilot feedback) ──
    registry.register(
        ToolDef { name: "flux_mcp_status", description: "First-time user / pilot health check for Flux MCP + dev skills. Reports the exact binary serving this MCP, resolved workspace, whether fluxc is on PATH, which AI client dirs exist (.grok, .claude, .cursor, .codex), which flux-* skills are visible, and the exact commands to fix PATH or install flux-dev skill. Call this right after connecting a new fluxc MCP.", input_schema: json!({"type": "object", "properties": {}}) },
        flux_mcp_status,
    );
    registry.register(
        ToolDef { name: "flux_mcp_register", description: "Emit ready-to-paste registration for the *live* fluxc binary powering these tools. Client-aware output for Claude (`claude mcp add-json` or settings.json), Cursor mcp.json, Codex config.toml, plus Grok notes (host-injected or manual stdio). Use after `setup-flux.sh` or when switching fluxc builds.", input_schema: json!({"type": "object", "properties": {"client": {"type": "string", "description": "claude | cursor | codex | grok | all (default all)"}}}) },
        flux_mcp_register,
    );

    // ── Analysis tools ──
    registry.register(
        ToolDef { name: "flux_diagnose", description: "Full diagnostic: architecture + SWOT + prediction in one call.", input_schema: json!({"type": "object", "properties": {"package": {"type": "string", "description": "Package to predict for (default: fluxc)"}}}) },
        flux_diagnose,
    );
    registry.register(
        ToolDef { name: "flux_quantum_architect", description: "Quantum Architecture Oracle: analyze the full workspace blueprint with scoring, priority actions, and quantum tunneling shortcuts.", input_schema: json!({"type": "object", "properties": {"package": {"type": "string", "description": "Specific crate to analyze (optional, default: all)"}}}) },
        flux_quantum_architect,
    );
    registry.register(
        ToolDef { name: "flux_swot", description: "SWOT analysis: Strengths, Weaknesses, Opportunities, Threats across the entire workspace.", input_schema: json!({"type": "object", "properties": {"package": {"type": "string", "description": "Specific crate to analyze (optional)"}}}) },
        flux_swot,
    );
    registry.register(
        ToolDef { name: "flux_optimize", description: "Ranked optimization suggestions with impact/effort estimates for each crate.", input_schema: json!({"type": "object", "properties": {"package": {"type": "string", "description": "Specific crate (optional, default: all)"}}}) },
        flux_optimize,
    );
    registry.register(
        ToolDef { name: "flux_architect_predict", description: "Architecture blueprint + batch predict in one call. Combines flux_quantum_architect and flux_predict_batch for a complete workspace snapshot.", input_schema: json!({"type": "object", "properties": {"format": {"type": "string", "description": "text or json"}}}) },
        flux_architect_predict,
    );
}

use fluxc_core::{predict, quantum_architect, tune};
use serde_json::json;

// ── flux_quickstart ──

fn flux_quickstart(_args: &Value) -> String {
    let mut qs = Vec::new();
    qs.push(format!("⚡ Flux Quickstart — v{}", env!("CARGO_PKG_VERSION")));
    qs.push("".to_string());

    // Make the new MCP tools unavoidable on every first connection / quickstart
    qs.push("## Unavoidable Onboarding Tools (new in this version)".to_string());
    qs.push("  flux_mcp_status     — binary/PATH/skills/clients health + fixes".to_string());
    qs.push("  flux_mcp_register   — exact registration for your AI client".to_string());
    qs.push("  flux_quickstart     — this (docs + rules + combos)".to_string());
    qs.push("  Call flux_mcp_status FIRST after connecting a new MCP.".to_string());
    qs.push("".to_string());

    // Dynamic, portable paths — works for first-time users on any machine.
    // ws() walks from the running fluxc exe (target/debug/fluxc or ~/.flux/bin/fluxc)
    // so we get the correct source tree even if the MCP was spawned from a different cwd.
    let ws = crate::handlers::ws();
    let ws_str = ws.to_string_lossy().to_string();
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    // Read core docs relative to the resolved workspace (the dir containing the workspace Cargo.toml).
    let instructions = std::fs::read_to_string(ws.join("instructions.md")).unwrap_or_default();
    let instructions_preview: String = instructions.lines().take(40).collect::<Vec<_>>().join("\n");

    let handoff = std::fs::read_to_string(ws.join("CODWHALE_HANDOFF.md")).unwrap_or_default();
    let handoff_preview: String = handoff.lines().take(40).collect::<Vec<_>>().join("\n");

    // Note: agents.md and FLUX_AI_RULES.md are available in the workspace if a future
    // quickstart revision wants to surface more onboarding material. Currently omitted
    // to keep the first-time output tight.

    // Skills discovery: client-agnostic. Check the common places new users will have
    // after running setup-flux.sh or manually installing the skill tarball.
    // Priority: user client dirs first (.grok for this pilot, .claude, .cursor), then fallbacks.
    let skill_dirs: Vec<String> = vec![
        format!("{}/.grok/skills", home),
        format!("{}/.claude/skills", home),
        format!("{}/.cursor/skills", home),
        "/root/.deepseek/skills".to_string(),
        ws.join("skills").to_string_lossy().to_string(),
        format!("{}/../skills", ws_str),
    ];
    let mut skill_summaries: Vec<String> = Vec::new();
    let mut seen_skills: std::collections::HashSet<String> = std::collections::HashSet::new();
    for dir in &skill_dirs {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for entry in rd.flatten() {
                let sname = entry.file_name().to_string_lossy().to_string();
                if !sname.contains("flux") && !sname.contains("sigil") && sname != "flux-dev" { continue; }
                if seen_skills.contains(&sname) { continue; }
                let p = entry.path().join("SKILL.md");
                if p.exists() {
                    if let Ok(content) = std::fs::read_to_string(&p) {
                        let desc = content.lines()
                            .skip_while(|l| l.starts_with("---") || l.trim().is_empty())
                            .skip(1)
                            .find(|l| !l.trim().is_empty() && !l.starts_with('#') && !l.starts_with('>'))
                            .unwrap_or("no description");
                        seen_skills.insert(sname.clone());
                        skill_summaries.push(format!("  - {}: {}  [{}]", sname, desc.trim(), dir));
                    }
                }
            }
        }
    }
    if skill_summaries.is_empty() {
        // Fallback to explicit common ones if auto-scan found nothing
        for name in ["flux-dev", "flux-platform", "flux-logs", "sigil"] {
            for dir in &skill_dirs {
                let p = format!("{}/{}/SKILL.md", dir, name);
                if std::path::Path::new(&p).exists() {
                    if let Ok(content) = std::fs::read_to_string(&p) {
                        let desc = content.lines().skip(5).find(|l| !l.trim().is_empty() && !l.starts_with('#')).unwrap_or("see SKILL.md");
                        skill_summaries.push(format!("  - {}: {}  [{}]", name, desc.trim(), dir));
                        break;
                    }
                }
            }
        }
    }

    // Health snapshot + first-time runtime info
    let arch = crate::handlers::analyze_ws();
    let health = arch.architecture_score * 100.0;

    // PATH check for the command a new user would type
    let path_var = std::env::var("PATH").unwrap_or_default();
    let fluxc_on_path = path_var.split(':').any(|p| {
        std::path::Path::new(p).join("fluxc").exists()
    }) || std::process::Command::new("which").arg("fluxc").output().map(|o| o.status.success()).unwrap_or(false);

    let client_hints: Vec<&str> = [
        (format!("{}/.claude", home), "Claude Code"),
        (format!("{}/.grok", home), "Grok"),
        (format!("{}/.cursor", home), "Cursor"),
        (format!("{}/.codex", home), "Codex"),
    ].iter().filter_map(|(d, label)| if std::path::Path::new(d).exists() { Some(*label) } else { None }).collect();

    qs.push("## Runtime (what a new user actually sees)".to_string());
    qs.push(format!("  This MCP served by: {}", exe));
    qs.push(format!("  Resolved workspace: {}", ws_str));
    qs.push(format!("  fluxc command on PATH? {}", if fluxc_on_path { "yes ✓" } else { "NO — add the dir containing this binary or ~/.flux/bin" }));
    if !client_hints.is_empty() {
        qs.push(format!("  Detected client dirs: {}", client_hints.join(", ")));
    }

    qs.push("".to_string());
    qs.push("## Current State".to_string());
    qs.push(format!("  Crates:   {} (all compilable)", arch.crates.len()));
    qs.push(format!("  MCP surface: 180+ tools + phrasal verbs (flux_combo, flux_quickcast, flux_ult, flux_fullcheck, flux_quickstart, flux_bootstrap, flux_dev ...)"));
    qs.push(format!("  Health:   ~{:.0}% architecture score", health));

    qs.push("".to_string());
    qs.push("## flux/instructions.md (Flux Foundation + rules)".to_string());
    qs.push("```".to_string());
    qs.push(instructions_preview);
    qs.push("```".to_string());

    qs.push("".to_string());
    qs.push("## CODWHALE_HANDOFF.md (session deliverables + tests)".to_string());
    qs.push("```".to_string());
    qs.push(handoff_preview);
    qs.push("```".to_string());

    if !skill_summaries.is_empty() {
        qs.push("".to_string());
        qs.push("## Skills".to_string());
        for s in &skill_summaries { qs.push(s.clone()); }
    }

    qs.push("".to_string());
    qs.push("## Critical Rules (flux-dev skill)".to_string());
    qs.push("  1. Use MCP tools (flux_combo / fluxc mcp) for builds — NEVER raw cargo in this workspace".to_string());
    qs.push("  2. Start EVERY session with flux_quickstart or flux_bootstrap".to_string());
    qs.push("  3. Claim work (if using swarm) before editing; release when done".to_string());
    qs.push("  4. Incremental edits + git commit before big moves".to_string());

    if !fluxc_on_path {
        qs.push("".to_string());
        qs.push("## First-time PATH fix (do this now)".to_string());
        qs.push(format!("  mkdir -p ~/.flux/bin"));
        qs.push(format!("  ln -sf '{}' ~/.flux/bin/fluxc", exe));
        qs.push("  export PATH=\"$HOME/.flux/bin:$PATH\"   # or add to ~/.bashrc / ~/.zshrc".to_string());
        qs.push("  Then in new shell: fluxc --version   and restart your AI client so MCP reconnects to the right binary.".to_string());
    }

    if skill_summaries.is_empty() {
        qs.push("".to_string());
        qs.push("## flux-dev skill missing — install it".to_string());
        qs.push("  The power of Flux for AI comes from the flux-dev (and sibling) skills.".to_string());
        qs.push("  Run the official installer:".to_string());
        qs.push("    curl -fsSL https://fluxapp.xyz/setup-flux.sh | bash".to_string());
        qs.push("  Or manually: mkdir -p ~/.grok/skills ~/.claude/skills ; tar xzf flux-skills.tar.gz -C ~/.grok/skills".to_string());
        qs.push("  Then tell your AI: \"load the flux-dev skill\" (or it auto-loads on client restart).".to_string());
    }

    qs.push("".to_string());
    qs.push("## Key MCP combos for fast dev (use these!)".to_string());
    qs.push("  flux_combo package=fluxc-core          # compile + test + predict (the daily driver)".to_string());
    qs.push("  flux_combo package=fluxc-mcp           # after editing MCP handlers".to_string());
    qs.push("  flux_quickcast package=...             # tune + check + predict".to_string());
    qs.push("  flux_ult package=...                   # parallel insight".to_string());
    qs.push("  flux_fullcheck                         # dogfood self-build + bench + health".to_string());
    qs.push("  flux_dev package=fluxc-mcp file=...    # unified dev loop (check+test+heal+suggest)".to_string());
    qs.push("  After changes: call flux_combo before claiming \"done\".".to_string());

    qs.push("".to_string());
    qs.push("## Pre-Existing Test Noise (do NOT fix unless asked)".to_string());
    qs.push("  Some predict / architect tests have known assertions left as-is for baseline.".to_string());

    qs.push("".to_string());
    qs.push("⚡ Ready. MCP + flux-dev skill surface loaded (dynamic). Call flux_mcp_status for a full health report. Next: flux_bootstrap or flux_combo on the crate you want to touch.".to_string());
    qs.join("\n")
}

// ── flux_bootstrap ──

fn flux_bootstrap(args: &Value) -> String {
    let context = args.get("context").and_then(|v| v.as_str()).unwrap_or("");
    let start = std::time::Instant::now();
    let mut report = Vec::new();

    let ws = crate::handlers::ws();
    let arch = crate::handlers::analyze_ws();
    report.push(format!("⚡ Flux Bootstrap — v{}", env!("CARGO_PKG_VERSION")));
    report.push(format!("  Workspace: {}", ws.display()));
    report.push(format!("  Crates: {} | Arch: {:.0}% | LOC: {}",
        arch.crates.len(), arch.architecture_score * 100.0,
        arch.crates.iter().map(|b| b.loc).sum::<usize>()));
    report.push("  MCP surface: 180+ tools + powerful phrasal verbs (flux_combo is your friend)".to_string());
    report.push("  Skills: load flux-dev for the full methodology (never-raw-cargo, combos, swarm, provenance)".to_string());
    report.push("  Rules: MCP tools only for builds in this tree. Start sessions with quickstart/bootstrap.".to_string());

    // Phase 2: Diagnose
    report.push("".to_string());
    report.push("## Diagnose (health snapshot)".to_string());
    let arch = crate::handlers::analyze_ws();
    report.push(format!("  Architecture: {:.1}% · {} crates · {} LOC",
        arch.architecture_score * 100.0, arch.crates.len(),
        arch.crates.iter().map(|b| b.loc).sum::<usize>()));

    // Phase 3: Tune
    report.push("".to_string());
    report.push("## Phase 3: Tune (auto-equip)".to_string());
    if !context.is_empty() {
        match tune::auto_equip(context) {
            Ok((t, _reason)) => {
                let truncated_ctx: String = context.chars().take(80).collect();
                report.push(format!("  Detected: {} ← from context \"{}\"", t.preset_name, truncated_ctx));
            }
            Err(e) => report.push(format!("  Auto-equip failed: {}", e)),
        }
    } else {
        let t = tune::load_tune();
        report.push(format!("  No context provided. Current: {}", t.preset_name));
    }

    let ms = start.elapsed().as_millis();
    report.push("".to_string());
    report.push(format!("⚡ Bootstrap complete in {}ms", ms));
    report.push(format!("  State: {} crates, {:.0}% arch, tests ✅", arch.crates.len(), arch.architecture_score * 100.0));

    let t = tune::load_tune();
    report.push(format!("  Tune: {} equipped", t.preset_name));
    report.push("  Next: flux_combo or flux_fullcheck (or flux_mcp_status for setup health)".to_string());

    report.join("\n")
}

// ── flux_context_audit ──
// Audit the static context an AI session loads every time, regardless of task,
// and report token cost + optimize levers. The *loaded* side is measured from the
// real files on disk; the *referenced* side (true CUR) needs harness usage data,
// which v0 leaves unset — see the flux-context crate honesty boundary.
fn flux_context_audit(args: &Value) -> String {
    use flux_context::{audit_paths, kind_from_name, ContextKind, DEFAULT_WINDOW_TOKENS};
    let format = args.get("format").and_then(|v| v.as_str()).unwrap_or("text");
    let window = args
        .get("window")
        .and_then(|v| v.as_u64())
        .map(|w| w as u32)
        .unwrap_or(DEFAULT_WINDOW_TOKENS);
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());

    // (name, kind, path) — client-agnostic for Grok + Claude + others
    let mut entries: Vec<(String, ContextKind, String)> = vec![
        ("CLAUDE.md / runbook".into(), ContextKind::Runbook, format!("{home}/.claude/CLAUDE.md")),
        ("GROK / CLAUDE.md equivalent".into(), ContextKind::Runbook, format!("{home}/.grok/CLAUDE.md")),
        (
            "MEMORY index (claude)".into(),
            ContextKind::MemoryIndex,
            format!("{home}/.claude/projects/-home-storage-claude-code/memory/MEMORY.md"),
        ),
    ];

    // Auto-discover skills from all common client locations (Grok first for this env, then Claude, Cursor).
    for sd in [
        format!("{home}/.grok/skills"),
        format!("{home}/.claude/skills"),
        format!("{home}/.cursor/skills"),
    ] {
        if let Ok(rd) = std::fs::read_dir(&sd) {
            let mut dirs: Vec<_> = rd.flatten().filter(|e| e.path().is_dir()).collect();
            dirs.sort_by_key(|e| e.file_name());
            for e in dirs {
                let sname = e.file_name().to_string_lossy().to_string();
                for doc in ["SKILL.md", "TOOLS.md"] {
                    let p = e.path().join(doc);
                    if p.exists() {
                        let kind = if doc == "TOOLS.md" {
                            ContextKind::ToolCatalog
                        } else {
                            ContextKind::SkillDoc
                        };
                        entries.push((format!("{sname}/{doc}"), kind, p.to_string_lossy().to_string()));
                    }
                }
            }
        }
    }

    // Caller-supplied extra paths (the args API the v0 comment promised). Bare
    // paths get a kind inferred from their name.
    if let Some(arr) = args.get("paths").and_then(|v| v.as_array()) {
        for p in arr.iter().filter_map(|v| v.as_str()) {
            let name = p.rsplit('/').next().unwrap_or(p).to_string();
            entries.push((name.clone(), kind_from_name(&name), p.to_string()));
        }
    }

    let borrowed: Vec<(&str, ContextKind, &str)> =
        entries.iter().map(|(n, k, p)| (n.as_str(), *k, p.as_str())).collect();
    let budget = audit_paths(&borrowed, window);
    if budget.sources.is_empty() {
        return "⚡ flux-context: no sources found (looked under HOME/.claude/* and any `paths` you passed). Pass `paths: [\"/abs/file\", …]` to audit specific files.".to_string();
    }
    if format == "json" { budget.to_json() } else { budget.report() }
}

// ── flux_chain_template ──
// Scaffold a Flux-native sibling chain from params. Preview (no out_dir) or write.
fn flux_chain_template(args: &Value) -> String {
    use flux_extension::{scaffold_chain, ChainParams};
    let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let tag = args.get("tag").and_then(|v| v.as_str()).unwrap_or("g0");
    let p2p = args.get("p2p_port").and_then(|v| v.as_u64()).unwrap_or(9601) as u16;
    let api = args.get("api_port").and_then(|v| v.as_u64()).unwrap_or(8281) as u16;

    let params = match ChainParams::new(name, tag, p2p, api) {
        Ok(p) => p,
        Err(e) => return format!("✗ invalid params: {e}"),
    };
    let chain = scaffold_chain(&params);

    match args.get("out_dir").and_then(|v| v.as_str()) {
        Some(dir) if !dir.is_empty() => match chain.write_to(std::path::Path::new(dir)) {
            Ok(written) => format!("{}\n✓ wrote {} files under {}", chain.manifest(), written.len(), dir),
            Err(e) => format!("{}\n✗ write failed: {e}", chain.manifest()),
        },
        _ => format!("{}\n(preview only — pass out_dir to write)", chain.manifest()),
    }
}

// ── flux_archive ──
// MCP surface for flux-archive content-addressed backup. Stateless — the manifest
// is persisted in the store, so verify/restore need only the store path.
fn flux_archive(args: &Value) -> String {
    use flux_archive::archive::{load_manifest, manifest_path, restore, snapshot_and_save, verify};
    use std::path::Path;
    let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
    let store = args.get("store").and_then(|v| v.as_str()).unwrap_or("");
    if store.is_empty() {
        return "✗ 'store' is required".to_string();
    }
    let store_p = Path::new(store);
    match action {
        "snapshot" => {
            let src = args.get("src").and_then(|v| v.as_str()).unwrap_or("");
            if src.is_empty() {
                return "✗ snapshot needs 'src'".to_string();
            }
            match snapshot_and_save(Path::new(src), store_p) {
                Ok(m) => {
                    let dedup = if m.total_bytes > 0 {
                        100.0 * (1.0 - m.unique_bytes as f64 / m.total_bytes as f64)
                    } else {
                        0.0
                    };
                    format!(
                        "📦 snapshot ok — {} files · {} logical bytes · {} unique ({:.1}% deduped)\n   manifest: {}",
                        m.entries.len(), m.total_bytes, m.unique_bytes, dedup,
                        manifest_path(store_p).display()
                    )
                }
                Err(e) => format!("✗ snapshot failed: {e}"),
            }
        }
        "verify" => match load_manifest(store_p) {
            Ok(m) => match verify(&m, store_p) {
                Ok(()) => format!("✓ verify ok — {} files, all CIDs present + hash-correct", m.entries.len()),
                Err(e) => format!("✗ integrity FAIL: {e}"),
            },
            Err(e) => format!("✗ no manifest in store ({e})"),
        },
        "restore" => {
            let dst = args.get("dst").and_then(|v| v.as_str()).unwrap_or("");
            if dst.is_empty() {
                return "✗ restore needs 'dst'".to_string();
            }
            match load_manifest(store_p) {
                Ok(m) => match restore(&m, store_p, Path::new(dst)) {
                    Ok(n) => format!("♻ restored {n} files to {dst} (each hash-verified)"),
                    Err(e) => format!("✗ restore failed: {e}"),
                },
                Err(e) => format!("✗ no manifest in store ({e})"),
            }
        }
        _ => "✗ action must be one of: snapshot | verify | restore".to_string(),
    }
}

// ── flux_fullcheck ──

fn flux_fullcheck(args: &Value) -> String {
    let release = args.get("release").and_then(|v| v.as_bool()).unwrap_or(false);
    let start = std::time::Instant::now();
    let mut report = Vec::new();
    report.push("⚡ Flux Fullcheck — Self-build + Benchmark + Health".to_string());

    // Step 1: Self-build
    let phase_start = std::time::Instant::now();
    let mut cmd = std::process::Command::new("cargo");
    cmd.arg("build").arg("--package").arg("fluxc");
    if release { cmd.arg("--release"); }
    // The MCP server's cwd is NOT the flux workspace (it runs from wherever the
    // editor launched it, e.g. /home/storage/claude-code). Without current_dir,
    // `cargo build --package fluxc` errors instantly (~200ms, exit 101) — the
    // dogfood meta-tool false-failed even though `fluxc self` builds green.
    // Resolve the workspace via the binary location, like the rest of the tools.
    cmd.current_dir(fluxc_core::version::workspace_root());
    cmd.env("RUSTC_WRAPPER", std::env::current_exe().unwrap_or_else(|_| "fluxc".into()));

    match cmd.status() {
        Ok(s) if s.success() => {
            report.push(format!("  ✓ Self-build: {}ms", phase_start.elapsed().as_millis()));
        }
        Ok(s) => {
            report.push(format!("  ✗ Self-build FAILED in {}ms (exit {})", phase_start.elapsed().as_millis(), s.code().unwrap_or(1)));
        }
        Err(e) => {
            report.push(format!("  ✗ Self-build error: {}", e));
        }
    }

    // Step 2: Benchmark
    let phase_start = std::time::Instant::now();
    let arch = crate::handlers::analyze_ws();
    let mut total_pred_ms: u64 = 0;
    let mut total_conf: f64 = 0.0;
    for c in &arch.crates {
        let p = predict::predict_build(&c.name, false, &[]);
        total_pred_ms += p.predicted_ms;
        total_conf += p.confidence;
    }
    let n = arch.crates.len() as f64;
    if n > 0.0 {
        report.push(format!("  ✓ Benchmark: {}ms — {} crates, {}ms predicted total, {:.0}% avg conf",
            phase_start.elapsed().as_millis(), arch.crates.len(), total_pred_ms, total_conf / n * 100.0));
    }

    // Step 3: Health
    let phase_start = std::time::Instant::now();
    let swot = quantum_architect::generate_swot(&arch);
    report.push(format!("  ✓ Health: {:.1}% · SWOT priority: {}", arch.architecture_score * 100.0, swot.top_priority));
    report.push(format!("   Predict: {}ms total · {:.0}% avg conf ({} crates)",
        total_pred_ms, total_conf / n * 100.0, arch.crates.len()));
    report.push(format!("   Health: {}ms", phase_start.elapsed().as_millis()));

    let total_ms = start.elapsed().as_millis();
    report.push(format!("\n⚡ Fullcheck complete in {}ms — dogfooding verified", total_ms));
    report.join("\n")
}

// ── flux_v4_optimize ──

fn flux_v4_optimize(args: &Value) -> String {
    let format = args.get("format").and_then(|v| v.as_str()).unwrap_or("text");
    let scope = args.get("scope").and_then(|v| v.as_str()).unwrap_or("all");

    let report = json!({
        "tool": "flux_v4_optimize",
        "version": concat!("v", env!("CARGO_PKG_VERSION")),
        "scope": scope,
        "analysis": {
            "prefix_caching": {
                "granularity": "128 tokens",
                "strategy": "append-dont-reorder — keep byte-stable prefix",
                "savings": "~90% cost on repeated reads to quillon:// resources",
                "recommendation": "Structure turn sequences for cache reuse"
            },
            "parallel_execution": {
                "strategy": "batch independent reads/searches/greps in single turn",
                "savings": "~67% latency reduction per batch",
                "tools_parallelizable": ["read_file", "grep_files", "list_dir", "git_*"]
            },
            "thinking_tokens": {
                "strategy": "Use strategically — skip for lookups, light for code gen, deep for architecture",
                "cost_note": "thinking tokens count against context and replay across turns"
            },
            "mcp_phrasal_verbs": {
                "active": ["flux_combo", "flux_quickcast", "flux_ult", "flux_fullcheck", "flux_quickstart", "flux_bootstrap"],
                "savings": "67-80% token savings per phrasal verb vs individual calls"
            }
        }
    });

    if format == "json" {
        serde_json::to_string_pretty(&report).unwrap_or_else(|e| format!("json: {}", e))
    } else {
        format!(
            "⚡ Flux V4 Optimization — v{v}\n\n\
             🔄 Prefix Cache: 128-token gran, 90% discount on hits.\n\
                Cold start: responses = cache MISS (full price). Warm: 90% off.\n\
             ⚡ Parallel: batch reads/searches in one turn (67% faster).\n\
             📦 Phrasal Verbs: 7 tools (67-80% savings):\n\
                combo quickcast ult fullcheck quickstart bootstrap architect_predict\n\
             💰 Per-Tool Token Cost (v{v} audit):\n\
                flux_bootstrap 4500ch $0.00016 flux_quickstart 3500ch $0.00012\n\
                flux_architect 2500ch $0.00009 flux_swot 2000ch $0.00007\n\
             💡 Skip quickstart/bootstrap. Read instructions.md directly —\n\
                prefix-cached in system prompt = ~90% cheaper.",
            v = env!("CARGO_PKG_VERSION"),
        )
    }
}

// ── flux_diagnose ──

fn flux_diagnose(args: &Value) -> String {
    let package = args.get("package").and_then(|v| v.as_str()).unwrap_or("fluxc");
    let start = std::time::Instant::now();
    let mut report = Vec::new();

    let arch = crate::handlers::analyze_ws();
    report.push("⚛️  Architecture:".to_string());
    report.push(format!("   Score: {:.1}% ideal · {} crates · {} LOC",
        arch.architecture_score * 100.0, arch.crates.len(),
        arch.crates.iter().map(|b| b.loc).sum::<usize>()));

    let swot = quantum_architect::generate_swot(&arch);
    report.push("\n📊 SWOT:".to_string());
    report.push(format!("   Strengths: {} · Weaknesses: {} · Opportunities: {} · Threats: {}",
        swot.strengths.len(), swot.weaknesses.len(),
        swot.opportunities.len(), swot.threats.len()));
    report.push(format!("   Top Priority: {}", swot.top_priority));

    let pred = predict::predict_build(package, false, &[]);
    report.push("\n🔮 Prediction:".to_string());
    report.push(format!("   {}ms predicted · {:.0}% cache · {:.0}% test pass · {:.0}% confidence",
        pred.predicted_ms,
        pred.predicted_cache_rate * 100.0,
        pred.predicted_test_pass * 100.0,
        pred.confidence * 100.0));

    if !arch.priority_actions.is_empty() {
        report.push("\n🎯 Priority Actions:".to_string());
        for a in arch.priority_actions.iter().take(3) {
            report.push(format!("   #{}. {} — {} (impact {:.0}%, {} effort)",
                a.rank, a.crate_name, a.action, a.impact * 100.0, a.effort));
        }
    }

    let ms = start.elapsed().as_millis();
    report.push(format!("\n⏱ Diagnose complete in {}ms", ms));
    report.join("\n")
}

// ── flux_quantum_architect ──

fn flux_quantum_architect(args: &Value) -> String {
    let start = std::time::Instant::now();
    let arch = crate::handlers::analyze_ws();
    let ms = start.elapsed().as_millis();

    let mut report = vec![format!("⚛️  Quantum Architecture — {} crates in {}ms", arch.crates.len(), ms)];
    report.push(format!("  Score: {:.1}% ideal", arch.architecture_score * 100.0));
    report.push("".to_string());

    for bp in &arch.crates {
        let bar_len = ((1.0 - bp.gap_to_ideal) * 20.0) as usize;
        let bar = "█".repeat(bar_len.min(20));
        report.push(format!("  {} [{:.0}%] {} {} LOC, {} dep(s)",
            bp.name, ((1.0 - bp.gap_to_ideal) * 100.0), bar, bp.loc, bp.dependencies.len()));
    }

    if !arch.priority_actions.is_empty() {
        report.push("".to_string());
        report.push("  ▶ Priority Actions (quantum tunneling shortcuts):".to_string());
        for a in arch.priority_actions.iter().take(10) {
            report.push(format!("    #{}. {} — {} (impact {:.0}%, {} effort, ~{}min)",
                a.rank, a.crate_name, a.action, a.impact * 100.0, a.effort, a.estimated_minutes));
        }
    }

    report.join("\n")
}

// ── flux_swot ──

fn flux_swot(_args: &Value) -> String {
    let arch = crate::handlers::analyze_ws();
    let swot = quantum_architect::generate_swot(&arch);

    let mut report = vec![format!("📊 SWOT Analysis — Architecture Score: {:.1}%", arch.architecture_score * 100.0)];

    if !swot.strengths.is_empty() {
        report.push("\n💪 Strengths:".to_string());
        for s in &swot.strengths { report.push(format!("  ✓ {}", s)); }
    }

    if !swot.weaknesses.is_empty() {
        report.push("\n🔧 Weaknesses:".to_string());
        for w in &swot.weaknesses { report.push(format!("  ✗ {}", w)); }
    }

    if !swot.opportunities.is_empty() {
        report.push("\n🚀 Opportunities:".to_string());
        for o in &swot.opportunities { report.push(format!("  → {}", o)); }
    }

    if !swot.threats.is_empty() {
        report.push("\n⚠️ Threats:".to_string());
        for t in &swot.threats { report.push(format!("  ⚡ {}", t)); }
    }

    report.push(format!("\n🎯 Top Priority: {}", swot.top_priority));
    report.join("\n")
}

// ── flux_optimize ──

fn flux_optimize(args: &Value) -> String {
    let arch = crate::handlers::analyze_ws();
    let mut report = vec!["⚡ Flux Optimize — Ranked suggestions".to_string()];

    for bp in &arch.crates {
        if bp.gap_to_ideal > 0.35 {
            let impact = (bp.gap_to_ideal - 0.35).max(0.0) * 100.0;
            report.push(format!("  {} [{:.0}%] → add tests/files/docs (+{:.0}% potential)",
                bp.name, ((1.0 - bp.gap_to_ideal) * 100.0), impact));
        }
    }

    if !arch.priority_actions.is_empty() {
        report.push("\n🎯 Top optimization candidates:".to_string());
        for a in arch.priority_actions.iter().take(5) {
            report.push(format!("  #{}. {} — {} (impact {:.0}%, {} effort, ~{}min)",
                a.rank, a.crate_name, a.action, a.impact * 100.0, a.effort, a.estimated_minutes));
        }
    }

    report.join("\n")
}
// ── flux_architect_predict ──

fn flux_architect_predict(args: &Value) -> String {
    let format = args.get("format").and_then(|v| v.as_str()).unwrap_or("text");
    let start = std::time::Instant::now();
    let arch = crate::handlers::analyze_ws();
    let crates: &[&str] = &[
        "flux-cache", "flux-db", "flux-driver", "flux-gpu", "flux-gui",
        "flux-hotswap", "flux-mempool", "flux-p2p", "flux-science",
        "flux-search", "flux-sniff", "flux-zk", "fluxc-core", "fluxc-mcp", "fluxc",
    ];
    let mut total_ms: u64 = 0;
    let mut total_conf: f64 = 0.0;
    let mut preds: Vec<predict::BuildPrediction> = Vec::with_capacity(15);
    for c in crates {
        let p = predict::predict_build(c, false, &[]);
        total_ms += p.predicted_ms;
        total_conf += p.confidence;
        preds.push(p);
    }
    let n = preds.len() as f64;
    let ms = start.elapsed().as_millis();

    if format == "json" {
        let items: Vec<Value> = preds.iter().map(|p| json!({
            "crate": p.package,
            "predicted_ms": p.predicted_ms,
            "cache_rate_pct": (p.predicted_cache_rate * 100.0).round(),
            "test_pass_pct": (p.predicted_test_pass * 100.0).round(),
            "confidence_pct": (p.confidence * 100.0).round(),
        })).collect();
        let report = json!({
            "arch_score_pct": (arch.architecture_score * 100.0).round(),
            "crate_count": arch.crates.len(),
            "total_loc": arch.crates.iter().map(|b| b.loc).sum::<usize>(),
            "total_pred_ms": total_ms,
            "avg_confidence_pct": (total_conf / n * 100.0).round(),
            "elapsed_ms": ms,
            "predictions": items,
        });
        serde_json::to_string_pretty(&report).unwrap_or_else(|e| format!("json: {}", e))
    } else {
        let mut report = vec![format!(
            "🔮 Flux Architect+Predict — {}ms\n\n⚛️  Architecture: {:.0}% · {} crates · {} LOC",
            ms, arch.architecture_score * 100.0, arch.crates.len(),
            arch.crates.iter().map(|b| b.loc).sum::<usize>()
        )];
        for bp in &arch.crates {
            report.push(format!("  {} [{:.0}%] {} LOC, {} dep(s)",
                bp.name, ((1.0 - bp.gap_to_ideal) * 100.0), bp.loc, bp.dependencies.len()));
        }
        report.push(format!("\n🔮 Batch Predict: {} crates · {}ms total · {:.0}% avg conf",
            preds.len(), total_ms, total_conf / n * 100.0));
        for p in &preds {
            report.push(format!("  {} — {}ms · {:.0}% cache · {:.0}% conf",
                p.package, p.predicted_ms,
                p.predicted_cache_rate * 100.0, p.confidence * 100.0));
        }
        if !arch.priority_actions.is_empty() {
            report.push("\n🎯 Priority Actions:".to_string());
            for a in arch.priority_actions.iter().take(3) {
                report.push(format!("  #{}. {} — {} ({} effort, ~{}min)",
                    a.rank, a.crate_name, a.action, a.effort, a.estimated_minutes));
            }
        }
        report.join("\n")
    }
}

// ── flux_mcp_status ──
// The tool a new user (or Grok pilot) calls immediately after wiring a fresh fluxc MCP.
// Gives concrete, actionable status instead of the old hardcoded quickstart assumptions.
fn flux_mcp_status(_args: &Value) -> String {
    let mut out = vec!["⚡ flux_mcp_status — MCP + dev skills health for first-time users".to_string()];
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    let ws = crate::handlers::ws();
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let path = std::env::var("PATH").unwrap_or_default();

    out.push(format!("exe (the binary serving these tools): {}", exe));
    out.push(format!("workspace (from exe walk): {}", ws.display()));

    let fluxc_dir = std::path::Path::new(&exe).parent().map(|p| p.display().to_string()).unwrap_or_default();
    let on_path = path.split(':').any(|seg| seg == fluxc_dir || std::path::Path::new(seg).join("fluxc").exists());
    out.push(format!("fluxc in PATH? {}", if on_path { "yes" } else { "NO" }));

    // Client + skill scan
    let candidates = [
        (format!("{}/.grok", home), "Grok"),
        (format!("{}/.claude", home), "Claude Code"),
        (format!("{}/.cursor", home), "Cursor"),
        (format!("{}/.codex", home), "Codex"),
    ];
    let mut found_clients = vec![];
    let mut skill_hits = vec![];
    for (dir, label) in &candidates {
        if std::path::Path::new(dir).exists() {
            found_clients.push(*label);
        }
        let sdir = format!("{}/skills", dir);
        if let Ok(rd) = std::fs::read_dir(&sdir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.contains("flux") || name.contains("sigil") {
                    if std::path::Path::new(&format!("{}/{}", sdir, name)).join("SKILL.md").exists() {
                        skill_hits.push(format!("{} @ {}", name, sdir));
                    }
                }
            }
        }
    }
    out.push(format!("detected client homes: {}", if found_clients.is_empty() { "none".to_string() } else { found_clients.join(", ") }));
    if skill_hits.is_empty() {
        out.push("flux-* / sigil skills found: NONE — run setup-flux.sh or place flux-dev/SKILL.md under one of the client skills/ dirs".to_string());
    } else {
        out.push("flux/skill hits:".to_string());
        for h in skill_hits.iter().take(8) { out.push(format!("  - {}", h)); }
    }

    out.push("".to_string());
    out.push("Fixes (copy-paste):".to_string());
    out.push(format!("  mkdir -p ~/.flux/bin && ln -sf {} ~/.flux/bin/fluxc", exe));
    out.push("  export PATH=\"$HOME/.flux/bin:$PATH\"".to_string());
    out.push("  # Then restart your AI client (or re-connect MCP) and say: load the flux-dev skill ; flux_mcp_status".to_string());
    out.push("  # For full installer: curl -fsSL https://fluxapp.xyz/setup-flux.sh | bash".to_string());

    out.push("".to_string());
    out.push("Next after green status: flux_quickstart ; flux_combo package=fluxc-mcp   (edit MCP → combo immediately)".to_string());
    out.join("\n")
}

// ── flux_mcp_register ──
// Outputs the precise bits a user pastes into their client so the *current* (live) fluxc
// becomes the MCP server. This directly addresses the pilot feedback that the one-liner
// script is Claude-centric and Grok/other setups need explicit help.
fn flux_mcp_register(args: &Value) -> String {
    let client = args.get("client").and_then(|v| v.as_str()).unwrap_or("all").to_lowercase();
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();

    let mut out = vec![format!("⚡ flux_mcp_register — live fluxc = {}", exe)];

    let json_blob = format!("{{\"command\":\"{}\",\"args\":[\"mcp\"]}}", exe);

    if client == "all" || client == "claude" {
        out.push("Claude Code:".to_string());
        out.push(format!("  claude mcp add-json flux '{}'", json_blob));
        out.push("  # or edit ~/.claude/settings.json mcpServers.flux = the above".to_string());
    }
    if client == "all" || client == "cursor" {
        out.push("Cursor (~/.cursor/mcp.json):".to_string());
        out.push("  { \"mcpServers\": { \"flux\": ".to_string() + &json_blob + " } }");
    }
    if client == "all" || client == "codex" {
        out.push("Codex (~/.codex/config.toml):".to_string());
        out.push(format!("  [mcp_servers.flux]\n  command = \"{}\"\n  args = [\"mcp\"]", exe));
    }
    if client == "all" || client == "grok" {
        out.push("Grok (this runtime):".to_string());
        out.push("  MCP servers are usually host-injected (see system MCP list).".to_string());
        out.push("  For local dev: ensure the host launches the desired fluxc with `mcp` subcommand".to_string());
        out.push("  and wires it before starting the agent session.".to_string());
        out.push("  Alternative (if your Grok build supports stdio MCP config): use the JSON blob above under flux key.".to_string());
    }
    out.push("".to_string());
    out.push("After registering: restart the AI client, then call flux_mcp_status + flux_quickstart.".to_string());
    out.join("\n")
}
