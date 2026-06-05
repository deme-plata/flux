//! `flux_nodeswarm_*` — control a swarm of REAL node processes on one VPS.
//!
//! Where `flux_chronos_run` simulates a mesh in virtual time (instant,
//! deterministic, single process), this drives **actual OS processes**: spawn
//! N copies of a node binary, each with its own P2P/API port + isolated data
//! dir, then measure the real per-process RAM/CPU footprint. That's the honest
//! answer to "how many nodes fit on a VPS" — the in-thread `sigil-scaling`
//! sweep gives the CPU scaling *law*; this gives the *process density ceiling*
//! you actually deploy against.
//!
//! Designed for the SIGIL two-class topology:
//!   • a few heavy ARCHIVE nodes (store everything — 80 TB), and
//!   • thousands of LIGHT 10 ms tip-verify nodes (O(1) RAM, no history).
//! Pass `--light`-style flags through `extra_args`; the swarm controller is
//! binary-agnostic (works for sigil-node, flux-p2p-test, anything).
//!
//! State persists to `$FLUX_NODESWARM_STATE` (default `/tmp/flux-nodeswarm.json`)
//! so `status` / `kill` work across separate MCP calls. Run the same combo on
//! Delta and on Epsilon to get the cross-host picture (each call is one VPS).
//!
//! Tools:
//!   flux_nodeswarm_spawn   — launch N processes (ports + data dirs auto-assigned)
//!   flux_nodeswarm_status  — alive count + per-process RSS/CPU + aggregate RAM
//!   flux_nodeswarm_kill    — tear down a swarm (by label, or all) via kill(PID)
//!   flux_nodeswarm_logs    — tail one node's stdout/stderr log

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::handlers::{ToolDef, ToolRegistry};

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_nodeswarm_spawn",
            description: "Spawn N real node processes on THIS VPS. Each gets P2P port base_port+idx, optional API port base_api_port+idx, and an isolated data dir under data_root/node-<idx>. stdout+stderr go to node.log. Binary-agnostic: pass node-specific flags via extra_args (e.g. a --light flag for tip-verify-only nodes). Returns the spawned PIDs + ports. State persists so status/kill work later.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "binary": {"type": "string", "description": "Absolute path to the node binary to run."},
                    "count": {"type": "integer", "description": "Number of node processes to spawn. 1-2000."},
                    "data_root": {"type": "string", "description": "Absolute base dir; each node gets data_root/node-<idx>/. NEVER relative (Q_DB_PATH foot-gun)."},
                    "base_port": {"type": "integer", "description": "First P2P port; node idx gets base_port+idx. Default 9600."},
                    "base_api_port": {"type": "integer", "description": "First API/HTTP port; node idx gets base_api_port+idx. 0 = none. Default 0."},
                    "label": {"type": "string", "description": "Swarm label for status/kill grouping. Default 'default'."},
                    "extra_args": {"type": "array", "items": {"type": "string"}, "description": "Extra CLI args appended to every node. Use {idx}/{port}/{api_port}/{data_dir} placeholders for per-node substitution."},
                    "port_flag": {"type": "string", "description": "Flag name the binary uses for its P2P port, e.g. '--port' or '--p2p-port'. If set, '<port_flag> <port>' is appended. Default '' (rely on extra_args/env)."},
                    "env": {"type": "object", "description": "Extra env vars set on every child. Values may use {idx}/{port}/{api_port}/{data_dir} placeholders."}
                },
                "required": ["binary", "count", "data_root"]
            }),
        },
        flux_nodeswarm_spawn,
    );

    registry.register(
        ToolDef {
            name: "flux_nodeswarm_status",
            description: "Report the live state of node swarms spawned via flux_nodeswarm_spawn: alive/dead per process, per-process RSS (MB) + cumulative CPU seconds, and aggregate RAM. The real 'how many nodes per VPS' density. Optional 'label' filters to one swarm.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "label": {"type": "string", "description": "Only this swarm. Omit for all swarms."},
                    "format": {"type": "string", "description": "text (default) or json"}
                }
            }),
        },
        flux_nodeswarm_status,
    );

    registry.register(
        ToolDef {
            name: "flux_nodeswarm_kill",
            description: "Tear down a node swarm: send SIGTERM then SIGKILL to each PID (kill by PID, never killall), and drop it from state. 'label' kills one swarm; all=true kills every swarm.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "label": {"type": "string", "description": "Swarm to kill."},
                    "all": {"type": "boolean", "description": "Kill every swarm. Default false."}
                }
            }),
        },
        flux_nodeswarm_kill,
    );

    registry.register(
        ToolDef {
            name: "flux_nodeswarm_logs",
            description: "Tail the last N lines of one node's combined stdout/stderr log.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "label": {"type": "string", "description": "Swarm label. Default 'default'."},
                    "idx": {"type": "integer", "description": "Node index within the swarm. Default 0."},
                    "lines": {"type": "integer", "description": "Tail this many lines. Default 40."}
                }
            }),
        },
        flux_nodeswarm_logs,
    );
}

// ── state file ──

fn state_path() -> PathBuf {
    std::env::var("FLUX_NODESWARM_STATE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp/flux-nodeswarm.json"))
}

fn load_state() -> Value {
    fs::read_to_string(state_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({ "swarms": {} }))
}

fn save_state(v: &Value) {
    if let Ok(s) = serde_json::to_string_pretty(v) {
        let _ = fs::write(state_path(), s);
    }
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// ── /proc helpers (Linux) ──

fn pid_alive(pid: i64) -> bool {
    pid > 0 && Path::new(&format!("/proc/{pid}")).exists()
}

/// Resident set size in MB from /proc/<pid>/statm (field 2 = pages).
fn pid_rss_mb(pid: i64) -> Option<f64> {
    let s = fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
    let pages: u64 = s.split_whitespace().nth(1)?.parse().ok()?;
    let page = 4096u64; // getconf PAGESIZE on x86_64
    Some((pages * page) as f64 / (1024.0 * 1024.0))
}

/// Cumulative CPU seconds (utime+stime) from /proc/<pid>/stat.
fn pid_cpu_secs(pid: i64) -> Option<f64> {
    let s = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Fields after the (comm) which may contain spaces/parens — split on ')'.
    let after = s.rsplit_once(')')?.1;
    let f: Vec<&str> = after.split_whitespace().collect();
    // After ')' the first field is state; utime is field 14 overall = index 11
    // here (state=0, ppid=1, ...). utime index 11, stime index 12 in `after`.
    let utime: u64 = f.get(11)?.parse().ok()?;
    let stime: u64 = f.get(12)?.parse().ok()?;
    let hz = 100.0; // CLK_TCK
    Some((utime + stime) as f64 / hz)
}

fn subst(s: &str, idx: u64, port: u64, api_port: u64, data_dir: &str) -> String {
    s.replace("{idx}", &idx.to_string())
        .replace("{port}", &port.to_string())
        .replace("{api_port}", &api_port.to_string())
        .replace("{data_dir}", data_dir)
}

// ── spawn ──

fn flux_nodeswarm_spawn(args: &Value) -> String {
    let binary = match args.get("binary").and_then(|v| v.as_str()) {
        Some(b) => b.to_string(),
        None => return "❌ 'binary' is required (absolute path).".into(),
    };
    let count = args.get("count").and_then(|v| v.as_u64()).unwrap_or(0).clamp(1, 2000);
    let data_root = match args.get("data_root").and_then(|v| v.as_str()) {
        Some(d) => d.to_string(),
        None => return "❌ 'data_root' is required (absolute path).".into(),
    };
    if !Path::new(&binary).exists() {
        return format!("❌ binary not found: {binary}");
    }
    if !data_root.starts_with('/') {
        return format!("❌ data_root must be absolute (Q_DB_PATH foot-gun): {data_root}");
    }
    let base_port = args.get("base_port").and_then(|v| v.as_u64()).unwrap_or(9600);
    let base_api = args.get("base_api_port").and_then(|v| v.as_u64()).unwrap_or(0);
    let label = args.get("label").and_then(|v| v.as_str()).unwrap_or("default").to_string();
    let port_flag = args.get("port_flag").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let extra: Vec<String> = args
        .get("extra_args")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let env_map = args.get("env").and_then(|v| v.as_object()).cloned().unwrap_or_default();

    let mut nodes = Vec::new();
    let mut spawned = 0u64;
    let mut errors = Vec::new();

    for idx in 0..count {
        let port = base_port + idx;
        let api_port = if base_api > 0 { base_api + idx } else { 0 };
        let data_dir = format!("{data_root}/node-{idx}");
        if let Err(e) = fs::create_dir_all(&data_dir) {
            errors.push(format!("node-{idx}: mkdir failed: {e}"));
            continue;
        }
        let log_path = format!("{data_dir}/node.log");
        let log = match fs::File::create(&log_path) {
            Ok(f) => f,
            Err(e) => {
                errors.push(format!("node-{idx}: log create failed: {e}"));
                continue;
            }
        };
        let log_err = match log.try_clone() {
            Ok(f) => f,
            Err(e) => {
                errors.push(format!("node-{idx}: log clone failed: {e}"));
                continue;
            }
        };

        let mut cmd = Command::new(&binary);
        if !port_flag.is_empty() {
            cmd.arg(&port_flag).arg(port.to_string());
        }
        for a in &extra {
            cmd.arg(subst(a, idx, port, api_port, &data_dir));
        }
        for (k, v) in &env_map {
            if let Some(vs) = v.as_str() {
                cmd.env(k, subst(vs, idx, port, api_port, &data_dir));
            }
        }
        cmd.current_dir(&data_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err));
        // Detach from the MCP's process group so signals to the MCP don't
        // propagate to the swarm (stable since Rust 1.64).
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }

        match cmd.spawn() {
            Ok(child) => {
                let pid = child.id() as i64;
                nodes.push(json!({
                    "idx": idx, "pid": pid, "p2p_port": port, "api_port": api_port,
                    "data_dir": data_dir, "log": log_path
                }));
                spawned += 1;
            }
            Err(e) => errors.push(format!("node-{idx}: spawn failed: {e}")),
        }
    }

    let mut state = load_state();
    state["swarms"][&label] = json!({
        "label": label, "binary": binary, "created_unix": now_unix(),
        "base_port": base_port, "base_api_port": base_api, "nodes": nodes
    });
    save_state(&state);

    let mut out = format!(
        "🛰  Spawned {spawned}/{count} node processes · label '{label}'\n  binary: {binary}\n  P2P ports: {base_port}..{}\n  data: {data_root}/node-*\n  state: {}",
        base_port + count - 1,
        state_path().display()
    );
    if !errors.is_empty() {
        out.push_str(&format!("\n  ⚠ {} error(s):\n    {}", errors.len(), errors.join("\n    ")));
    }
    out.push_str("\n  → flux_nodeswarm_status to measure RAM/CPU density.");
    out
}

// ── status ──

fn flux_nodeswarm_status(args: &Value) -> String {
    let filter = args.get("label").and_then(|v| v.as_str());
    let format = args.get("format").and_then(|v| v.as_str()).unwrap_or("text");
    let state = load_state();
    let swarms = state.get("swarms").and_then(|v| v.as_object()).cloned().unwrap_or_default();
    if swarms.is_empty() {
        return "🛰  No node swarms in state. Use flux_nodeswarm_spawn first.".into();
    }

    let mut json_out = serde_json::Map::new();
    let mut text = String::from("🛰  Node swarm status\n");

    for (label, sw) in &swarms {
        if let Some(f) = filter {
            if f != label {
                continue;
            }
        }
        let empty = vec![];
        let nodes = sw.get("nodes").and_then(|v| v.as_array()).unwrap_or(&empty);
        let mut alive = 0u64;
        let mut total_rss = 0.0f64;
        let mut total_cpu = 0.0f64;
        let mut max_rss = 0.0f64;
        for n in nodes {
            let pid = n.get("pid").and_then(|v| v.as_i64()).unwrap_or(0);
            if pid_alive(pid) {
                alive += 1;
                if let Some(r) = pid_rss_mb(pid) {
                    total_rss += r;
                    if r > max_rss {
                        max_rss = r;
                    }
                }
                if let Some(c) = pid_cpu_secs(pid) {
                    total_cpu += c;
                }
            }
        }
        let n = nodes.len() as u64;
        let avg_rss = if alive > 0 { total_rss / alive as f64 } else { 0.0 };
        let binary = sw.get("binary").and_then(|v| v.as_str()).unwrap_or("?");

        text.push_str(&format!(
            "\n  ── '{label}' ({binary})\n     nodes: {n}  alive: {alive}  dead: {}\n     RAM: {:.1} MB total · {:.1} MB/node avg · {:.1} MB peak\n     CPU: {:.1} cpu-seconds cumulative\n",
            n - alive, total_rss, avg_rss, max_rss, total_cpu
        ));
        // Density projection: how many of these fit in remaining host RAM.
        if avg_rss > 0.5 {
            text.push_str(&format!(
                "     density: ~{:.0} more nodes per free GB at {:.1} MB/node\n",
                1024.0 / avg_rss, avg_rss
            ));
        }

        json_out.insert(
            label.clone(),
            json!({
                "nodes": n, "alive": alive, "dead": n - alive,
                "total_rss_mb": total_rss, "avg_rss_mb": avg_rss, "peak_rss_mb": max_rss,
                "cpu_seconds": total_cpu,
                "nodes_per_free_gb": if avg_rss > 0.0 { 1024.0 / avg_rss } else { 0.0 }
            }),
        );
    }

    if format == "json" {
        serde_json::to_string_pretty(&Value::Object(json_out)).unwrap_or(text)
    } else {
        text
    }
}

// ── kill ──

fn send_signal(pid: i64, sig: i32) {
    // Use the `kill` syscall via libc-free route: /bin/kill. Avoids a libc dep
    // and respects the CLAUDE.md "kill by PID, never killall" rule.
    let _ = Command::new("kill")
        .arg(format!("-{sig}"))
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn flux_nodeswarm_kill(args: &Value) -> String {
    let all = args.get("all").and_then(|v| v.as_bool()).unwrap_or(false);
    let label = args.get("label").and_then(|v| v.as_str());
    let mut state = load_state();
    let swarms = state.get("swarms").and_then(|v| v.as_object()).cloned().unwrap_or_default();
    if swarms.is_empty() {
        return "🛰  No swarms to kill.".into();
    }

    let targets: Vec<String> = if all {
        swarms.keys().cloned().collect()
    } else if let Some(l) = label {
        vec![l.to_string()]
    } else {
        return "❌ pass label=<name> or all=true.".into();
    };

    let mut killed = 0u64;
    for t in &targets {
        if let Some(sw) = swarms.get(t) {
            let empty = vec![];
            let nodes = sw.get("nodes").and_then(|v| v.as_array()).unwrap_or(&empty);
            // SIGTERM pass.
            for n in nodes {
                if let Some(pid) = n.get("pid").and_then(|v| v.as_i64()) {
                    if pid_alive(pid) {
                        send_signal(pid, 15);
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
            // SIGKILL stragglers.
            for n in nodes {
                if let Some(pid) = n.get("pid").and_then(|v| v.as_i64()) {
                    if pid_alive(pid) {
                        send_signal(pid, 9);
                    }
                    killed += 1;
                }
            }
        }
        if let Some(obj) = state.get_mut("swarms").and_then(|v| v.as_object_mut()) {
            obj.remove(t);
        }
    }
    save_state(&state);
    format!("🛰  Killed swarm(s) [{}] — {killed} process(es) signalled (SIGTERM→SIGKILL).", targets.join(", "))
}

// ── logs ──

fn flux_nodeswarm_logs(args: &Value) -> String {
    let label = args.get("label").and_then(|v| v.as_str()).unwrap_or("default");
    let idx = args.get("idx").and_then(|v| v.as_u64()).unwrap_or(0);
    let lines = args.get("lines").and_then(|v| v.as_u64()).unwrap_or(40) as usize;
    let state = load_state();
    let log = state["swarms"][label]["nodes"]
        .as_array()
        .and_then(|a| a.iter().find(|n| n.get("idx").and_then(|v| v.as_u64()) == Some(idx)))
        .and_then(|n| n.get("log").and_then(|v| v.as_str()).map(String::from));
    let log = match log {
        Some(l) => l,
        None => return format!("❌ no log for swarm '{label}' node {idx}."),
    };
    let mut s = String::new();
    if let Ok(mut f) = fs::File::open(&log) {
        let _ = f.read_to_string(&mut s);
    } else {
        return format!("❌ cannot open {log}");
    }
    let tail: Vec<&str> = s.lines().rev().take(lines).collect();
    let body: Vec<&str> = tail.into_iter().rev().collect();
    format!("📜 {log} (last {} lines)\n{}", body.len(), body.join("\n"))
}
