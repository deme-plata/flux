//! Flux Swarm Compile — Distributed compilation across the P2P fleet.
//!
//! Cortex MCP combo: ties together flux-cortex (optimization engine),
//! flux-rev (content-addressed snapshots), and flux-p2p (mesh distribution)
//! into a single MCP toolset for cross-server compile coordination.
//!
//! Tools:
//!   flux_swarm_compile    — Distribute a compile job across the P2P fleet
//!   flux_swarm_snapshot   — Create flux-rev snapshot + broadcast via P2P
//!   flux_swarm_cortex     — Run Cortex optimization loop across fleet nodes
//!   flux_swarm_status     — Fleet compile status (per-node build state)
//!
//! Fleet servers:
//!   Delta   5.79.79.158       — primary build server
//!   Epsilon 89.149.241.126    — secondary build + sigil node
//!   Gamma   109.205.176.60    — tertiary build
//!   Beta    185.182.185.227   — quantum cosmos bridge

use crate::handlers::{ToolDef, ToolRegistry};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ── Fleet node definitions ──

const FLEET_NODES: &[(&str, &str, &str)] = &[
    ("delta",   "root@5.79.79.158",       "/home/storage/deepseek-codewhale/flux"),
    ("epsilon", "root@89.149.241.126",    "/home/storage/deepseek-codewhale/flux"),
    ("gamma",   "root@109.205.176.60",    "/home/storage/deepseek-codewhale/flux"),
    ("beta",    "root@185.182.185.227",   "/home/storage/deepseek-codewhale/flux"),
];

const SIGIL_FLEET: &[(&str, &str, &str)] = &[
    ("epsilon-sigil", "root@89.149.241.126",    "/home/storage/deepseek-codewhale/sigil"),
    ("delta-sigil",   "root@5.79.79.158",       "/home/storage/deepseek-codewhale/sigil"),
    ("gamma-sigil",   "root@109.205.176.60",    "/home/storage/deepseek-codewhale/sigil"),
    ("beta-sigil",    "root@185.182.185.227",   "/home/storage/deepseek-codewhale/sigil"),
];

// ── Register ──

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_swarm_compile",
            description: "Distribute compilation across the P2P fleet (Delta, Epsilon, Gamma, Beta). Pushes a flux-rev snapshot, broadcasts compile request via P2P gossipsub, and collects per-node build results. Uses Cortex to optimize the build plan before distribution.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "package": {"type": "string", "description": "Package to compile (e.g. sigil-top, flux-p2p)"},
                    "workspace": {"type": "string", "description": "Workspace root (flux or sigil). Default: flux"},
                    "nodes": {"type": "array", "items": {"type": "string"}, "description": "Target nodes. Default: all ['delta','epsilon','gamma','beta']"},
                    "release": {"type": "boolean", "description": "Release build. Default: true"},
                    "cortex_first": {"type": "boolean", "description": "Run Cortex optimization loop before distributing. Default: true"}
                },
                "required": ["package"]
            }),
        },
        flux_swarm_compile,
    );
    registry.register(
        ToolDef {
            name: "flux_swarm_snapshot",
            description: "Create a flux-rev content-addressed snapshot of the current workspace and broadcast the revision manifest over the P2P mesh so all fleet nodes can pull missing objects.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workspace": {"type": "string", "description": "Workspace root. Default: flux"},
                    "message": {"type": "string", "description": "Snapshot description"},
                    "broadcast": {"type": "boolean", "description": "Broadcast revision id via P2P. Default: true"}
                }
            }),
        },
        flux_swarm_snapshot,
    );
    registry.register(
        ToolDef {
            name: "flux_swarm_cortex",
            description: "Run a Cortex optimization loop distributed across the fleet. Each node independently optimizes its workspace, then results are collected and the best optimization plan is broadcast back.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "iterations": {"type": "integer", "description": "Cortex iterations per node. Default: 3"},
                    "preset": {"type": "string", "description": "Optimization preset: MaxPerf, Balanced, PowerSaver. Default: MaxPerf"},
                    "nodes": {"type": "array", "items": {"type": "string"}, "description": "Target nodes. Default: all"}
                }
            }),
        },
        flux_swarm_cortex,
    );
    registry.register(
        ToolDef {
            name: "flux_swarm_status",
            description: "Check fleet compile status — per-node build state, last snapshot revision, P2P mesh health, and Cortex optimization scores.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "nodes": {"type": "array", "items": {"type": "string"}, "description": "Nodes to query. Default: all"}
                }
            }),
        },
        flux_swarm_status,
    );
}

// ── Tool implementations ──

fn flux_swarm_compile(args: &Value) -> String {
    let package = match args.get("package").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return "❌ flux_swarm_compile: 'package' is required.".into(),
    };
    let workspace = args.get("workspace").and_then(|v| v.as_str()).unwrap_or("flux");
    let release = args.get("release").and_then(|v| v.as_bool()).unwrap_or(true);
    let cortex_first = args.get("cortex_first").and_then(|v| v.as_bool()).unwrap_or(true);
    let nodes: Vec<String> = args
        .get("nodes")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_else(|| FLEET_NODES.iter().map(|n| n.0.to_string()).collect());

    let fleet = if workspace == "sigil" { &SIGIL_FLEET[..] } else { &FLEET_NODES[..] };
    let profile = if release { "release" } else { "debug" };
    let mut out = format!(
        "🦾 flux_swarm_compile · package '{package}' · {profile} · {} node(s)\n",
        nodes.len()
    );

    // Phase 1: Cortex optimization (local)
    if cortex_first {
        out.push_str("── Phase 1: Cortex optimization ──\n");
        let local_ws = crate::handlers::ws();
        let cortex_result = run_local_cortex(&local_ws, "MaxPerf");
        out.push_str(&format!("  Cortex loop: {}\n", cortex_result));
    }

    // Phase 2: flux-rev snapshot
    out.push_str("── Phase 2: flux-rev snapshot ──\n");
    let ws_root = resolve_workspace(workspace);
    let rev_id = match create_rev_snapshot(&ws_root, package) {
        Ok(id) => {
            out.push_str(&format!("  ✓ Snapshot created: {}\n", id));
            id
        }
        Err(e) => {
            out.push_str(&format!("  ⚠ Snapshot skipped: {}\n", e));
            "none".to_string()
        }
    };

    // Phase 3: drive the v2 distributed compiler (`fluxc build --distributed`).
    // This replaces the old per-node redundant loop. distributed_build() owns
    // probe + toolchain-gate + identical-path source sync + batch-barrier artifact
    // reconciliation + coordinator assemble, so the SIGIL/FLEET tables are no longer
    // used for distribution (distributed_build resolves the fleet via FLUX_DIST_PEERS
    // or its defaults). The old "unknown node" mismatch (sigil names vs FLEET_NODES)
    // is gone because we no longer look crates up in those tables here.
    out.push_str("── Phase 3: distributed compile (v2 fluxc --distributed) ──\n");
    let mut ok = 0u32;
    let mut fail = 0u32;

    let build_root = resolve_workspace(workspace);
    let fluxc_bin = crate::handlers::ws().join("target/debug/fluxc");
    let fluxc = if fluxc_bin.exists() {
        fluxc_bin.to_string_lossy().to_string()
    } else {
        "fluxc".into()
    };

    let mut cmd = Command::new(&fluxc);
    cmd.arg("build").arg("--distributed").arg("--package").arg(package);
    if release {
        cmd.arg("--release");
    }
    cmd.current_dir(&build_root);
    // Optional explicit peer set → FLUX_DIST_PEERS="name=host,...". Accepts either
    // plain ("delta") or sigil-suffixed ("delta-sigil") names from either table.
    if !nodes.is_empty() {
        let peers: Vec<String> = nodes
            .iter()
            .filter_map(|n| {
                let base = n.trim_end_matches("-sigil");
                fleet
                    .iter()
                    .chain(FLEET_NODES.iter())
                    .find(|f| f.0 == *n || f.0 == base)
                    .map(|f| format!("{}={}", base, f.1))
            })
            .collect();
        if !peers.is_empty() {
            cmd.env("FLUX_DIST_PEERS", peers.join(","));
        }
    }

    let t = Instant::now();
    match cmd.output() {
        Ok(o) => {
            let so = String::from_utf8_lossy(&o.stdout);
            let se = String::from_utf8_lossy(&o.stderr);
            for line in so.lines().chain(se.lines()) {
                let l = line.trim();
                if l.contains("Distributed Build")
                    || l.contains("sync src")
                    || l.contains("ready —")
                    || l.contains("SKIPPED")
                    || l.contains("Active fleet")
                    || l.contains("assembling")
                    || l.contains("Distributed build complete")
                    || l.contains("FAILED")
                    || l.contains("falling back")
                {
                    out.push_str(&format!("  {l}\n"));
                }
            }
            if so.contains("Distributed build complete") && o.status.success() {
                ok = 1;
                out.push_str(&format!(
                    "  ✓ distributed build OK in {:.1}s\n",
                    t.elapsed().as_secs_f64()
                ));
            } else {
                fail = 1;
                out.push_str(&format!(
                    "  ✗ distributed build did not complete cleanly ({:.1}s)\n",
                    t.elapsed().as_secs_f64()
                ));
            }
        }
        Err(e) => {
            fail = 1;
            out.push_str(&format!("  ✗ failed to spawn fluxc: {e}\n"));
        }
    }

    // Phase 4: P2P broadcast (announce compile result)
    out.push_str("── Phase 4: P2P broadcast ──\n");
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let p2p_msg = json!({
        "event": "swarm_compile_complete",
        "package": package,
        "workspace": workspace,
        "release": release,
        "nodes_ok": ok,
        "nodes_fail": fail,
        "rev_id": rev_id,
        "ts_unix": ts
    });
    let p2p_status = broadcast_compile_event(&p2p_msg);
    out.push_str(&format!("  P2P broadcast: {}\n", p2p_status));

    out.push_str(&format!(
        "\n  {ok}/{total} node(s) compiled successfully.\n",
        total = ok + fail
    ));
    out
}

fn flux_swarm_snapshot(args: &Value) -> String {
    let workspace = args.get("workspace").and_then(|v| v.as_str()).unwrap_or("flux");
    let message = args
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("swarm auto-snapshot");
    let broadcast = args.get("broadcast").and_then(|v| v.as_bool()).unwrap_or(true);
    let ws_root = resolve_workspace(workspace);

    let mut out = format!("📸 flux_swarm_snapshot · {workspace}\n");

    // Create flux-rev snapshot
    match create_rev_snapshot(&ws_root, message) {
        Ok(rev_id) => {
            out.push_str(&format!("  ✓ Revision: {}\n", rev_id));

            if broadcast {
                let ts = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                let p2p_msg = json!({
                    "event": "rev_snapshot",
                    "workspace": workspace,
                    "rev_id": rev_id,
                    "message": message,
                    "ts_unix": ts
                });
                let status = broadcast_compile_event(&p2p_msg);
                out.push_str(&format!("  📡 P2P broadcast: {}\n", status));
            }
        }
        Err(e) => {
            out.push_str(&format!("  ✗ Snapshot failed: {e}\n"));
            out.push_str("  hint: ensure flux-rev crate is available and .flux-rev/ dir exists\n");
        }
    }
    out
}

fn flux_swarm_cortex(args: &Value) -> String {
    let iterations = args
        .get("iterations")
        .and_then(|v| v.as_u64())
        .unwrap_or(3)
        .min(20) as usize;
    let preset_str = args.get("preset").and_then(|v| v.as_str()).unwrap_or("MaxPerf");
    let nodes: Vec<String> = args
        .get("nodes")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_else(|| FLEET_NODES.iter().map(|n| n.0.to_string()).collect());

    let mut out = format!(
        "🧠 flux_swarm_cortex · {iterations} iteration(s) · preset={preset_str} · {} node(s)\n",
        nodes.len()
    );

    // Local Cortex first
    out.push_str("── Local Cortex ──\n");
    let local_ws = crate::handlers::ws();
    let local_result = run_local_cortex_continuous(&local_ws, iterations, preset_str);
    out.push_str(&format!("  {}\n", local_result));

    // Fleet Cortex via SSH
    out.push_str("── Fleet Cortex ──\n");
    for node_name in &nodes {
        let node = match FLEET_NODES.iter().find(|n| n.0 == *node_name) {
            Some(n) => n,
            None => continue,
        };
        let (name, host, remote_root) = (node.0, node.1, node.2);

        if !probe_host(host) {
            out.push_str(&format!("  ✗ {name}: unreachable\n"));
            continue;
        }

        let cortex_cmd = format!(
            "cd {root} && fluxc mcp --tool flux_cortex_loop --args '{{\"preset\":\"{preset}\"}}' 2>&1 | tail -3",
            root = remote_root,
            preset = preset_str
        );
        match ssh_exec(host, &cortex_cmd) {
            Ok(output) => {
                let summary = output.lines().last().unwrap_or("(no output)");
                out.push_str(&format!("  ✓ {name}: {}\n", summary));
            }
            Err(e) => {
                out.push_str(&format!("  ✗ {name}: {e}\n"));
            }
        }
    }
    out
}

fn flux_swarm_status(args: &Value) -> String {
    let nodes: Vec<String> = args
        .get("nodes")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_else(|| FLEET_NODES.iter().map(|n| n.0.to_string()).collect());

    let mut out = format!(
        "⚓ flux_swarm_status · {} node(s) · {}\n",
        nodes.len(),
        chrono_now()
    );

    let mut online = 0u32;
    let mut has_flux = 0u32;
    let mut has_sigil = 0u32;

    for node_name in &nodes {
        let node = match FLEET_NODES.iter().find(|n| n.0 == *node_name) {
            Some(n) => n,
            None => {
                out.push_str(&format!("  ✗ {node_name}: unknown\n"));
                continue;
            }
        };
        let (name, host, _) = (node.0, node.1, node.2);

        if !probe_host(host) {
            out.push_str(&format!("  🔴 {name} ({host}): OFFLINE\n"));
            continue;
        }
        online += 1;

        // Check flux version
        let flux_ver = ssh_exec(host, "cd /home/storage/deepseek-codewhale/flux && ./target/debug/fluxc --version 2>&1 || echo 'no-fluxc'")
            .unwrap_or_else(|_| "unknown".into());
        let flux_ver = flux_ver.trim();
        if flux_ver != "no-fluxc" && !flux_ver.is_empty() {
            has_flux += 1;
        }

        // Check sigil-top version
        let sigil_ver = ssh_exec(host, "cd /home/storage/deepseek-codewhale/sigil && ./target/release/sigil-top --version 2>&1 || echo 'no-sigil-top'")
            .unwrap_or_else(|_| "unknown".into());
        let sigil_ver = sigil_ver.trim();
        if sigil_ver != "no-sigil-top" && !sigil_ver.is_empty() {
            has_sigil += 1;
        }

        out.push_str(&format!(
            "  🟢 {name}: flux={flux_ver} sigil-top={sigil_ver}\n"
        ));
    }

    // Local status
    out.push_str("── Local ──\n");
    let local_ws = crate::handlers::ws();
    out.push_str(&format!("  workspace: {}\n", local_ws.display()));

    // Check flux-rev
    let rev_dir = local_ws.join(".flux-rev");
    if rev_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&rev_dir) {
            let count = entries.count();
            out.push_str(&format!("  flux-rev: {} objects\n", count));
        }
    } else {
        out.push_str("  flux-rev: not initialized\n");
    }

    out.push_str(&format!(
        "\n  {online}/{total} fleet node(s) online · {has_flux} with fluxc · {has_sigil} with sigil-top\n",
        total = nodes.len()
    ));
    out
}

// ── Helpers ──

fn resolve_workspace(name: &str) -> PathBuf {
    match name {
        "sigil" => PathBuf::from("/home/storage/deepseek-codewhale/sigil"),
        _ => crate::handlers::ws(),
    }
}

fn probe_host(host: &str) -> bool {
    let out = Command::new("ssh")
        .args([
            "-o", "BatchMode=yes",
            "-o", "ConnectTimeout=8",
            "-o", "StrictHostKeyChecking=accept-new",
            host,
            "echo ok",
        ])
        .output();
    match out {
        Ok(o) => o.status.success(),
        Err(_) => false,
    }
}

fn ssh_exec(host: &str, cmd: &str) -> Result<String, String> {
    let out = Command::new("ssh")
        .args([
            "-o", "BatchMode=yes",
            "-o", "ConnectTimeout=15",
            "-o", "StrictHostKeyChecking=accept-new",
            host,
            cmd,
        ])
        .output()
        .map_err(|e| format!("ssh spawn: {e}"))?;

    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        Err(format!("ssh exit {}: {}", out.status, stderr.trim()))
    }
}

fn run_local_cortex(ws_root: &PathBuf, preset: &str) -> String {
    // Use the flux-cortex crate directly if available
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let member_paths = match flux_graph::workspace::discover_members(ws_root) {
            Ok(p) => p,
            Err(e) => return format!("workspace discovery: {e}"),
        };
        let crates: Vec<flux_graph::CrateInfo> = member_paths
            .iter()
            .filter_map(|p| {
                let name = p.file_name()?.to_string_lossy().to_string();
                Some(flux_graph::CrateInfo {
                    name,
                    path: p.clone(),
                    dependencies: vec![],
                    edition: "2021".to_string(),
                    crate_type: flux_graph::CrateType::Lib,
                    features: vec![],
                })
            })
            .collect();
        let ws = flux_graph::WorkspaceGraph {
            root: ws_root.clone(),
            crates,
            batches: vec![],
        };
        let preset_enum = match preset.to_lowercase().as_str() {
            "powersaver" | "power_saver" => flux_optimize::OptimizationPreset::PowerSaver,
            "balanced" => flux_optimize::OptimizationPreset::Balanced,
            _ => flux_optimize::OptimizationPreset::MaxPerf,
        };
        let mut cortex = flux_cortex::Cortex::new(ws);
        let result = cortex.run_loop(preset_enum);
        format!(
            "{:.2}% gain, {} actions",
            result.actual_total_gain_pct.unwrap_or(0.0),
            result.top_actions.len()
        )
    })) {
        Ok(s) => s,
        Err(_) => "cortex unavailable (flux-cortex crate not linked)".into(),
    }
}

fn run_local_cortex_continuous(ws_root: &PathBuf, iterations: usize, preset: &str) -> String {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let member_paths = match flux_graph::workspace::discover_members(ws_root) {
            Ok(p) => p,
            Err(e) => return format!("discovery: {e}"),
        };
        let crates: Vec<flux_graph::CrateInfo> = member_paths
            .iter()
            .filter_map(|p| {
                let name = p.file_name()?.to_string_lossy().to_string();
                Some(flux_graph::CrateInfo {
                    name,
                    path: p.clone(),
                    dependencies: vec![],
                    edition: "2021".to_string(),
                    crate_type: flux_graph::CrateType::Lib,
                    features: vec![],
                })
            })
            .collect();
        let ws = flux_graph::WorkspaceGraph {
            root: ws_root.clone(),
            crates,
            batches: vec![],
        };
        let preset_enum = match preset.to_lowercase().as_str() {
            "powersaver" | "power_saver" => flux_optimize::OptimizationPreset::PowerSaver,
            "balanced" => flux_optimize::OptimizationPreset::Balanced,
            _ => flux_optimize::OptimizationPreset::MaxPerf,
        };
        let mut cortex = flux_cortex::Cortex::new(ws);
        let results = cortex.run_continuous(iterations, preset_enum);
        let total_gain: f64 = results.iter().filter_map(|r| r.actual_total_gain_pct).sum();
        format!("{} iter, {:.2}% total gain", results.len(), total_gain)
    })) {
        Ok(s) => s,
        Err(_) => "cortex continuous unavailable".into(),
    }
}

fn create_rev_snapshot(ws_root: &PathBuf, message: &str) -> Result<String, String> {
    let rev_dir = ws_root.join(".flux-rev");
    std::fs::create_dir_all(&rev_dir).map_err(|e| format!("mkdir .flux-rev: {e}"))?;

    // Use flux-rev if available
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let store = flux_rev::Store::open(ws_root)
            .map_err(|e| format!("store open: {e}"))?;
        let genesis_id = "swarm-genesis";
        let rev = flux_rev::snapshot(
            ws_root,
            &store,
            None,
            genesis_id,
            "0.7.0",
            "swarm-compile",
            message,
        )
        .map_err(|e| format!("snapshot: {e}"))?;
        Ok(rev.id)
    })) {
        Ok(id) => id,
        Err(_) => {
            // Fallback: create a minimal manifest manually
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let fallback_id = format!("swarm-{ts}-{}", blake3::hash(message.as_bytes()).to_hex());
            let manifest = json!({
                "id": fallback_id,
                "ts_unix": ts,
                "message": message,
                "workspace": ws_root.to_string_lossy()
            });
            let path = rev_dir.join(format!("{}.json", &fallback_id[..16]));
            std::fs::write(&path, manifest.to_string())
                .map_err(|e| format!("write fallback manifest: {e}"))?;
            Ok(fallback_id)
        }
    }
}

fn broadcast_compile_event(msg: &Value) -> String {
    // Write to the swarm activity log so other MCP tools can see it
    let log_path = "/tmp/flux-swarm-activity.jsonl";
    let line = format!("{}\n", msg);
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    {
        Ok(mut f) => {
            use std::io::Write;
            match f.write_all(line.as_bytes()) {
                Ok(_) => "logged to swarm activity".into(),
                Err(e) => format!("log write error: {e}"),
            }
        }
        Err(e) => format!("log open error: {e}"),
    }
}

fn chrono_now() -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    // Simple ISO-like format
    let secs = ts % 60;
    let mins = (ts / 60) % 60;
    let hours = (ts / 3600) % 24;
    format!("T+{:02}:{:02}:{:02}", hours, mins, secs)
}
