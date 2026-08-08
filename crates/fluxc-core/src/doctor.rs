//! `fluxc doctor` — one-command triage for the "builds are suddenly slow /
//! combos fail mysteriously" class of incidents.
//!
//! Every check here is a failure mode that actually happened and cost a
//! session to diagnose:
//!   * wrapper-path drift → fingerprint universe split → minutes-long "no-op" builds
//!   * cache symlink dangling (and the `du -sh` false-empty trap)
//!   * poisoned `.rustc_info.json` after cross-target builds (jq/stdin trap)
//!   * long-running `fluxc mcp` servers holding a DELETED binary (fixes not live)
//!   * MCP server cwd outside the workspace (the UNVERIFIED-combos trap; now
//!     anchored in code, reported here for visibility)
//!   * missing fast-linker config (mold) → 50-70s relinks on fat binaries

use std::path::{Path, PathBuf};

struct Report {
    ok: usize,
    warn: usize,
    fail: usize,
}

impl Report {
    fn ok(&mut self, label: &str, detail: &str) {
        println!("  ✓ {label:<28} {detail}");
        self.ok += 1;
    }
    fn warn(&mut self, label: &str, detail: &str, fix: &str) {
        println!("  ⚠ {label:<28} {detail}\n      fix: {fix}");
        self.warn += 1;
    }
    fn fail(&mut self, label: &str, detail: &str, fix: &str) {
        println!("  ✗ {label:<28} {detail}\n      fix: {fix}");
        self.fail += 1;
    }
}

pub fn run() {
    println!("🩺 fluxc doctor — build-health triage\n");
    let mut r = Report { ok: 0, warn: 0, fail: 0 };
    let ws = fluxc_util::version::workspace_root();

    // 1. Workspace resolution.
    if ws.join("Cargo.toml").exists() {
        r.ok("workspace root", &ws.display().to_string());
    } else {
        r.fail(
            "workspace root",
            &format!("resolved to {} but no Cargo.toml there", ws.display()),
            "set Q_FLUX_WORKSPACE=/home/storage/deepseek-codewhale/flux",
        );
    }

    // 2. Live binary.
    let live = ws.join("target/debug/fluxc");
    match std::fs::metadata(&live) {
        Ok(meta) => {
            let age = meta
                .modified()
                .ok()
                .and_then(|m| m.elapsed().ok())
                .map(|d| d.as_secs() / 60)
                .unwrap_or(0);
            r.ok("live binary", &format!("{} ({}m old)", live.display(), age));
        }
        Err(_) => r.fail(
            "live binary",
            "target/debug/fluxc missing",
            "fluxc build --package fluxc (or fluxc self)",
        ),
    }

    // 3. Stale MCP servers holding a deleted binary.
    check_stale_mcp_servers(&mut r);

    // 4. Wrapper path — ONE fingerprint universe for the whole fleet.
    match std::env::var("FLUX_WRAPPER_PATH") {
        Ok(p) if !p.is_empty() => {
            let wp = Path::new(&p);
            if !wp.exists() {
                r.fail(
                    "FLUX_WRAPPER_PATH",
                    &format!("{p} does not exist"),
                    "ln -sf <ws>/target/debug/fluxc ~/.flux/bin/fluxc",
                );
            } else if same_file(wp, &live) {
                r.ok("FLUX_WRAPPER_PATH", &p);
            } else {
                r.warn(
                    "FLUX_WRAPPER_PATH",
                    &format!("{p} is NOT the live workspace binary"),
                    "point it at <ws>/target/debug/fluxc — a mismatch splits the cargo fingerprint universe (each build↔test alternation goes cold)",
                );
            }
        }
        _ => r.warn(
            "FLUX_WRAPPER_PATH",
            "unset — sessions may live in different fingerprint universes",
            "export FLUX_WRAPPER_PATH=$HOME/.flux/bin/fluxc (symlinked to the live binary)",
        ),
    }

    // 5. Shared content cache.
    check_cache(&mut r);

    // 6. Poisoned rustc probe caches.
    check_rustc_info(&mut r, &ws);

    // 7. Fast linker.
    check_linker(&mut r, &ws);

    // 8. cwd advisory (informational — spawns are anchored in code since v0.40).
    if let Ok(cwd) = std::env::current_dir() {
        if cwd.starts_with(&ws) {
            r.ok("cwd", &cwd.display().to_string());
        } else {
            r.ok("cwd", &format!("{} (outside ws — fine, spawns are anchored since v0.40)", cwd.display()));
        }
    }

    println!(
        "\n  {} ok · {} warnings · {} failures{}",
        r.ok,
        r.warn,
        r.fail,
        if r.fail == 0 && r.warn == 0 { " — build health nominal 🟢" } else { "" }
    );
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

fn check_stale_mcp_servers(r: &mut Report) {
    let mut stale = Vec::new();
    let mut live_count = 0usize;
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for e in entries.flatten() {
            let pid = e.file_name().to_string_lossy().to_string();
            if !pid.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let Ok(cmdline) = std::fs::read(format!("/proc/{pid}/cmdline")) else { continue };
            let cmd = String::from_utf8_lossy(&cmdline).replace('\0', " ");
            if !(cmd.contains("fluxc") && cmd.contains(" mcp")) {
                continue;
            }
            live_count += 1;
            if let Ok(exe) = std::fs::read_link(format!("/proc/{pid}/exe")) {
                if exe.to_string_lossy().ends_with(" (deleted)") {
                    stale.push(pid);
                }
            }
        }
    }
    if stale.is_empty() {
        r.ok("mcp servers", &format!("{live_count} running, none stale"));
    } else {
        r.warn(
            "mcp servers",
            &format!("{} of {live_count} hold a DELETED (pre-rebuild) binary: pids {}", stale.len(), stale.join(", ")),
            "restart those sessions' MCP (/mcp) — their combos run OLD code",
        );
    }
}

fn check_cache(r: &mut Report) {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    let cache = std::env::var("FLUX_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| Path::new(&home).join(".flux/cache"));
    if !cache.exists() {
        r.warn(
            "content cache",
            &format!("{} missing", cache.display()),
            "first build recreates it; if it was a symlink, restore the target (/home/storage/flux-shared-cache)",
        );
        return;
    }
    // A symlink whose target vanished still "exists" via symlink_metadata only.
    let link_note = std::fs::read_link(&cache)
        .map(|t| format!(" → {}", t.display()))
        .unwrap_or_default();
    let hits = counter_size(Path::new(&home).join(".flux/.cache-unit-hits"));
    let misses = counter_size(Path::new(&home).join(".flux/.cache-unit-misses"));
    let rate = if hits + misses > 0 {
        format!("{}% unit hit rate ({hits} hits / {misses} misses)", hits * 100 / (hits + misses))
    } else {
        "no counter data yet".into()
    };
    r.ok("content cache", &format!("{}{link_note} · {rate}", cache.display()));
}

fn counter_size(p: PathBuf) -> u64 {
    // 1 byte appended per event — file SIZE is the count (du lies on sparse).
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

fn check_rustc_info(r: &mut Report, ws: &Path) {
    let mut poisoned = Vec::new();
    let mut candidates = vec![ws.join("target/.rustc_info.json")];
    if let Ok(entries) = std::fs::read_dir(ws.join("target")) {
        for e in entries.flatten() {
            let p = e.path().join(".rustc_info.json");
            if p.exists() {
                candidates.push(p);
            }
        }
    }
    for c in candidates {
        if let Ok(text) = std::fs::read_to_string(&c) {
            if text.contains("\"success\":false") || text.contains("unclosed") {
                poisoned.push(c.display().to_string());
            }
        }
    }
    if poisoned.is_empty() {
        r.ok("rustc probe cache", "clean");
    } else {
        r.fail(
            "rustc probe cache",
            &format!("poisoned: {}", poisoned.join(", ")),
            "delete the poisoned .rustc_info.json (cargo replays the cached failure on every cross build)",
        );
    }
}

fn check_linker(r: &mut Report, ws: &Path) {
    let cfg = ws.join(".cargo/config.toml");
    let has_fast = std::fs::read_to_string(&cfg)
        .map(|t| t.contains("mold") || t.contains("lld"))
        .unwrap_or(false);
    let mold_installed = std::process::Command::new("which")
        .arg("mold")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    match (has_fast, mold_installed) {
        (true, true) => r.ok("fast linker", "mold configured and installed"),
        (true, false) => r.fail(
            "fast linker",
            ".cargo/config.toml references mold/lld but the binary is missing",
            "apt-get install mold (or clang lld)",
        ),
        (false, _) => r.warn(
            "fast linker",
            "no mold/lld in .cargo/config.toml — fat-binary links run 50-70s on bfd",
            "add [target.x86_64-unknown-linux-gnu] rustflags=[\"-C\",\"link-arg=-fuse-ld=mold\"] (NOTE: changes the fingerprint → one cold rebuild for everyone)",
        ),
    }
}
