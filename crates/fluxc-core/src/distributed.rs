// fluxc-core/distributed.rs — Phase 2 distributed build v2
//
// REAL distributed compilation across the fleet (Epsilon/Delta/Gamma/Beta + local).
//
// Why v1 didn't actually distribute: every node ran `cargo build -p <crate>` into
// its OWN local target dir, so a crate built on node A was invisible to a dependent
// crate built on node B. cargo therefore rebuilt each crate's full dependency
// closure locally on whichever node it landed — redundant work, zero speedup.
//
// v2 makes it correct + actually parallel:
//   1. Probe the fleet: ssh-reachable, cargo present, AND rustc version == coordinator's.
//      A toolchain mismatch breaks cargo fingerprint portability (synced artifacts get
//      rebuilt), which silently degrades back to redundant builds — so we SKIP mismatched
//      nodes loudly instead of pretending. Pin the fleet with rust-toolchain.toml.
//   2. Sync the workspace source to the IDENTICAL absolute path on every node. Same source
//      path + same shared absolute target-dir + same rustc => cargo fingerprints port, so a
//      crate compiled on one node is accepted as fresh on the others.
//   3. Build batch-by-batch in topological order. Crates within a batch are independent, so
//      they are round-robined across {local + remotes} and compiled in parallel. After each
//      batch there is a BARRIER: pull every remote's new target artifacts back to the
//      coordinator and push the merged set out again, so the next batch sees all deps.
//   4. Assemble the requested package on the coordinator at the end.

use crate::BuildConfig;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

/// A build node. `host` empty => the local coordinator.
#[derive(Clone)]
struct Node {
    name: String,
    host: String,
    local: bool,
}

/// Default fleet (overridable via FLUX_DIST_PEERS="name=user@host,name=user@host").
/// Local is always added implicitly.
const DEFAULT_PEERS: &[(&str, &str)] = &[
    ("delta", "root@5.79.79.158"),
    ("gamma", "root@109.205.176.60"),
    ("beta", "root@185.182.185.227"),
];

pub fn distributed_build(config: &BuildConfig) {
    let t0 = Instant::now();
    println!("⚡ Flux Distributed Build v2 — P2P supercluster");

    let root = std::env::current_dir().unwrap_or_default();
    let ws = match flux_graph::resolve_workspace(&root) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("flux-graph: {e}");
            return;
        }
    };
    let root_str = root.to_string_lossy().to_string();
    let profile = if config.release { "release" } else { "debug" };
    let target_dir = detect_target_dir(&root);
    let profile_dir = target_dir.join(profile);

    // Closure-pruning: when a package is requested, build ONLY its dependency
    // closure (the crate + its transitive workspace path-deps), not all 69 crates.
    // Keeps targeted distributed builds fast and isolates them from unrelated broken
    // crates (e.g. a `sigil-header` build no longer drags in `sigil-book`).
    let selected = config.package.as_deref().and_then(|p| dep_closure(&ws, p));
    let batches: Vec<Vec<usize>> = ws
        .batches
        .iter()
        .map(|b| {
            b.iter()
                .copied()
                .filter(|i| selected.as_ref().map(|s| s.contains(i)).unwrap_or(true))
                .collect::<Vec<usize>>()
        })
        .filter(|b| !b.is_empty())
        .collect();
    let scope = match (config.package.as_deref(), &selected) {
        (Some(p), Some(s)) => format!("'{p}' closure ({} crates)", s.len()),
        (Some(p), None) => format!("'{p}' not in workspace → full"),
        (None, _) => "full workspace".to_string(),
    };
    println!(
        "  Workspace: {} crates · scope: {} · {} batches · target={}",
        ws.crates.len(),
        scope,
        batches.len(),
        target_dir.display()
    );

    // ── Phase 1: discover reachable peers + sync source to the IDENTICAL path ──
    // Source must live at the same absolute path on every node (carrying the same
    // rust-toolchain.toml) so cargo fingerprints port across the fleet.
    let parent = root.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    // Cross-workspace path-deps (e.g. sigil → ../../../flux) must also exist at the
    // SAME absolute path on every node, or remotes can't resolve them. Detect the
    // sibling workspace roots referenced by out-of-tree path-deps and sync them too.
    let siblings = sibling_workspace_roots(&ws, &root);
    if !siblings.is_empty() {
        println!(
            "  sibling path-dep roots: {}",
            siblings.iter().map(|s| s.display().to_string()).collect::<Vec<_>>().join(", ")
        );
    }
    let src_excludes = ["--exclude=target", "--exclude=.git", "--exclude=node_modules", "--exclude=.target-shared"];
    // The coordinator is already in the fleet as `local`; never SSH-dial ourselves
    // (that produced a cosmetic "epsilon unreachable" when the peer list named this host).
    let locals = local_ips();
    let mut reachable: Vec<(String, String)> = Vec::new();
    for (name, host) in parse_peers() {
        let host_ip = host.rsplit('@').next().unwrap_or(&host).to_string();
        if locals.iter().any(|ip| ip == &host_ip) {
            continue; // skip self
        }
        print!("  sync src → {name} ({host}) ... ");
        let _ = ssh_run(&host, &format!("mkdir -p {}", shell_quote(&parent)));
        let mut all_ok = true;
        for sib in &siblings {
            let sib_str = sib.to_string_lossy().to_string();
            let sib_parent = sib.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
            let _ = ssh_run(&host, &format!("mkdir -p {}", shell_quote(&sib_parent)));
            if !rsync(&format!("{sib_str}/"), &format!("{host}:{sib_str}/"), &src_excludes) {
                all_ok = false;
            }
        }
        let ok = all_ok
            && rsync(&format!("{root_str}/"), &format!("{host}:{root_str}/"), &src_excludes);
        println!("{}", if ok { "✓" } else { "✗ (unreachable, dropped)" });
        if ok {
            reachable.push((name, host));
        }
    }

    // ── Phase 2: toolchain gate (measured IN the workspace so rust-toolchain.toml applies) ──
    let local_rustc = rustc_version_in(&root_str);
    println!(
        "  Coordinator rustc (in-workspace): {}",
        local_rustc.as_deref().unwrap_or("unknown")
    );
    let mut nodes: Vec<Node> = vec![Node { name: "local".into(), host: String::new(), local: true }];
    for (name, host) in reachable {
        match probe_remote(&host, &root_str) {
            Ok(rv) if Some(&rv) == local_rustc.as_ref() => {
                println!("  ✓ {name} ready — rustc match ({rv})");
                nodes.push(Node { name, host, local: false });
            }
            Ok(rv) => eprintln!(
                "  ⚠ {name} SKIPPED — rustc {rv} ≠ coordinator {}. Pin rust-toolchain.toml + install rustup on the node.",
                local_rustc.as_deref().unwrap_or("?")
            ),
            Err(e) => eprintln!("  ✗ {name} probe failed: {e}"),
        }
    }
    let remotes: Vec<Node> = nodes.iter().filter(|n| !n.local).cloned().collect();
    println!("  Active fleet: {} node(s) ({} remote + local)", nodes.len(), remotes.len());

    if remotes.is_empty() {
        eprintln!("  ⚠ No toolchain-matched remotes — falling back to a local build.");
        let _ = local_build(&root_str, config.package.as_deref(), config.release);
        return;
    }

    // NOTE: we deliberately do NOT warm-push the whole pre-existing target up front.
    // That was a multi-GB rsync that threw `rsync error 255` (SSH transport) and
    // aborted the build. Instead each node builds shared deps locally as needed, and
    // ONLY the per-batch delta is reconciled (small, resilient). Set
    // FLUX_DIST_WARM_PUSH=1 to opt back into the eager push.
    if std::env::var("FLUX_DIST_WARM_PUSH").map(|v| v == "1").unwrap_or(false) && profile_dir.exists() {
        for n in &remotes {
            let _ = ssh_run(&n.host, &format!("mkdir -p {}", shell_quote(&profile_dir.to_string_lossy())));
            let _ = push_target(&n.host, &profile_dir);
        }
    } else {
        for n in &remotes {
            let _ = ssh_run(&n.host, &format!("mkdir -p {}", shell_quote(&profile_dir.to_string_lossy())));
        }
        println!("  (warm-push skipped — per-batch delta reconcile only; FLUX_DIST_WARM_PUSH=1 to force)");
    }

    // ── Phase 2: batch-barrier distributed compile ──
    let total = batches.iter().map(|b| b.len()).sum::<usize>() as u64;
    let mut built = 0u64;
    let mut failed: Option<String> = None;

    'batches: for (bi, batch) in batches.iter().enumerate() {
        let mut handles = Vec::new();
        for (j, &idx) in batch.iter().enumerate() {
            let ci = &ws.crates[idx];
            let crate_name = ci.name.clone();
            // SEC-1: crate name is interpolated into a remote shell. Cargo names are
            // [A-Za-z0-9_-]; reject anything else before it can reach ssh (RCE guard).
            if crate_name.is_empty()
                || !crate_name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                eprintln!("⚠ SEC: refusing unsafe crate name {crate_name:?}");
                failed = Some(crate_name);
                break 'batches;
            }
            // Round-robin this batch's crates across the active fleet.
            let node = nodes[j % nodes.len()].clone();
            let root_c = root_str.clone();
            let rel = config.release;
            handles.push(std::thread::spawn(move || {
                let ok = if node.local {
                    local_build(&root_c, Some(&crate_name), rel).is_ok()
                } else {
                    let cmd = format!(
                        "cd {} && cargo build {} --package {} 2>&1 | tail -3",
                        shell_quote(&root_c),
                        if rel { "--release" } else { "" },
                        crate_name
                    );
                    ssh_run(&node.host, &cmd).map(|o| !o.contains("error[") && !o.contains("error:")).unwrap_or(false)
                };
                (crate_name, node.name, ok)
            }));
        }

        // Barrier: collect this batch's results.
        for h in handles {
            match h.join() {
                Ok((name, node, true)) => {
                    built += 1;
                    let pct = (built as f64 / total as f64 * 24.0) as usize;
                    eprint!(
                        "\r  [{}{}] {}/{}  batch {}  {} @ {}        ",
                        "█".repeat(pct),
                        "░".repeat(24usize.saturating_sub(pct)),
                        built, total, bi, name, node
                    );
                }
                Ok((name, node, false)) => {
                    eprintln!("\n  ✗ {name} failed on {node}");
                    failed = Some(name);
                }
                Err(_) => eprintln!("\n  ✗ build thread panicked"),
            }
        }
        if failed.is_some() {
            break;
        }

        // Reconcile artifacts: pull each remote's new outputs, then push the merged
        // set back out so the NEXT batch's dependents can link against them.
        for n in &remotes {
            let _ = pull_target(&n.host, &profile_dir);
        }
        for n in &remotes {
            let _ = push_target(&n.host, &profile_dir);
        }
    }
    eprintln!();

    // ── Phase 3: assemble the requested package locally ──
    if failed.is_none() {
        if let Some(pkg) = &config.package {
            print!("  assembling {pkg} on coordinator ... ");
            let ok = local_build(&root_str, Some(pkg), config.release).is_ok();
            println!("{}", if ok { "✓" } else { "✗" });
            if !ok {
                failed = Some(pkg.clone());
            }
        }
    }

    let ms = t0.elapsed().as_millis();
    match &failed {
        None => {
            println!(
                "✓ Distributed build complete in {ms}ms — {built}/{total} crates across {} node(s)",
                nodes.len()
            );
            // flux-aether verifiable build receipt: content-addressed (BLAKE3),
            // K-of-N sharded, mesh-distributable. Provenance for the reconciliation.
            let node_names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
            if let Some(root) = aether_build_receipt(
                config.package.as_deref().unwrap_or("(workspace)"),
                profile,
                built,
                total,
                &node_names,
                ms as u64,
            ) {
                println!("  📦 aether build receipt: content_root {}", &root[..16.min(root.len())]);
            }
        }
        Some(c) => eprintln!("✗ Distributed build FAILED at crate '{c}' after {ms}ms"),
    }
}

/// Shard a small JSON build receipt into flux-aether (content-addressed, BLAKE3,
/// K-of-N encrypted) and round-trip-verify it. Returns the hex content_root. This
/// is the verifiable-artifact lever the platform skill prescribes — provenance for
/// a distributed build without trusting any single node. Best-effort (None on error).
fn aether_build_receipt(
    package: &str,
    profile: &str,
    built: u64,
    total: u64,
    nodes: &[&str],
    elapsed_ms: u64,
) -> Option<String> {
    let receipt = serde_json::json!({
        "kind": "flux-distributed-build-receipt",
        "package": package,
        "profile": profile,
        "crates_built": built,
        "crates_total": total,
        "fleet": nodes,
        "elapsed_ms": elapsed_ms,
    });
    let bytes = serde_json::to_vec(&receipt).ok()?;
    let key = std::env::var("FLUX_AETHER_KEY")
        .map(|s| s.into_bytes())
        .unwrap_or_else(|_| b"flux-aether-v0-cluster".to_vec());
    let producer = *blake3::hash(b"fluxc-distributed").as_bytes();
    let (fb, shards) = flux_aether::shard_file(&bytes, 65536, &key, producer);
    // Verify-don't-trust: reassemble from the shards and confirm it matches.
    let ok = flux_aether::reassemble(&fb, &shards, &key).map(|b| b == bytes).unwrap_or(false);
    if !ok {
        return None;
    }
    Some(hex::encode(fb.content_root))
}

// ── helpers ──

/// Local machine IPs (from `hostname -I`), used to skip self-dialing a peer that
/// names this coordinator. Empty on failure (then nothing is treated as self).
fn local_ips() -> Vec<String> {
    Command::new("hostname")
        .arg("-I")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Indices of the dependency closure of `package` — the crate plus all its
/// transitive workspace path-deps. None if the package isn't in the workspace
/// (caller then builds everything). Forward-edge BFS over path-deps.
fn dep_closure(ws: &flux_graph::WorkspaceGraph, package: &str) -> Option<HashSet<usize>> {
    let idx: HashMap<&str, usize> =
        ws.crates.iter().enumerate().map(|(i, c)| (c.name.as_str(), i)).collect();
    let start = *idx.get(package)?;
    let n = ws.crates.len();
    let mut fwd: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, c) in ws.crates.iter().enumerate() {
        for d in &c.dependencies {
            if d.path.is_some() {
                if let Some(&j) = idx.get(d.name.as_str()) {
                    if j != i {
                        fwd[i].push(j);
                    }
                }
            }
        }
    }
    let mut seen = HashSet::new();
    seen.insert(start);
    let mut q = VecDeque::from([start]);
    while let Some(node) = q.pop_front() {
        for &j in &fwd[node] {
            if seen.insert(j) {
                q.push_back(j);
            }
        }
    }
    Some(seen)
}

/// Parse FLUX_DIST_PEERS env or fall back to DEFAULT_PEERS.
fn parse_peers() -> Vec<(String, String)> {
    if let Ok(spec) = std::env::var("FLUX_DIST_PEERS") {
        let v: Vec<(String, String)> = spec
            .split(',')
            .filter_map(|e| {
                let mut it = e.splitn(2, '=');
                match (it.next(), it.next()) {
                    (Some(n), Some(h)) if !n.trim().is_empty() && !h.trim().is_empty() => {
                        Some((n.trim().to_string(), h.trim().to_string()))
                    }
                    _ => None,
                }
            })
            .collect();
        if !v.is_empty() {
            return v;
        }
    }
    DEFAULT_PEERS.iter().map(|(n, h)| (n.to_string(), h.to_string())).collect()
}

/// `rustc --version` measured FROM the workspace dir, so a rust-toolchain.toml pin
/// applies (the whole line, so nightly/commit differences count).
fn rustc_version_in(dir: &str) -> Option<String> {
    let out = Command::new("rustc")
        .arg("--version")
        .current_dir(dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Probe a remote: must be ssh-reachable AND have cargo+rustc resolvable IN the
/// synced workspace dir (so its rust-toolchain.toml pin is honored). Returns the
/// effective rustc version.
fn probe_remote(host: &str, root: &str) -> Result<String, String> {
    let cmd = format!(
        "cd {} && command -v cargo >/dev/null && rustc --version",
        shell_quote(root)
    );
    let out = Command::new("ssh")
        .args([
            "-o", "ConnectTimeout=10",
            "-o", "BatchMode=yes",
            "-o", "StrictHostKeyChecking=no",
            host,
            &format!("bash -lc {}", shell_quote(&cmd)),
        ])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("ssh failed or no cargo/rustc on PATH".into());
    }
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if v.is_empty() {
        return Err("empty rustc version".into());
    }
    Ok(v)
}

/// Run a remote command, returning combined stdout+stderr.
fn ssh_run(host: &str, cmd: &str) -> Result<String, String> {
    let out = Command::new("ssh")
        .args([
            "-o", "ConnectTimeout=15",
            "-o", "BatchMode=yes",
            "-o", "StrictHostKeyChecking=no",
            host,
            &format!("bash -lc {}", shell_quote(cmd)),
        ])
        .output()
        .map_err(|e| e.to_string())?;
    let mut s = String::from_utf8_lossy(&out.stdout).to_string();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok(s)
}

fn local_build(root: &str, package: Option<&str>, release: bool) -> Result<(), ()> {
    let mut cmd = Command::new("cargo");
    cmd.arg("build");
    if release {
        cmd.arg("--release");
    }
    if let Some(p) = package {
        cmd.args(["--package", p]);
    }
    cmd.current_dir(root);
    match cmd.status() {
        Ok(s) if s.success() => Ok(()),
        _ => Err(()),
    }
}

/// Resilient rsync: keep-alive SSH (survives idle stalls that throw error 255),
/// `--partial --append-verify` (resume interrupted transfers, content-verify on
/// resume), and up to 3 attempts with backoff. Returns success. This is the fix
/// for the `rsync error 255` that aborted distributed builds on large transfers.
fn rsync(src: &str, dst: &str, extra: &[&str]) -> bool {
    const SSH: &str = "ssh -o BatchMode=yes -o StrictHostKeyChecking=no \
                       -o ConnectTimeout=20 -o ServerAliveInterval=15 \
                       -o ServerAliveCountMax=8 -o TCPKeepAlive=yes";
    for attempt in 0..3 {
        let mut c = Command::new("rsync");
        // No aggressive --timeout (that aborted big trees mid-stream); keepalive
        // handles dead links. --partial keeps progress across retries.
        c.args(["-az", "--partial", "--append-verify", "-e", SSH]);
        c.args(extra);
        c.args([src, dst]);
        if c.status().map(|s| s.success()).unwrap_or(false) {
            return true;
        }
        if attempt < 2 {
            std::thread::sleep(std::time::Duration::from_secs(2 * (attempt as u64 + 1)));
        }
    }
    false
}

/// Pull a remote profile dir's artifacts back to the coordinator (merge, no delete).
/// Excludes `incremental/` (per-machine, huge, never portable).
fn pull_target(host: &str, profile_dir: &PathBuf) -> bool {
    let p = profile_dir.to_string_lossy();
    rsync(&format!("{host}:{p}/"), &format!("{p}/"), &["--exclude=incremental/"])
}

/// Push the merged profile dir out to a remote so the next batch sees all deps.
fn push_target(host: &str, profile_dir: &PathBuf) -> bool {
    let p = profile_dir.to_string_lossy();
    rsync(&format!("{p}/"), &format!("{host}:{p}/"), &["--exclude=incremental/"])
}

/// Resolve the cargo target dir: CARGO_TARGET_DIR > .cargo/config(.toml) build.target-dir > root/target.
fn detect_target_dir(root: &PathBuf) -> PathBuf {
    if let Ok(d) = std::env::var("CARGO_TARGET_DIR") {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    for cfg in [root.join(".cargo/config.toml"), root.join(".cargo/config")] {
        if let Ok(text) = std::fs::read_to_string(&cfg) {
            if let Some(d) = parse_target_dir(&text) {
                return PathBuf::from(d);
            }
        }
    }
    root.join("target")
}

/// Minimal extractor for `target-dir = "..."` under a [build] table.
fn parse_target_dir(toml: &str) -> Option<String> {
    let mut in_build = false;
    for line in toml.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_build = l == "[build]";
            continue;
        }
        if in_build {
            if let Some(rest) = l.strip_prefix("target-dir") {
                if let Some(eq) = rest.split_once('=') {
                    let v = eq.1.trim().trim_matches('"').trim();
                    if !v.is_empty() {
                        return Some(v.to_string());
                    }
                }
            }
        }
    }
    None
}

/// Single-quote a string for safe embedding in a remote `bash -lc '...'`.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Collect the unique sibling workspace roots referenced by path-deps that point
/// OUTSIDE this workspace (e.g. sigil's `../../../flux/crates/...`). Each must be
/// synced to the same absolute path on remotes for dep resolution to work.
fn sibling_workspace_roots(ws: &flux_graph::WorkspaceGraph, root: &Path) -> Vec<PathBuf> {
    let mut set: Vec<PathBuf> = Vec::new();
    for ci in &ws.crates {
        for dep in &ci.dependencies {
            let Some(p) = &dep.path else { continue };
            let abs = if p.is_absolute() { p.clone() } else { ci.path.join(p) };
            let abs = std::fs::canonicalize(&abs).unwrap_or(abs);
            if abs.starts_with(root) {
                continue;
            }
            if let Some(wsr) = find_workspace_root(&abs) {
                if wsr != *root && !set.contains(&wsr) {
                    set.push(wsr);
                }
            }
        }
    }
    set
}

/// Walk up from `start` to the nearest ancestor whose Cargo.toml declares `[workspace]`.
fn find_workspace_root(start: &Path) -> Option<PathBuf> {
    let mut cur = if start.is_file() { start.parent() } else { Some(start) };
    while let Some(d) = cur {
        if let Ok(t) = std::fs::read_to_string(d.join("Cargo.toml")) {
            if t.contains("[workspace]") {
                return Some(d.to_path_buf());
            }
        }
        cur = d.parent();
    }
    None
}

#[cfg(test)]
mod shell_safety_tests {
    //! shell_quote is the RCE boundary for remote `bash -lc '...'` (Tier 3). A
    //! regression here is a fleet-wide command-injection vector — so we pin the
    //! exact POSIX single-quote escaping, including adversarial inputs.
    use super::{parse_target_dir, shell_quote};

    #[test]
    fn shell_quote_wraps_and_escapes_single_quotes() {
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("abc"), "'abc'");
        assert_eq!(shell_quote("a b"), "'a b'");
        // a single quote becomes the 4-char POSIX sequence '\'' — never bare.
        assert_eq!(shell_quote("'"), r"''\'''");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn shell_quote_neutralizes_injection() {
        // The classic break-out attempt: the embedded `'; rm -rf /` must end up
        // INSIDE the quoting, with every quote escaped — not a command separator.
        let evil = "a'; rm -rf /";
        assert_eq!(shell_quote(evil), r"'a'\''; rm -rf /'");
        let q = shell_quote(evil);
        assert!(q.starts_with('\'') && q.ends_with('\''), "always fully single-quoted");
        // No bare single-quote survives: every ' is part of the '\'' escape.
        assert!(!q.contains("' '"), "no unescaped quote-space-quote breakout");
    }

    #[test]
    fn parse_target_dir_reads_build_table_only() {
        assert_eq!(parse_target_dir("[build]\ntarget-dir = \"/tmp/x\""), Some("/tmp/x".into()));
        assert_eq!(parse_target_dir("[build]\ntarget-dir=unquoted"), Some("unquoted".into()));
        // first match wins, even with a later section redefining it.
        assert_eq!(
            parse_target_dir("[build]\ntarget-dir=\"a\"\n[other]\ntarget-dir=\"b\""),
            Some("a".into())
        );
        // target-dir outside [build] is ignored.
        assert_eq!(parse_target_dir("[other]\ntarget-dir=\"z\""), None);
        // [build] present but no target-dir, and empty value → None.
        assert_eq!(parse_target_dir("[build]\njobs = 4"), None);
        assert_eq!(parse_target_dir("[build]\ntarget-dir = \"\""), None);
        assert_eq!(parse_target_dir(""), None);
    }
}
