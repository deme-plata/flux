// fluxc-core/distributed.rs — Phase 2 distributed build
// Round-robin crate distribution across Epsilon/Delta/Beta via SSH.
// Pure P2P gossipsub replacement: when flux-p2p workers are running on peers.

use crate::BuildConfig;

pub fn distributed_build(config: &BuildConfig) {
    let start = std::time::Instant::now();
    println!("⚡ Flux Distributed Build — P2P supercluster");

    let root = std::env::current_dir().unwrap_or_default();
    let ws = match flux_graph::resolve_workspace(&root) {
        Ok(w) => w,
        Err(e) => { eprintln!("flux-graph: {}", e); return; }
    };

    let peers: &[(&str, &str)] = &[
        ("delta", "root@5.79.79.158"),
        ("beta", "root@185.182.185.227"),
    ];

    println!("  Peers: local + {} remote (delta=8c, beta=18c)", peers.len());
    println!("  Workspace: {} crates in {} batches", ws.crates.len(), ws.batches.len());

    // rsync workspace to peers once
    let root_str = root.to_string_lossy();
    for (name, host) in peers {
        print!("  rsync {}... ", name);
        let s = std::process::Command::new("rsync")
            .args(["-azq", "--exclude=target", "--exclude=.git", &root_str,
                   &format!("{}:/tmp/flux-dist/", host)])
            .status();
        println!("{}", if s.map(|s| s.success()).unwrap_or(false) { "✓" } else { "✗" });
    }

    let total = ws.crates.len() as u64;
    let mut built = 0u64;
    let mut failed = false;
    let build_start = std::time::Instant::now();

    for batch in &ws.batches {
        let mut peer_idx = 0usize;
        let mut tasks = Vec::new();

        for &idx in batch {
            let ci = &ws.crates[idx];
            let (peer_name, host) = if peer_idx < peers.len() {
                peers[peer_idx]
            } else {
                ("local", "")
            };
            peer_idx = (peer_idx + 1) % (peers.len() + 1);

            let name = ci.name.clone();
            let host_str = host.to_string();
            let is_local = host_str.is_empty();
            let root_c = root_str.to_string();
            let rel = config.release;

            tasks.push(std::thread::spawn(move || {
                // SEC-1 (audit): `name` is interpolated into the remote ssh shell command below.
                // A malicious Cargo.toml [package] name like `foo; rm -rf ~` would be RCE on every
                // build peer. Cargo crate names are [A-Za-z0-9_-]; reject anything else BEFORE it can
                // reach the shell. Valid builds are unaffected; injection is impossible.
                if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                    eprintln!("⚠ SEC: refusing unsafe crate name {name:?} (distributed build)");
                    return (name, peer_name.to_string(), false);
                }
                if is_local {
                    let mut cmd = std::process::Command::new("cargo");
                    cmd.arg(if rel { "build" } else { "check" });
                    if rel { cmd.arg("--release"); }
                    cmd.args(["--package", &name]).current_dir(&root_c);
                    let ok = cmd.status().map(|s| s.success()).unwrap_or(false);
                    (name, peer_name.to_string(), ok)
                } else {
                    let cmd = format!("cd /tmp/flux-dist/flux && cargo {} --package {} 2>&1",
                        if rel { "build --release" } else { "check" }, name);
                    let out = std::process::Command::new("ssh")
                        .args(["-o", "ConnectTimeout=15", "-o", "StrictHostKeyChecking=no", &host_str, &cmd])
                        .output();
                    let ok = out.map(|o| o.status.success()).unwrap_or(false);
                    (name, peer_name.to_string(), ok)
                }
            }));
        }

        for t in tasks {
            match t.join() {
                Ok((name, peer, true)) => {
                    built += 1;
                    let pct = (built as f64 / total as f64 * 20.0) as usize;
                    eprint!("\r  [{}{}] {}/{} {} via {}", "=".repeat(pct),
                        " ".repeat(20usize.saturating_sub(pct)), built, total, name, peer);
                }
                Ok((name, peer, false)) => {
                    eprintln!("\n  ✗ {} failed on {}", name, peer);
                    failed = true;
                }
                Err(_) => eprintln!("\n  ✗ thread panicked"),
            }
        }
        if failed { break; }
    }

    eprint!("\n");
    let elapsed = build_start.elapsed().as_millis();
    if !failed {
        println!("✓ Distributed build complete in {}ms ({} crates across 3 machines)", elapsed, built);
    }
}
