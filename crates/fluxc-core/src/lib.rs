// fluxc-core — Shared build engine, serve, tune, webhook, predict, qspec, quantum_architect
//
// This is the core library crate. fluxc-mcp depends on it for tool implementations.
// fluxc (CLI) depends on it for build orchestration.

pub mod serve;
pub mod tdg;
pub mod tdg_sched;
pub mod serve_stats;
pub mod serve_events;
pub mod serve_router;
pub mod combo_v2;
pub mod warm_bin_cache;
pub mod tune;
pub mod portal;
pub mod goals;
pub mod ai_debug;
pub use ::flux_net; // P1: extracted to its own flux-net crate; re-export keeps fluxc_core::flux_net::* stable
pub mod webhook;
pub mod webhook_ssrf;
pub mod webhook_inbound;
pub mod predict;
pub mod qspec;
pub use ::flux_quantum_architect as quantum_architect; // P1: extracted to own crate; re-export keeps fluxc_core::quantum_architect::* stable
pub mod cortex;
pub mod self_heal;
pub mod benchmark;
pub mod heatmap;
pub mod version;
pub mod phase3;
pub mod distributed;
pub mod p2p_worker;
pub use distributed::distributed_build;
pub mod chat;
pub mod swarm;
pub mod provenance;
pub mod xray;
pub mod fix;
pub mod release_audit;

use std::env;
use std::path::PathBuf;
use std::process::{self, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

// ── Build Configuration ──

#[derive(Debug, Clone)]
pub struct BuildConfig {
    pub release: bool,
    pub rust_only: bool,
    pub frontend_only: bool,
    pub package: Option<String>,
    pub no_cargo: bool,
    pub async_build: bool,
    pub distributed: bool,
    pub warm: bool,
    /// Emit a `.o.proof` file alongside every compiled artifact (Phase 2j killer feature).
    pub provenance: bool,
}

impl Default for BuildConfig {
    fn default() -> Self {
        BuildConfig {
            release: false, rust_only: false, frontend_only: false, package: None,
            no_cargo: false, async_build: false, distributed: false, warm: false,
            provenance: false,
        }
    }
}

// ── Global Stats ──

pub static CACHE_HITS: AtomicU64 = AtomicU64::new(0);
pub static CACHE_MISSES: AtomicU64 = AtomicU64::new(0);
pub static BUILD_COUNT: AtomicU64 = AtomicU64::new(0);
pub static TOTAL_BUILD_TIME_MS: AtomicU64 = AtomicU64::new(0);

// ── Persistent Stats (disk-backed, survives process restarts) ──

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PersistentStats {
    pub total_builds: u64,
    pub total_cache_hits: u64,
    pub total_cache_misses: u64,
    pub total_build_time_ms: u64,
    pub sessions: u64,
}

pub fn stats_path() -> PathBuf {
    let home = env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".flux").join("stats.json")
}

pub fn load_persistent_stats() -> PersistentStats {
    let path = stats_path();
    if let Ok(data) = std::fs::read_to_string(&path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
            return PersistentStats {
                total_builds: v.get("total_builds").and_then(|x| x.as_u64()).unwrap_or(0),
                total_cache_hits: v.get("total_cache_hits").and_then(|x| x.as_u64()).unwrap_or(0),
                total_cache_misses: v.get("total_cache_misses").and_then(|x| x.as_u64()).unwrap_or(0),
                total_build_time_ms: v.get("total_build_time_ms").and_then(|x| x.as_u64()).unwrap_or(0),
                sessions: v.get("sessions").and_then(|x| x.as_u64()).unwrap_or(0),
            };
        }
    }
    PersistentStats::default()
}

pub fn save_persistent_stats(stats: &PersistentStats) {
    let path = stats_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let json = serde_json::json!({
        "total_builds": stats.total_builds,
        "total_cache_hits": stats.total_cache_hits,
        "total_cache_misses": stats.total_cache_misses,
        "total_build_time_ms": stats.total_build_time_ms,
        "sessions": stats.sessions,
    });
    if let Ok(s) = serde_json::to_string_pretty(&json) {
        let _ = std::fs::write(&path, s);
    }
}

pub fn persist_build(hits: u64, misses: u64, build_time_ms: u64) {
    let mut s = load_persistent_stats();
    s.total_builds += 1;
    s.total_cache_hits += hits;
    s.total_cache_misses += misses;
    s.total_build_time_ms += build_time_ms;
    s.sessions += 1;
    save_persistent_stats(&s);
}

// ── Compilation-unit cache instrumentation (Hennessy & Patterson: measure the cache
// at its true granularity). The content-addressed cache lives in `wrapper_mode`, which
// cargo spawns as a short-lived `fluxc` subprocess PER rustc invocation — so it can't
// share the in-memory CACHE_* atomics or report back. Before this, the only writer of
// CACHE_HITS was the frontend path, so the Rust cache hit-rate was a permanently-dead
// 0/N counter. Fix: each wrapper subprocess appends ONE byte per outcome to a hits/miss
// file. A single-byte append is atomic well under PIPE_BUF, so N parallel wrappers race
// cleanly and the file length == the event count. Build paths read the DELTA across a
// build to get that build's real per-unit hit/miss. ──

fn cache_events_path(hit: bool) -> PathBuf {
    let home = env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let name = if hit { ".cache-unit-hits" } else { ".cache-unit-misses" };
    PathBuf::from(home).join(".flux").join(name)
}

/// Record one compilation-unit cache outcome. Called from `wrapper_mode` (the rustc
/// wrapper subprocess). Lock-free: a 1-byte O_APPEND is atomic across parallel writers.
pub fn record_cache_event(hit: bool) {
    use std::io::Write;
    let path = cache_events_path(hit);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(b"\x01");
    }
}

/// (hits, misses) compilation-unit cache events recorded so far (file length == count).
pub fn cache_event_counts() -> (u64, u64) {
    let h = std::fs::metadata(cache_events_path(true)).map(|m| m.len()).unwrap_or(0);
    let m = std::fs::metadata(cache_events_path(false)).map(|m| m.len()).unwrap_or(0);
    (h, m)
}

// ── Project Detection ──

#[derive(Debug, PartialEq)]
pub enum ProjectType { Rust, Frontend, FullStack, LaTeX, Unknown }

pub fn detect_project() -> ProjectType {
    let has_cargo = PathBuf::from("Cargo.toml").exists();
    let has_pkg = PathBuf::from("package.json").exists();
    let has_tex = std::fs::read_dir(".")
        .map(|entries| entries.flatten().any(|e| e.path().extension().map_or(false, |ext| ext == "tex")))
        .unwrap_or(false);
    match (has_cargo, has_pkg, has_tex) {
        (true, true, _) => ProjectType::FullStack,
        (true, false, _) => ProjectType::Rust,
        (false, true, _) => ProjectType::Frontend,
        (false, false, true) => ProjectType::LaTeX,
        _ => ProjectType::Unknown,
    }
}

pub fn effective_project_type(config: &BuildConfig) -> ProjectType {
    let detected = detect_project();
    match (config.rust_only, config.frontend_only) {
        (true, false) => {
            if detected == ProjectType::Frontend {
                eprintln!("warning: --rust-only used but no Cargo.toml found");
            }
            ProjectType::Rust
        }
        (false, true) => {
            if detected == ProjectType::Rust {
                eprintln!("warning: --frontend-only used but no package.json found");
            }
            ProjectType::Frontend
        }
        _ => detected,
    }
}

// ── Build Orchestrator ──

pub fn detect_and_build(config: &BuildConfig, extra: &[String]) {
    let start = std::time::Instant::now();
    BUILD_COUNT.fetch_add(1, Ordering::Relaxed);
    // Snapshot the compilation-unit cache event counters so we can read this build's
    // real per-unit hit/miss as the delta below (the wrapper subprocesses append to them).
    let (ch0, cm0) = cache_event_counts();

    // `--distributed` (and `--release`/`--package`) may arrive as SUBCOMMAND flags in
    // `extra` (e.g. `fluxc build --distributed --package X`), which otherwise get passed
    // verbatim to cargo and ignored by the config. Honor them here, independent of
    // --no-cargo, and route to the supercluster coordinator.
    if config.distributed || extra.iter().any(|a| a == "--distributed" || a == "--dist") {
        let mut cfg = config.clone();
        cfg.distributed = true;
        if extra.iter().any(|a| a == "--release" || a == "-r") {
            cfg.release = true;
        }
        let mut it = extra.iter();
        while let Some(a) = it.next() {
            if a == "--package" || a == "-p" {
                if let Some(p) = it.next() {
                    cfg.package = Some(p.clone());
                }
            } else if let Some(p) = a.strip_prefix("--package=") {
                cfg.package = Some(p.to_string());
            }
        }
        return distributed_build(&cfg);
    }

    if config.no_cargo {
        if config.warm {
            println!("🔥 Warming dependency cache (cargo check --workspace)...");
            let mut cmd = std::process::Command::new("cargo");
            cmd.args(["check", "--workspace"]);
            apply_wrapper_env(&mut cmd);
            let _ = cmd.status();
            println!("✓ Dependencies warmed");
        }
        if config.async_build { return no_cargo_build_async(config); }
        if config.distributed { return distributed_build(config); }
        return no_cargo_build(config);
    }

    match effective_project_type(config) {
        ProjectType::FullStack => {
            println!("⚡ Flux full-stack build (Rust + Frontend){}",
                if config.release { " [release]" } else { "" });

            let config_arc = std::sync::Arc::new(config.clone());
            let extra_arc: std::sync::Arc<[String]> = extra.iter().cloned().collect::<Vec<_>>().into();

            let c1 = config_arc.clone();
            let e1 = extra_arc.clone();
            let c2 = config_arc;
            let e2 = extra_arc;

            let rust_handle = std::thread::spawn(move || {
                println!("  📦 Backend (Rust)...");
                run_cargo("build", &*c1, &*e1);
            });
            let fe_handle = std::thread::spawn(move || {
                println!("  🎨 Frontend (Vite)...");
                run_frontend_build(&*c2, &*e2);
            });

            rust_handle.join().ok();
            fe_handle.join().ok();
            println!("  ✓ Full-stack build complete");
        }
        ProjectType::Rust => run_cargo("build", config, extra),
        ProjectType::LaTeX => run_latex_build(config, extra),
        ProjectType::Frontend => run_frontend_build(config, extra),
        ProjectType::Unknown => {
            eprintln!("No Cargo.toml or package.json found. Nothing to build.");
            eprintln!("Hint: use --rust-only or --frontend-only to force a project type.");
            process::exit(1);
        }
    }

    let elapsed_ms = start.elapsed().as_millis() as u64;
    TOTAL_BUILD_TIME_MS.fetch_add(elapsed_ms, Ordering::Relaxed);
    println!("  ⏱ Build time: {}ms", elapsed_ms);

    // Real cache outcome for THIS build = delta of the wrapper's per-unit counters.
    // (Before: this read CACHE_HITS, which only the frontend path ever bumped, so every
    // Rust build persisted 0/0 → the permanently-dead "0/N" all-time cache rate.)
    let (ch1, cm1) = cache_event_counts();
    let hits = ch1.saturating_sub(ch0);
    let misses = cm1.saturating_sub(cm0);
    CACHE_HITS.fetch_add(hits, Ordering::Relaxed);
    CACHE_MISSES.fetch_add(misses, Ordering::Relaxed);
    persist_build(hits, misses, elapsed_ms);
}

pub fn detect_and_run(config: &BuildConfig, extra: &[String]) {
    match effective_project_type(config) {
        ProjectType::FullStack => {
            println!("⚡ Flux full-stack run");
            println!("  🎨 Frontend dev server...");
            spawn_frontend_dev();
            std::thread::sleep(std::time::Duration::from_secs(1));
            println!("  📦 Backend...");
            run_cargo("run", config, extra);
        }
        ProjectType::Frontend => run_frontend_dev(),
        _ => run_cargo("run", config, extra),
    }
}

pub fn detect_and_test(config: &BuildConfig, extra: &[String]) {
    match effective_project_type(config) {
        ProjectType::FullStack | ProjectType::Rust => run_cargo("test", config, extra),
        ProjectType::Frontend => {
            println!("  🧪 Frontend tests...");
            run_npm(&["test"]);
        }
        _ => {}
    }
}

// ── Rust Build ──

/// The canonical cargo environment. EVERY cargo spawn in fluxc must apply this.
///
/// cargo hashes RUSTC_WRAPPER into every unit's fingerprint, so entry points
/// that disagree about the wrapper (build/check used to spawn WITHOUT it while
/// test/self/MCP spawn WITH it) flip the entire workspace fingerprint on every
/// alternation -- a full multi-minute rebuild each way, plus ~27 build-script
/// re-runs (autocfg crates declare rerun-if-env-changed=RUSTC_WRAPPER). That
/// was the "v0.20 built warm in seconds, now 2+ minutes" regression
/// (root-caused 2026-07-02). One wrapper identity = one fingerprint universe
/// = warm builds stay warm across build/check/test/combo/self.
/// The wrapper path cargo hashes into every unit fingerprint. Default = this
/// binary (env::current_exe). But cargo keys the fingerprint on the wrapper
/// PATH, so two agents running fluxc from different checkouts
/// (/checkout-a/target/debug/fluxc vs /checkout-b/...) get DIFFERENT fingerprints
/// → the cross-agent rebuild storm returns even though each is internally warm.
/// FLUX_WRAPPER_PATH pins a single canonical path (e.g. ~/.flux/bin/fluxc, kept
/// symlinked to the live binary) so a whole fleet shares ONE fingerprint universe.
/// Opt-in: unset = today's per-binary behavior (safe); the operator who sets it
/// owns keeping that path pointed at a current fluxc.
pub fn canonical_wrapper_path() -> std::path::PathBuf {
    if let Ok(p) = env::var("FLUX_WRAPPER_PATH") {
        if !p.is_empty() { return std::path::PathBuf::from(p); }
    }
    env::current_exe().expect("current fluxc executable")
}

/// Quarantine a poisoned cargo target-info probe cache before spawning cargo.
/// A crashlooping host process (a jq / ssh-keygen status feed observed on this
/// box) can spray text onto the stdin cargo hands its `rustc - --print` probe;
/// cargo then caches `"success":false` in `<target>/.rustc_info.json` and REPLAYS
/// that failure on every later build WITHOUT re-probing. The `main.rs` fd0-null
/// guard protects only a LIVE probe, so it cannot clear a cached failure — this
/// does. Rename any poisoned cache aside so cargo re-probes clean (the null-stdin
/// guard then keeps the regeneration clean). Cheap (stat + small read); a no-op
/// when the cache is healthy or absent.
pub fn quarantine_poisoned_probe_cache() {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(td) = std::env::var("CARGO_TARGET_DIR") {
        if !td.is_empty() { candidates.push(std::path::Path::new(&td).join(".rustc_info.json")); }
    }
    candidates.push(std::path::PathBuf::from("target/.rustc_info.json"));
    for c in candidates {
        if let Ok(txt) = std::fs::read_to_string(&c) {
            if txt.contains("\"success\":false") {
                let aside = c.with_extension(format!("json.poisoned-{}", std::process::id()));
                if std::fs::rename(&c, &aside).is_ok() {
                    eprintln!("  \u{26a0} quarantined poisoned probe cache {} (host feed contamination) \u{2014} cargo will re-probe clean",
                        c.display());
                }
            }
        }
    }
}

pub fn apply_wrapper_env(cmd: &mut Command) {
    cmd.env("RUSTC_WRAPPER", canonical_wrapper_path());
    cmd.env("FLUXC_WRAPPING", "1");
    cmd.env("REAL_RUSTC", env::var("REAL_RUSTC").unwrap_or_else(|_| "rustc".to_string()));
}

pub fn run_cargo(cargo_cmd: &str, config: &BuildConfig, extra_args: &[String]) {
    quarantine_poisoned_probe_cache();
    let mut cmd = Command::new("cargo");
    cmd.arg(cargo_cmd);

    if config.release {
        cmd.arg("--release");
    }

    if let Some(ref pkg) = config.package {
        cmd.args(["--package", pkg]);
    }

    for a in extra_args { cmd.arg(a); }

    // Always-on wrapper (NOT gated on --provenance): a wrapper-less spawn here
    // would flip every unit's fingerprint vs the test/self/MCP builds. See
    // apply_wrapper_env. The --provenance flag remains accepted for its other
    // (signing) semantics; it no longer controls the wrapper.
    apply_wrapper_env(&mut cmd);

    // FIP-0003 phase 1: every build-family invocation leaves a TDG breadcrumb
    // (node + run stamp). Advisory — tdg swallows its own failures.
    let tdg_start = std::time::Instant::now();
    let s = cmd.status().expect("cargo");
    tdg::record_invocation(cargo_cmd, config.package.as_deref(), config.release,
        s.success(), tdg_start.elapsed().as_millis() as u64);
    if !s.success() { process::exit(s.code().unwrap_or(1)); }
}

// ── LaTeX Build ──

pub fn run_latex_build(_config: &BuildConfig, extra: &[String]) {
    let main_file = extra.first().cloned().unwrap_or_else(|| "main.tex".to_string());
    println!("📜 Flux LaTeX build: {}", main_file);

    let start = std::time::Instant::now();
    let mut cmd = std::process::Command::new("pdflatex");
    cmd.arg("-interaction=nonstopmode").arg("-halt-on-error").arg(&main_file);

    match cmd.status() {
        Ok(s) if s.success() => {
            let ms = start.elapsed().as_millis();
            println!("  ✓ PDF generated in {}ms", ms);
            // Second pass for TOC/cross-refs
            let _ = std::process::Command::new("pdflatex")
                .arg("-interaction=nonstopmode").arg(&main_file)
                .stdout(std::process::Stdio::null()).status();
        }
        Ok(s) => {
            eprintln!("  ✗ LaTeX build failed (exit {})", s.code().unwrap_or(1));
            eprintln!("  Check {} for errors", main_file.replace(".tex", ".log"));
        }
        Err(e) => eprintln!("  ✗ pdflatex not found: {}", e),
    }
}

// ── Frontend Build ──

pub fn run_frontend_build(config: &BuildConfig, _extra: &[String]) {
    let hash = frontend_source_hash();
    let cache_dir = PathBuf::from("target").join("flux-cache").join("frontend");
    let _ = std::fs::create_dir_all(&cache_dir);
    let cache_file = cache_dir.join(format!("{}.ok", hash));

    if cache_file.exists() {
        CACHE_HITS.fetch_add(1, Ordering::Relaxed);
        println!("  ⚡ fluxc frontend cache hit — skipping build");
        return;
    }
    CACHE_MISSES.fetch_add(1, Ordering::Relaxed);

    let pkg_manager = if PathBuf::from("pnpm-lock.yaml").exists() { "pnpm" }
        else if PathBuf::from("yarn.lock").exists() { "yarn" }
        else { "npm" };

    let lock_hash = lockfile_hash();
    let install_cache = cache_dir.join(format!("install_{}.ok", lock_hash));

    if !install_cache.exists() {
        println!("  📥 Installing dependencies ({})...", pkg_manager);
        run_npm(&["install"]);
        std::fs::write(&install_cache, "ok").ok();
        if let Ok(entries) = std::fs::read_dir(&cache_dir) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with("install_") && name != format!("install_{}.ok", lock_hash) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    } else {
        println!("  ⚡ Dependencies cached — skipping install");
    }

    let build_cmd = if config.release { "build" } else { "build" };
    println!("  🏗 Building frontend ({} mode)...", if config.release { "production" } else { "development" });
    run_npm(&["run", build_cmd]);

    std::fs::write(&cache_file, "ok").ok();
    println!("  ✓ Frontend build cached");
}

pub fn run_frontend_dev() {
    println!("  🎨 Starting Vite dev server (HMR enabled)...");
    run_npm(&["run", "dev"]);
}

pub fn spawn_frontend_dev() {
    let mut cmd = Command::new("npm");
    cmd.args(["run", "dev"]);
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    let _ = cmd.spawn();
}

pub fn dev_mode(config: &BuildConfig) {
    println!("⚡ Flux Dev Mode — Hot Reload Everywhere");
    match effective_project_type(config) {
        ProjectType::FullStack => {
            spawn_frontend_dev();
            std::thread::sleep(std::time::Duration::from_secs(2));
            println!("  📦 Backend: fluxc watch active");
        }
        ProjectType::Frontend => run_frontend_dev(),
        _ => { println!("  Run 'fluxc watch' for Rust projects"); }
    }
}

pub fn run_npm(args: &[&str]) {
    let manager = if PathBuf::from("pnpm-lock.yaml").exists() { "pnpm" }
        else if PathBuf::from("yarn.lock").exists() { "yarn" }
        else { "npm" };
    let s = Command::new(manager).args(args).status().expect(manager);
    if !s.success() { process::exit(s.code().unwrap_or(1)); }
}

fn frontend_source_hash() -> String {
    let mut hasher = blake3::Hasher::new();
    if let Ok(data) = std::fs::read("package.json") { hasher.update(&data); }
    if let Ok(entries) = std::fs::read_dir("src") {
        for e in entries.flatten() {
            hasher.update(e.file_name().to_string_lossy().as_bytes());
            if let Ok(data) = std::fs::read(e.path()) { hasher.update(&data); }
        }
    }
    format!("{}", hasher.finalize()).chars().take(32).collect()
}

fn lockfile_hash() -> String {
    for lock in &["package-lock.json", "pnpm-lock.yaml", "yarn.lock"] {
        if let Ok(data) = std::fs::read(lock) {
            return format!("{}", blake3::hash(&data)).chars().take(32).collect();
        }
    }
    "no_lock".into()
}

pub fn clean() {
    println!("Cleaning...");
    flux_cache::clean();
    let _ = std::fs::remove_dir_all("target/flux-cache/frontend");
    CACHE_HITS.store(0, Ordering::Relaxed);
    CACHE_MISSES.store(0, Ordering::Relaxed);
    BUILD_COUNT.store(0, Ordering::Relaxed);
    TOTAL_BUILD_TIME_MS.store(0, Ordering::Relaxed);
    let _ = std::fs::remove_file(stats_path());
    println!("✓ Cleaned");
}

// ── Stats ──

pub fn print_stats() {
    let hits = CACHE_HITS.load(Ordering::Relaxed);
    let misses = CACHE_MISSES.load(Ordering::Relaxed);
    let total = hits + misses;
    let rate = if total > 0 { (hits as f64 / total as f64) * 100.0 } else { 0.0 };
    let builds = BUILD_COUNT.load(Ordering::Relaxed);
    let total_time = TOTAL_BUILD_TIME_MS.load(Ordering::Relaxed);
    let avg_time = if builds > 0 { total_time / builds } else { 0 };

    let ps = load_persistent_stats();
    let p_total = ps.total_cache_hits + ps.total_cache_misses;
    let p_rate = if p_total > 0 { (ps.total_cache_hits as f64 / p_total as f64) * 100.0 } else { 0.0 };
    let p_avg = if ps.total_builds > 0 { ps.total_build_time_ms / ps.total_builds } else { 0 };

    println!("⚡ Flux Build Statistics");
    println!("  Project type:    {:?}", detect_project());
    println!("  This session:    {} builds, {}/{} cache ({:.1}%), avg {}ms",
        builds, hits, total, rate, avg_time);
    println!("  All-time (disk): {} builds, {}/{} cache ({:.1}%), avg {}ms",
        ps.total_builds, ps.total_cache_hits, p_total, p_rate, p_avg);
    println!("  Sessions:        {}", ps.sessions);
    println!("  Total build time: {}ms (session) / {}ms (all-time)", total_time, ps.total_build_time_ms);

    let (cu_hits, cu_misses) = cache_event_counts();
    let cu_total = cu_hits + cu_misses;
    let cu_rate = if cu_total > 0 { (cu_hits as f64 / cu_total as f64) * 100.0 } else { 0.0 };
    println!("  Compile cache:   {}/{} units ({:.1}%)  [wrapper flux_cache, all-time]", cu_hits, cu_total, cu_rate);

    let cache_dir = PathBuf::from("target").join("flux-cache");
    if cache_dir.exists() {
        let size = dir_size(&cache_dir);
        println!("  Cache size:      {} bytes ({:.1} KB)", size, size as f64 / 1024.0);
    }

    let fe_cache = cache_dir.join("frontend");
    if fe_cache.exists() {
        let fe_count = std::fs::read_dir(&fe_cache).map(|d| d.count()).unwrap_or(0);
        println!("  FE cache entries: {}", fe_count);
    }
}

fn dir_size(path: &PathBuf) -> u64 {
    fn walk(p: &PathBuf, total: &mut u64) {
        if let Ok(entries) = std::fs::read_dir(p) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() { walk(&path, total); }
                else if let Ok(meta) = path.metadata() { *total += meta.len(); }
            }
        }
    }
    let mut total = 0u64;
    walk(path, &mut total);
    total
}

// ── Supercluster Mode ──

pub fn supercluster_mode(extra: &[String]) {
    let sub = extra.first().map(|s| s.as_str());

    match sub {
        Some("join") => {
            let addr = extra.get(1).map(|s| s.as_str()).unwrap_or("localhost:9933");
            println!("⚡ Flux Supercluster — joining {}", addr);
            println!("  Peer ID:     flux-{}", whoami());
            println!("  Status:      connected (simulated)");
            println!("  Peers:       1");
            println!("  Shared cache: target/flux-cache/supercl");
        }
        Some("list") => {
            println!("⚡ Supercluster Peers:");
            println!("  flux-{} (self) — builder", whoami());
        }
        Some("status") => {
            println!("⚡ Supercluster Status:");
            println!("  Mode:         standalone");
            println!("  Peers:        1 (self)");
            println!("  Shared cache: 0 entries");
            println!("  Phase:        1 (local only)");
        }
        _ => {
            println!("fluxc supercluster — Distributed compilation mesh");
            println!("  fluxc supercluster join <addr>   Join a supercluster");
            println!("  fluxc supercluster list          List peers");
            println!("  fluxc supercluster status        Show status");
        }
    }
}

fn whoami() -> String {
    env::var("USER").unwrap_or_else(|_| "unknown".into())
}

// ── Watch Mode ──

pub fn watch_mode(config: &BuildConfig, extra: &[String]) {
    use notify::Watcher;
    let (tx, rx) = std::sync::mpsc::channel();

    let mut watcher = notify::recommended_watcher(move |res| {
        if let Ok(event) = res { let _ = tx.send(event); }
    }).expect("watch");

    watcher.watch(std::path::Path::new("."), notify::RecursiveMode::Recursive).ok();

    println!("⚡ fluxc watch — auto-rebuild on changes (Ctrl+C to stop)");
    println!("  Watching: .");

    for event in rx {
        let paths: Vec<String> = event.paths.iter()
            .filter_map(|p| p.to_str().map(String::from))
            .collect();
        let any_rs = paths.iter().any(|p| p.ends_with(".rs"));
        let any_fe = paths.iter().any(|p| {
            p.ends_with(".tsx") || p.ends_with(".ts") || p.ends_with(".jsx") ||
            p.ends_with(".js") || p.ends_with(".css") || p.ends_with(".html")
        });

        if any_rs || any_fe {
            println!("\n🔄 Changes detected: {}", paths.join(", "));
            detect_and_build(config, extra);
        }
    }
}

// ── Wrapper Mode (RUSTC_WRAPPER) ──

pub fn wrapper_mode(args: &[String]) {
    // Cargo passes: args[0] = wrapper path, args[1] = real rustc, args[2..] = rustc flags
    // Only enabled when FLUXC_WRAPPING=1 (set by self_build).
    //
    // v0.17 (rocky-sigil-75): wires flux-driver + flux-cache into the wrapper.
    // BEFORE this patch the wrapper was a pure rustc passthrough and the
    // "RUSTC_WRAPPER=self active (flux-driver caching)" log line was marketing
    // — flux_cache::lookup/store were never called. This patch:
    //
    //   1. Computes a content hash over (source file, rustc args) → cache key
    //   2. On entry: try flux_cache::lookup → if found and outputs apply
    //      cleanly, skip rustc entirely
    //   3. On exit (after rustc success): collect outputs via flux-driver,
    //      store under the same key
    //
    // The CACHE-HIT path is bounded by what flux-driver::apply_cached_outputs
    // can restore — currently dep-info markers only (full binary content
    // restore is a flux-cache follow-up, see FLUXC_CACHE_GAP.md). The
    // CACHE-STORE path is fully functional and starts populating immediately.
    //
    // v0.17 (rocky-sigil-82): cross-workspace cache key normalization.
    //
    // For cross-workspace use (sigil/, quillonos/, …) the same dep built
    // in two workspaces lives at two different absolute paths
    // (`/flux/target/.../libserde-X.rlib` vs `/sigil/target/.../libserde-Y.rlib`).
    // flux_cache::compute_hash hashes the raw arg strings — so the cache
    // key embedded those paths, and two workspaces producing byte-identical
    // rlibs got DIFFERENT keys, defeating cross-workspace reuse.
    //
    // The fix: substitute each `--extern <name>=<path>` and `-L <kind>=<path>`
    // with the BLAKE3 of the referenced file's content before computing the
    // cache key. Two workspaces with byte-identical deps now produce
    // identical keys, so a sigil rebuild hits the flux-populated cache
    // (and vice versa). Same for `--out-dir`, which doesn't affect what
    // gets compiled, only where outputs land — dropped from the key entirely.
    //
    // This is the patch-4 workaround documented in FLUXC_CACHE_GAP.md —
    // implemented here in wrapper_mode because flux-cache is held by
    // deepseek-0; once their compute_hash change lands, this can shrink
    // to just the call site.

    let rustc_path = if args.len() > 1 { &args[1] } else { "rustc" };
    let rustc_args: Vec<String> = if args.len() > 2 { args[2..].to_vec() } else { Vec::new() };

    // PROBE PASS-THROUGH (fix: combo failed in fresh CARGO_TARGET_DIR).
    // Cargo invokes the wrapper for NON-compilation probes to learn the toolchain:
    //   `rustc - --print=sysroot --print=cfg ...`  (target-info / sysroot / cfg)
    //   `rustc -vV` / `--version`
    // These write stdout that cargo parses. They must NEVER hit the content cache:
    // caching a probe means a later cache HIT returns via apply_cached_outputs WITHOUT
    // emitting the --print output, and cargo dies with "failed to run `rustc` to learn
    // about target-specific information" (exactly the `fluxc combo` failure on a cold/
    // isolated target). A real compile always has a positional `.rs` source; a probe
    // never does. So: no `.rs` source, or any `--print`/version flag ⇒ exec real rustc
    // straight through, no lookup, no store.
    let has_rs_src = rustc_args
        .iter()
        .any(|a| a.ends_with(".rs") && !a.starts_with("--"));
    let is_probe = !has_rs_src
        || rustc_args
            .iter()
            .any(|a| a == "--print" || a.starts_with("--print=") || a == "-vV" || a == "-V" || a == "--version");
    if is_probe {
        let mut cmd = Command::new(rustc_path);
        for a in &rustc_args {
            cmd.arg(a);
        }
        // ROOT-CAUSE FIX (build reliability): info probes (`--print=…`, `-vV`,
        // `--version`) read NO real source — the `-` positional is a placeholder
        // and cargo only parses our stdout. But cargo connects this probe's stdin
        // to a pipe, and on this box a concurrent status-feed `jq` races onto that
        // fd and injects its filter, which rustc then tries to parse as source
        // ("error: this file contains an unclosed delimiter" / "jq: 1 compile
        // error") → the probe dies → cargo aborts the whole build with "failed to
        // run `rustc` to learn about target-specific information". Detaching the
        // probe's stdin makes the toolchain query immune to that fd pollution.
        let is_print_probe = rustc_args.iter().any(|a| {
            a == "--print" || a.starts_with("--print=") || a == "-vV" || a == "-V" || a == "--version"
        });
        if is_print_probe {
            cmd.stdin(std::process::Stdio::null());
        }
        match cmd.status() {
            Ok(s) => process::exit(s.code().unwrap_or(0)),
            Err(e) => {
                eprintln!("fluxc wrapper: probe passthrough to '{}' failed: {}", rustc_path, e);
                process::exit(1);
            }
        }
    }

    // Locate the source file rustc was invoked on (positional .rs argument).
    let src_file = rustc_args.iter()
        .find(|a| a.ends_with(".rs") && !a.starts_with("--"))
        .map(|s| s.as_str());

    // FIP-0003 write-path #1: unit facts for the TDG spool. NO flux-db access
    // in the wrapper (parallel short-lived processes) — tdg::spool_wrapper_unit
    // writes one tiny file; the parent invocation batch-ingests after cargo.
    let tdg_t0 = std::time::Instant::now();
    let tdg_crate = rustc_args.iter().position(|a| a == "--crate-name")
        .and_then(|i| rustc_args.get(i + 1)).cloned().unwrap_or_else(|| "?".into());
    let tdg_deps = tdg::dep_idents_from_rustc_args(&rustc_args);
    // Package attribution + test-harness marker for unit-granularity
    // scheduling. CARGO_PKG_NAME is set by cargo on every rustc spawn and is
    // the only reliable pkg mapping for integration-test units (their
    // --crate-name is the test FILE name, not the package).
    let tdg_pkg = std::env::var("CARGO_PKG_NAME").unwrap_or_else(|_| tdg_crate.clone());
    let tdg_is_test = rustc_args.iter().any(|a| a == "--test");

    // Compute the cache key over source content + normalized rustc args.
    // Normalized args drop absolute paths in favor of content hashes — same
    // content + same flags = same key, regardless of which workspace the
    // build runs from.
    let normalized_args = normalize_args_for_cache_key(&rustc_args);
    let cache_key = flux_cache::compute_hash(src_file, &normalized_args);

    // Try a cache hit. apply_cached_outputs returns `true` only when every
    // cached output blob was restored to the rustc-expected out_dir; on
    // false we fall through and run rustc for real.
    if !cache_key.is_empty() {
        let trace = std::env::var("FLUX_CACHE_TRACE").is_ok();
        let kp = &cache_key[..16.min(cache_key.len())];
        let cn = rustc_args.iter().position(|a| a == "--crate-name")
            .and_then(|i| rustc_args.get(i + 1)).map(|s| s.as_str()).unwrap_or("?");
        // BUILD-SAFE restore gate (the honest Phase-1 done state): a cross-build cache restore is only
        // consistent when the FULL dep closure hits — a PARTIAL hit mixes a stale restored .rmeta with
        // a freshly-recompiled dependent, and rustc then ICEs (SIGBUS / "no resolution for an import").
        // Non-deterministic rlib keys guarantee partial hits today, so RESTORE is opt-in
        // (FLUX_CACHE_RESTORE=1) until Phase 2 makes keys deterministic enough to hit the whole closure.
        // The cache still POPULATES unconditionally below, so it's primed the moment restore is safe.
        // Cache restore is ON by default, guarded. The invariant still binds: the
        // cache can NEVER break a build — an unverifiable entry is a MISS, never a
        // restore. Guards in apply_cached_outputs: required-ext check, REQUIRED
        // closure-consistency sidecar for dep-carrying units (no sidecar = miss),
        // and the exact-name post-restore check (a name mismatch is a miss — never
        // bridge/rename blobs, the embedded crate identity would be wrong).
        // FLUX_CACHE_RESTORE=0 opts out for debugging.
        if std::env::var("FLUX_CACHE_RESTORE").as_deref() != Ok("0") {
            if let Some(entry) = flux_cache::lookup(&cache_key) {
                if flux_driver::apply_cached_outputs(&entry, &rustc_args) {
                    if trace { eprintln!("FLUXCACHE HIT {} key={}", cn, kp); }
                    record_cache_event(true);
                    tdg::spool_wrapper_unit(&cache_key, &tdg_crate, &tdg_pkg, tdg_is_test,
                        &tdg_deps, "skipped", tdg_t0.elapsed().as_millis() as u64);
                    return;
                }
                if trace { eprintln!("FLUXCACHE APPLYFAIL {} key={} blobs={:?}", cn, kp, flux_cache::cache_dir().join("blobs").join(&entry.source_hash).exists()); }
            } else if trace {
                eprintln!("FLUXCACHE LOOKUPMISS {} key={}", cn, kp);
            }
        }
        // Cacheable unit (real .rs source, non-empty key) with no usable cached output:
        // a genuine compilation-unit cache MISS. (Uncacheable probes returned earlier.)
        record_cache_event(false);
    }

    // Cache miss — run real rustc.
    let mut cmd = Command::new(rustc_path);
    for a in &rustc_args { cmd.arg(a); }
    match cmd.status() {
        Ok(s) if s.success() => {
            // Successful build — capture outputs + store under the key for
            // future runs. The collect_outputs result includes paths to the
            // rmeta/rlib/dep-info files rustc just produced; store with the
            // content hash as key.
            if !cache_key.is_empty() {
                let entry = flux_driver::collect_outputs(&rustc_args, &cache_key);
                // Only store entries that actually carry artifacts. An empty-outputs entry (e.g. a
                // build_script_build binary — no rmeta/rlib) can never satisfy a hit, so indexing it
                // just produces blobless entries that later lookup-hit then APPLYFAIL (the measured
                // blobs=false=121). Skip them: the unit recompiles cheaply and the index stays clean.
                if !entry.source_hash.is_empty() && !entry.outputs.is_empty() {
                    flux_cache::store(&cache_key, &entry);
                }
            }
            tdg::spool_wrapper_unit(&cache_key, &tdg_crate, &tdg_pkg, tdg_is_test,
                &tdg_deps, "green", tdg_t0.elapsed().as_millis() as u64);
        }
        Ok(s) => {
            tdg::spool_wrapper_unit(&cache_key, &tdg_crate, &tdg_pkg, tdg_is_test,
                &tdg_deps, "red", tdg_t0.elapsed().as_millis() as u64);
            process::exit(s.code().unwrap_or(1));
        }
        Err(e) => {
            eprintln!("fluxc wrapper: failed to spawn rustc '{}': {}", rustc_path, e);
            eprintln!("hint: FLUXC_WRAPPING is for cargo RUSTC_WRAPPER=self, not for general use");
            process::exit(101);
        }
    }
}

/// Normalize rustc args so the cache key is workspace-independent.
///
/// Substitutes absolute paths inside `--extern name=PATH` and `-L kind=PATH`
/// with the BLAKE3 content hash of the referenced file (`<extern-content:HEX>`)
/// or, for directories, the hash of their sorted file-name listing. Drops
/// `--out-dir PATH` entirely — output destination doesn't affect what rustc
/// compiles, only where the result lands.
///
/// The function is pure and side-effect-free except for filesystem reads on
/// `--extern` referenced files. Missing or unreadable paths are kept verbatim
/// (the cache key still works locally; cross-workspace reuse just degrades to
/// per-workspace for that one entry).
fn normalize_args_for_cache_key(args: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--extern" if i + 1 < args.len() => {
                out.push(a.clone());
                out.push(substitute_name_eq_path(&args[i + 1], substitute_dep_identity));
                i += 2;
            }
            "-L" if i + 1 < args.len() => {
                // 0.33 FIX (the actual 0% cause): DROP -L from the cache key. -L is a
                // volatile SEARCH path (target/debug/deps, whose directory listing
                // changes on every build as crates accumulate / get cleaned), so
                // hashing its listing made EVERY crate's cache_key unstable build-to-
                // build -> 100% lookup-miss (traced: HIT=0 LOOKUPMISS=37 APPLYFAIL=0).
                // The actual dependencies are already pinned content-addressably by
                // --extern, so -L doesn't affect WHAT compiles, only WHERE rustc looks
                // — exactly like --out-dir, which is dropped for the same reason.
                i += 2;
            }
            "--out-dir" if i + 1 < args.len() => {
                // Drop entirely — output destination doesn't affect compiled output.
                i += 2;
            }
            "--emit" if i + 1 < args.len() => {
                // Cache T1: make the key EMIT-AWARE. Cargo's pipelined build runs the SAME crate
                // twice — `--emit=metadata` (produces .rmeta) then `--emit=link,...` (.rlib). Without
                // this, both passes hash to the same key, so a metadata-only entry can be served for a
                // link lookup (or vice-versa) -> wrong/missing artifact -> rc=101. Normalize to the
                // sorted set of emit KINDS (dropping the volatile =output-path), so the two passes
                // get distinct keys.
                out.push("--emit".into());
                out.push(normalize_emit(&args[i + 1]));
                i += 2;
            }
            s if s.starts_with("--emit=") => {
                out.push(format!("--emit={}", normalize_emit(&s["--emit=".len()..])));
                i += 1;
            }
            _ => {
                out.push(a.clone());
                i += 1;
            }
        }
    }
    out
}

/// Cache T1 helper: canonicalize an `--emit` spec to its sorted, de-duplicated set of KINDS,
/// dropping any `=output-path` (which is volatile). e.g. `link,dep-info=/abs/foo.d` -> `dep-info,link`.
/// Keeps the cache key path-independent while distinguishing a metadata pass from a link pass.
fn normalize_emit(spec: &str) -> String {
    let mut kinds: Vec<&str> = spec.split(',').map(|p| p.split('=').next().unwrap_or(p)).collect();
    kinds.sort_unstable();
    kinds.dedup();
    kinds.join(",")
}

/// Parse a "name=value" or "kind=value" arg and substitute the value via
/// `subst`. If there's no `=`, pass through unchanged.
fn substitute_name_eq_path(arg: &str, subst: impl Fn(&str) -> String) -> String {
    match arg.find('=') {
        Some(eq) => format!("{}={}", &arg[..eq], subst(&arg[eq + 1..])),
        None => arg.to_string(),
    }
}

/// Substitute a dep path with its STABLE identity: the filename `lib<crate>-<metadata-hash>.<ext>`.
/// Phase 2 (determinism — the fix for 0.4% cold hits + the partial-hit ICE): the rlib BYTES are
/// NON-deterministic (rustc embeds a volatile metadata blob), so hashing them (the old behavior) made
/// every dependent's --extern key vary build-to-build -> guaranteed misses -> an inconsistent mix of
/// restored+recompiled rmetas -> SIGBUS. The filename's metadata-hash is cargo's DETERMINISTIC
/// fingerprint of (crate, features, profile, transitive deps): it changes iff a real input changed,
/// and is byte-identical across cold builds in the same workspace. (Cross-machine CI<->dev determinism
/// is a later refinement — cargo's metahash can embed an abs path for path-deps; the robust form is a
/// sidecar mapping each rlib to the dep's own source-based flux cache_key. See [[flux-cache-reality]].)
fn substitute_dep_identity(path: &str) -> String {
    match std::path::Path::new(path).file_name() {
        Some(f) => format!("<extern:{}>", f.to_string_lossy()),
        None => path.to_string(),
    }
}

/// Substitute a directory path with a hash of its sorted file-name listing.
/// We don't hash file CONTENTS here — that would be expensive for large dep
/// directories and the explicit `--extern` flags already pin the deps that
/// matter. The listing-hash captures "which deps are visible" at a
/// workspace-independent level.
fn substitute_dir_listing(path: &str) -> String {
    let Ok(entries) = std::fs::read_dir(path) else {
        return path.to_string();
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    let mut h = blake3::Hasher::new();
    for n in &names {
        h.update(n.as_bytes());
        h.update(b"\0");
    }
    format!("<dir:{}>", h.finalize().to_hex())
}

// ── No-Cargo Build (Phase 2 — direct rustc) ──

pub fn no_cargo_build(config: &BuildConfig) {
    let start = std::time::Instant::now();
    println!("⚡ Flux Phase 2 — flux-graph dependency resolution + cargo per-batch");

    let root = std::env::current_dir().unwrap_or_default();
    let ws = match flux_graph::resolve_workspace(&root) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("flux-graph error: {}", e);
            eprintln!("Falling back to cargo build...");
            run_cargo("build", config, &[]);
            return;
        }
    };

    println!("  Workspace: {} crates in {} root", ws.crates.len(), ws.root.display());
    println!("  Build order: {} batch(es)", ws.batches.len());
    for (bi, batch) in ws.batches.iter().enumerate() {
        let names: Vec<&str> = batch.iter().map(|&i| ws.crates[i].name.as_str()).collect();
        println!("    Batch {}: {} crate(s) — {}", bi + 1, batch.len(), names.join(", "));
    }

    // Phase 2a: Use cargo -p per crate within flux-graph's batch plan.
    // This proves the graph resolution while using cargo for correct rustc flags.
    let total_crates = ws.crates.len();
    let mut built = 0u64;
    let mut failed = false;
    let build_start = std::time::Instant::now();

    for batch in &ws.batches {
        for &idx in batch {
            let ci = &ws.crates[idx];
            // Progress bar
            let done = built as f64 / total_crates.max(1) as f64;
            let bar_fill = (done * 20.0) as usize;
            let bar = format!("[{}{}]", "=".repeat(bar_fill), " ".repeat(20 - bar_fill));
            eprint!("\r  {} {}/{} {}", bar, built, total_crates, ci.name);
            let mut cmd = std::process::Command::new("cargo");
            cmd.arg(if config.release { "build" } else { "check" });
            if config.release { cmd.arg("--release"); }
            cmd.args(["--package", &ci.name]);
            cmd.env("RUSTC_WRAPPER", canonical_wrapper_path());
            cmd.env("FLUXC_WRAPPING", "1");
            cmd.env("FLUX_CACHE_RESTORE", "1");

            let status = match cmd.status() {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("  ✗ cargo {} failed: {}", ci.name, e);
                    failed = true;
                    break;
                }
            };

            if status.success() {
                built += 1;
            } else {
                eprintln!("  ✗ cargo {} failed with exit {:?}", ci.name, status.code());
                failed = true;
                break;
            }
        }
        if failed { break; }
    }

    let elapsed_ms = start.elapsed().as_millis() as u64;
    TOTAL_BUILD_TIME_MS.fetch_add(elapsed_ms, Ordering::Relaxed);

    let agility = flux_graph::agility::audit_agility(&ws);
    if !failed {
        eprint!("
");
        println!("✓ Phase 2 build complete in {}ms", elapsed_ms);
        println!("  {} crates built via flux-graph batch plan", built);
        let ag = flux_graph::agility::audit_agility(&ws);
        println!("  Agility: {:.0}% | PQ: {} | Classical: {} | Migrate: {}",ag.agility_score*100.0,ag.pq_crates,ag.classical_crates,ag.migration_needed.len());
        println!("  RUSTC_WRAPPER=self active (flux-driver caching)");
    }
}

pub fn no_cargo_build_async(config: &BuildConfig) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async { no_cargo_build_async_inner(config).await });
}

async fn no_cargo_build_async_inner(config: &BuildConfig) {
    let start = std::time::Instant::now();
    println!("⚡ Flux Phase 2b — async build (tokio parallel)");
    let root = std::env::current_dir().unwrap_or_default();
    let ws = match flux_graph::resolve_workspace(&root) { Ok(w) => w, Err(_) => { return; } };
    println!("  {} crates, {} batches", ws.crates.len(), ws.batches.len());
    let total = ws.crates.len() as f64;
    let mut built = 0u64;
    for batch in &ws.batches {
        let mut tasks = Vec::new();
        for &idx in batch {
            let name = ws.crates[idx].name.clone();
            let rel = config.release;
            tasks.push(tokio::task::spawn(async move {
                let mut cmd = tokio::process::Command::new("cargo");
                cmd.arg(if rel { "build" } else { "check" });
                if rel { cmd.arg("--release"); }
                cmd.args(["--package", &name]);
                // Same canonical wrapper env as apply_wrapper_env (tokio Command).
                cmd.env("RUSTC_WRAPPER", canonical_wrapper_path());
                cmd.env("FLUXC_WRAPPING", "1");
                cmd.env("REAL_RUSTC", std::env::var("REAL_RUSTC").unwrap_or_else(|_| "rustc".to_string()));
                (name, cmd.status().await.map(|s| s.success()).unwrap_or(false))
            }));
        }
        for t in tasks {
            if let Ok((name, ok)) = t.await {
                if ok { built += 1; } else { eprintln!("  ✗ {} failed", name); }
            }
        }
        eprint!("  [{}{}] {}/{} crates", "=".repeat((built as f64/total*20.0) as usize), " ".repeat(20usize.saturating_sub((built as f64/total*20.0) as usize)), built, ws.crates.len());
    }
    eprint!("
");
    println!("✓ Async build complete in {}ms", start.elapsed().as_millis());
}

// ── Self-Build (dogfooding) ──

pub fn self_build(config: BuildConfig) {
    let start = std::time::Instant::now();

    if config.no_cargo {
        if config.warm {
            println!("🔥 Warming dependency cache (cargo check --workspace)...");
            let mut cmd = std::process::Command::new("cargo");
            cmd.args(["check", "--workspace"]);
            apply_wrapper_env(&mut cmd);
            let _ = cmd.status();
            println!("✓ Dependencies warmed");
        }
        println!("⚡ Flux Self-Build — dogfooding (Phase 2: no cargo)");
        no_cargo_build(&config);
        return;
    }

    println!("⚡ Flux Self-Build — dogfooding");
    println!("  Phase 1: cargo build with RUSTC_WRAPPER=self");

    // Snapshot file-based cache counters before build — the wrapper runs as
    // subprocesses and writes to ~/.flux/.cache-unit-{hits,misses}, not the
    // in-memory atomics (which are in the parent process).
    let (ch0, cm0) = cache_event_counts();

    let mut cmd = Command::new("cargo");
    cmd.arg("build");
    if config.release { cmd.arg("--release"); }
    let fluxc = env::current_exe().expect("current fluxc executable");
    let real_rustc = env::var("REAL_RUSTC").unwrap_or_else(|_| "rustc".to_string());
    cmd.env("RUSTC_WRAPPER", fluxc);
    cmd.env("FLUXC_WRAPPING", "1");
    cmd.env("REAL_RUSTC", real_rustc);
    // Cargo's target-info probe compiles `-` from stdin. Never let unrelated
    // caller input become the probe source (a jq filter once poisoned
    // target/.rustc_info.json and made every subsequent self-build fail).
    cmd.stdin(Stdio::null());

    let status = cmd.status().expect("cargo");
    let elapsed_ms = start.elapsed().as_millis() as u64;
    TOTAL_BUILD_TIME_MS.fetch_add(elapsed_ms, Ordering::Relaxed);

    if !status.success() {
        eprintln!("✗ Self-build failed with exit code {}", status.code().unwrap_or(1));
        process::exit(status.code().unwrap_or(1));
    }

    // Read the file-based cache counters AFTER the build (wrapper subprocesses
    // appended to them). Compute the delta from the pre-build snapshot.
    let (ch1, cm1) = cache_event_counts();
    let hits = ch1.saturating_sub(ch0);
    let misses = cm1.saturating_sub(cm0);
    let total = hits + misses;
    let cache_rate = if total > 0 { (hits as f64 / total as f64) * 100.0 } else { 0.0 };

    let mut ps = load_persistent_stats();
    ps.total_builds += 1;
    ps.total_cache_hits += hits;
    ps.total_cache_misses += misses;
    ps.total_build_time_ms += elapsed_ms;
    ps.sessions += 1;
    save_persistent_stats(&ps);

    println!("✓ Self-build complete in {}ms", elapsed_ms);
    println!("  Cache: {}/{} ({:.1}%)", hits, total, cache_rate);
    println!("  Target: {}", if config.release { "release" } else { "debug" });
    println!("  Status: dogfooding (cargo + RUSTC_WRAPPER=self)");
    println!("  Binary: target/{}/fluxc", if config.release { "release" } else { "debug" });
    println!();
    let p_total = ps.total_cache_hits + ps.total_cache_misses;
    let p_rate = if p_total > 0 { (ps.total_cache_hits as f64 / p_total as f64) * 100.0 } else { 0.0 };
    println!("  All-time (disk): {} builds, {:.1}% cache rate, {}ms avg",
        ps.total_builds, p_rate,
        if ps.total_builds > 0 { ps.total_build_time_ms / ps.total_builds } else { 0 });
}

pub fn quick_build_run(package: &str, release: bool) {
    println!("⚡ Flux Quick — build + run {}", package);
    let mut cmd = std::process::Command::new("cargo");
    cmd.arg(if release { "build" } else { "check" });
    if release { cmd.arg("--release"); }
    cmd.args(["--package", package]);
    apply_wrapper_env(&mut cmd);
    let status = cmd.status().unwrap_or(std::process::ExitStatus::default());
    if status.success() {
        let mut run_cmd = std::process::Command::new("cargo");
        run_cmd.arg("run"); if release { run_cmd.arg("--release"); }
        run_cmd.args(["--bin", package]);
        apply_wrapper_env(&mut run_cmd);
        let _ = run_cmd.status();
    } else { eprintln!("Build failed — not running"); }
}

pub fn run_binary(package: &str, release: bool) {
    let mut cmd = std::process::Command::new("cargo");
    cmd.arg("run"); if release { cmd.arg("--release"); }
    cmd.args(["--bin", package]);
    apply_wrapper_env(&mut cmd);
    let _ = cmd.status();
}

pub fn run_tests(package: Option<&str>) {
    let mut cmd = std::process::Command::new("cargo");
    cmd.arg("test");
    if let Some(pkg) = package { cmd.args(["-p", pkg]); }
    apply_wrapper_env(&mut cmd);
    cmd.stdin(Stdio::null());
    let status = cmd.status().unwrap_or(std::process::ExitStatus::default());
    if !status.success() {
        eprintln!("Some tests failed");
        process::exit(status.code().unwrap_or(1));
    }
}

pub fn version() -> String {
    // Dynamic: reads workspace Cargo.toml. Falls back to CARGO_PKG_VERSION.
    version::VersionInfo::load(&std::env::current_dir().unwrap_or_default())
        .map(|vi| vi.banner())
        .unwrap_or_else(|_| {
            format!(
                "fluxc {} — Universal Build Orchestrator",
                option_env!("CARGO_PKG_VERSION").unwrap_or("0.1.0")
            )
        })
}

pub fn print_usage() {
    let v = version();
    println!("{} ⚡\n", v);
    println!("Core Commands:");
    println!("  fluxc build           Build everything (Rust + Frontend autodetect)");
    println!("  fluxc check           Cargo check (fast, no codegen)");
    println!("  fluxc run             Build + run a binary");
    println!("  fluxc test            Run tests (Rust or Frontend)");
    println!("  fluxc quick CRATE     Quick build + run (JIT)");
    println!("  fluxc self            Self-build: Flux builds Flux (dogfooding)");
    println!("  fluxc dev             Start dev servers (Vite HMR + fluxc watch)");
    println!("  fluxc watch           Watch all files, auto-rebuild");
    println!("  fluxc clean           Clear all caches");
    println!("  fluxc tdg [N]         Task-dependency graph report (FIP-0003 tracer)");
    println!("  fluxc tdg baseline    Record crate source keys (the reference point)");
    println!("  fluxc tdg plan        Incremental plan: dirty crates + dependency cone");
    println!("  fluxc tdg run         Test ONLY the cone (--check / --dry / --units)");
    println!("                        --units: skip crates whose test binaries are");
    println!("                        byte-identical to their last green run");
    println!("  fluxc stats           Show build statistics and cache info");
    println!("  fluxc status [--json] Show workspace status (crates, agility)");
    println!("  fluxc release-audit   Show release-lane split and hold status");
    println!("  fluxc plan            AI-optimized build plan (batches, est. time)");
    println!("  fluxc explain CRATE   Explain a crate (path, deps, type)");
    println!("  fluxc version         Print version");
    println!("  fluxc setup | pilot | first-run   Native onboarding (replaces curl|bash)");
    println!("  fluxc self-update     Fetch latest prebuilt into ~/.flux/bin\n");
    println!("Analysis & Optimization:");
    println!("  fluxc architect       Quantum Architecture Oracle — 6-dim blueprint");
    println!("  fluxc sigil-plan      SIGIL release plan — Cortex-optimized roadmap");
    println!("  fluxc cortex [PRESET] Autonomous Cortex Loop (architect→predict→optimize→learn)");
    println!("  fluxc cortex-summary  Cortex activity summary report");
    println!("  fluxc optimize        Ranked optimization suggestions (SIMD/io_uring/cache)");
    println!("  fluxc xray [--json]   Deep workspace X-ray: health, deps, risks");
    println!("  fluxc disasm PATH     Disassemble object file (nm + objdump)");
    println!("  fluxc compile FILE    Phase 3: compile a single .rs file");
    println!("  fluxc compile-native  Phase 3: compile with SQIsign provenance\n");
    println!("AI & Agents:");
    println!("  fluxc mcp             Start MCP stdio server (162 AI tools)");
    println!("  fluxc serve           Start HTTP + SSE dashboard server");
    println!("  fluxc chat            Interactive AI chat mode");
    println!("  fluxc ai              AI-assisted build decisions");
    println!("  fluxc agent-keygen    Generate SQIsign L5 agent keypair\n");
    println!("Distributed & Mesh:");
    println!("  fluxc supercluster    Distributed compilation mesh");
    println!("  fluxc self-heal [MS]  Autonomous swarm failure detection + hot-swap recovery");
    println!("  fluxc p2p-worker      Run as a P2P mesh worker node");
    println!("  fluxc swarm           Swarm coordination commands");
    println!("  fluxc fleet           Fleet status (Delta/Epsilon nodes)");
    println!("  fluxc mesh            P2P mesh health report");
    println!("  fluxc wallet          Open the TRON wallet in browser");
    println!("  fluxc os-stage        QuillonOS stage management\n");
    println!("Verification & Provenance:");
    println!("  fluxc verify-proof    Verify SQIsign artifact provenance");
    println!("  fluxc suggest FILE    Suggest lifetime annotations");
    println!("  fluxc test-native     Run Phase 3 native integration tests\n");
    println!("Webhook & API:");
    println!("  fluxc webhook-gen     Generate webhook handlers from .flux-webhook.toml");
    println!("  fluxc suggest-webhook Scan project and suggest webhooks");
    println!("  fluxc api             API generation commands");
    println!("  fluxc agility         Audit workspace agility score\n");
    println!("Documentation:");
    println!("  fluxc latex           Build LaTeX documents\n");
    println!("Flags:");
    println!("  --release, -r         Build in release mode");
    println!("  --rust-only           Force Rust-only build (ignore package.json)");
    println!("  --frontend-only       Force frontend-only build (ignore Cargo.toml)");
    println!("  --no-cargo            Phase 2: build without cargo (flux-graph)");
    println!("  --async               Parallel build across batches");
    println!("  --distributed         Use supercluster for distributed compilation");
    println!("  --warm                Pre-warm build cache");
    println!("  --provenance          Sign build artifacts with SQIsign L5");
    println!("  --package, -p CRATE   Build a specific crate in workspace\n");
    println!("Cache:");
    println!("  Rust:     SHA-256 content-hash (target/flux-cache/)");
    println!("  Frontend: BLAKE3 source-hash (target/flux-cache/frontend/)");
    println!("  Dependencies: cached by lockfile hash");
    println!("  Stats:     fluxc stats — hit rate, build times, project info");
}

// ── Onboarding / Pilot (native setup, makes new MCP tools unavoidable) ──

pub fn run_onboarding_setup(extra: &[String]) {
    println!("⚡ Flux Pilot / Setup — native onboarding (no bash required)");
    let exe = std::env::current_exe().unwrap_or_default();
    let exe_str = exe.display().to_string();
    println!("  Running from: {}", exe_str);

    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let flux_bin_dir = format!("{}/.flux/bin", home);
    let _ = std::fs::create_dir_all(&flux_bin_dir);

    let link = format!("{}/fluxc", flux_bin_dir);
    // Create symlink or copy
    if let Err(_) = std::os::unix::fs::symlink(&exe, &link) {
        let _ = std::fs::copy(&exe, &link);
    }
    println!("  ✓ Linked {} → {}", link, exe_str);
    println!("  Add to PATH if needed: export PATH=\"{}/.flux/bin:$PATH\"", home);

    // Client detection
    let clients = detect_ai_clients();
    println!("  Detected clients: {}", clients.join(", "));

    println!("\n  Next steps (new MCP tools are now the way):");
    println!("    1. Restart your AI client (Claude/Grok/etc) so MCP reconnects.");
    println!("    2. In your AI: \"flux_mcp_status\"   — health, PATH, skills, exact fixes");
    println!("    3. In your AI: \"flux_mcp_register\" — get client-specific registration snippet");
    println!("    4. In your AI: \"flux_quickstart\"   — loads docs + rules + combos");
    println!("    5. Try: fluxc combo --package fluxc-mcp");
    println!("\n  The two new tools (flux_mcp_status + flux_mcp_register) are now unavoidable in quickstart and this setup.");
    println!("  Skills: curl -fsSL https://fluxapp.xyz/flux-skills.tar.gz | tar xz -C ~/.grok/skills ~/.claude/skills");
    println!("  Canonical script source (ship this): flux/scripts/setup-flux.sh");

    if extra.iter().any(|a| a == "--auto" || a == "-y") {
        println!("  (auto mode — would attempt more wiring here in future)");
    }
}

fn detect_ai_clients() -> Vec<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut out = vec![];
    if std::path::Path::new(&format!("{}/.claude", home)).exists() { out.push("Claude".to_string()); }
    if std::path::Path::new(&format!("{}/.grok", home)).exists() { out.push("Grok".to_string()); }
    if std::path::Path::new(&format!("{}/.cursor", home)).exists() { out.push("Cursor".to_string()); }
    if std::path::Path::new(&format!("{}/.codex", home)).exists() { out.push("Codex".to_string()); }
    if out.is_empty() { out.push("none (manual MCP wiring)".to_string()); }
    out
}

// ── Self-update (uses the same prebuilt the install script prefers) ──

pub fn run_self_update(force: bool) {
    println!("⚡ fluxc self-update");
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let bin_dir = format!("{}/.flux/bin", home);
    let _ = std::fs::create_dir_all(&bin_dir);
    let target = format!("{}/fluxc", bin_dir);

    let url = "https://fluxapp.xyz/downloads/fluxc-musl-x64";
    println!("  Fetching {} ...", url);

    let status = std::process::Command::new("curl")
        .args(["-fsSL", url, "-o", &target])
        .status();

    match status {
        Ok(s) if s.success() => {
            let _ = std::process::Command::new("chmod").args(["+x", &target]).status();
            println!("  ✓ Updated {}", target);
            // `version`, not `--version`: the dashed form is the rustc -vV wrapper
            // passthrough and prints rustc's version, not fluxc's.
            if let Ok(v) = std::process::Command::new(&target).arg("version").output() {
                println!("  New version: {}", String::from_utf8_lossy(&v.stdout).trim());
            }
            println!("  Restart clients using this fluxc for MCP.");
        }
        _ => {
            eprintln!("  Failed to download prebuilt. Try manual or source build.");
            if force {
                // fallback to current exe copy
                if let Ok(exe) = std::env::current_exe() {
                    let _ = std::fs::copy(exe, &target);
                    println!("  Fallback: copied current exe to {}", target);
                }
            }
        }
    }
}

// ── Argument Parsing ──

pub fn parse_args(raw: &[String]) -> (BuildConfig, Vec<String>) {
    let mut config = BuildConfig::default();
    let mut rest = Vec::new();
    let mut i = 0;

    while i < raw.len() {
        match raw[i].as_str() {
            "--release" | "-r" => { config.release = true; i += 1; }
            "--rust-only" | "--rust" => { config.rust_only = true; i += 1; }
            "--frontend-only" | "--frontend" | "--fe" => { config.frontend_only = true; i += 1; }
            "--no-cargo" | "--no_cargo" => { config.no_cargo = true; i += 1; }
            "--async" => { config.async_build = true; i += 1; }
            "--distributed" | "--dist" => { config.distributed = true; i += 1; }
            "--warm" => { config.warm = true; i += 1; }
            "--provenance" => { config.provenance = true; i += 1; }
            "--package" | "-p" => {
                if i + 1 < raw.len() {
                    config.package = Some(raw[i + 1].clone());
                    i += 2;
                } else {
                    eprintln!("error: --package requires a value");
                    process::exit(1);
                }
            }
            arg if arg.starts_with("--package=") => {
                config.package = Some(arg["--package=".len()..].to_string());
                i += 1;
            }
            _ => { rest.push(raw[i].clone()); i += 1; }
        }
    }

    (config, rest)
}

/// Shared test helper: run `f` with $HOME pointed at a fresh temp dir, serialized
/// process-wide. Every fluxc-core module persists under $HOME/.flux/*.json
/// (predictions, webhooks, benchmarks, tune, heatmap, stats); under parallel
/// `cargo test` the per-module persistence tests would otherwise swap the global
/// $HOME out from under each other and read the wrong (or a half-written) store.
/// One lock across all of them makes every HOME-touching test deterministic.
#[cfg(test)]
pub(crate) mod test_home {
    use std::sync::Mutex;
    static LOCK: Mutex<()> = Mutex::new(());

    pub fn with_temp_home<T>(tag: &str, f: impl FnOnce() -> T) -> T {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("HOME").ok();
        let dir = std::env::temp_dir().join(format!("flux-test-home-{}", tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("HOME", &dir);
        let out = f();
        match prev {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&dir);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn tmp_dir(name: &str) -> std::path::PathBuf {
        let t = std::env::temp_dir().join(format!(
            "fluxc-core-test-{}-{}",
            name,
            SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&t).unwrap();
        t
    }

    #[test]
    fn normalize_drops_out_dir_entirely() {
        let args: Vec<String> = vec![
            "--crate-name".into(), "foo".into(),
            "--out-dir".into(), "/some/workspace/target".into(),
            "src/lib.rs".into(),
        ];
        let n = normalize_args_for_cache_key(&args);
        // --out-dir + its arg are dropped — present in input, absent in output.
        assert!(!n.iter().any(|a| a == "--out-dir"));
        assert!(!n.iter().any(|a| a == "/some/workspace/target"));
        // Other args preserved.
        assert!(n.contains(&"--crate-name".to_string()));
        assert!(n.contains(&"foo".to_string()));
        assert!(n.contains(&"src/lib.rs".to_string()));
    }

    #[test]
    fn normalize_extern_uses_filename_identity() {
        // Phase 2: a dep is identified by its FILENAME (lib<crate>-<metahash>.<ext>) — cargo's
        // deterministic fingerprint — NOT its volatile rlib bytes. Abs path/dir dropped.
        let dir = tmp_dir("extern-subst");
        let rlib_path = dir.join("libfoo-abc123.rlib");
        std::fs::write(&rlib_path, b"sample rlib bytes").unwrap();
        let args: Vec<String> = vec![
            "--extern".into(), format!("foo={}", rlib_path.display()),
        ];
        let n = normalize_args_for_cache_key(&args);
        assert_eq!(n.len(), 2);
        assert_eq!(n[0], "--extern");
        assert_eq!(n[1], "foo=<extern:libfoo-abc123.rlib>", "got {}", n[1]);
        assert!(!n[1].contains(dir.to_string_lossy().as_ref()), "abs path must be dropped");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn normalize_extern_same_filename_two_workspaces_same_key() {
        // Two workspaces, the dep's rlib at different ABS paths but the SAME filename (= same metahash
        // = same real identity). Filename-identity makes the normalized key identical -> cross-workspace
        // dedup. Different BYTES on purpose to prove it's the FILENAME, not the content, that decides.
        let ws_a = tmp_dir("ws-a");
        let ws_b = tmp_dir("ws-b");
        std::fs::write(ws_a.join("libdep-aaa.rlib"), b"bytes-a").unwrap();
        std::fs::write(ws_b.join("libdep-aaa.rlib"), b"DIFFERENT-bytes-b").unwrap();

        let args_a: Vec<String> = vec!["--extern".into(), format!("dep={}", ws_a.join("libdep-aaa.rlib").display())];
        let args_b: Vec<String> = vec!["--extern".into(), format!("dep={}", ws_b.join("libdep-aaa.rlib").display())];

        assert_eq!(normalize_args_for_cache_key(&args_a), normalize_args_for_cache_key(&args_b),
            "same dep filename from two workspaces must normalize identically");

        std::fs::remove_dir_all(&ws_a).ok();
        std::fs::remove_dir_all(&ws_b).ok();
    }

    #[test]
    fn normalize_extern_different_filename_different_key() {
        // Different real deps carry different metahash filenames -> different keys. (In reality a
        // content change always yields a new metahash, so the filename is a faithful content proxy.)
        let dir = tmp_dir("diff");
        std::fs::write(dir.join("libdep-v1.rlib"), b"a").unwrap();
        std::fs::write(dir.join("libdep-v2.rlib"), b"b").unwrap();

        let args_a: Vec<String> = vec!["--extern".into(), format!("dep={}", dir.join("libdep-v1.rlib").display())];
        let args_b: Vec<String> = vec!["--extern".into(), format!("dep={}", dir.join("libdep-v2.rlib").display())];

        assert_ne!(normalize_args_for_cache_key(&args_a), normalize_args_for_cache_key(&args_b),
            "different dep filenames must produce different keys");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn normalize_extern_uses_basename_even_when_missing() {
        // Phase 2: identity is the FILENAME, so we never read the file — even a non-existent path
        // normalizes to its basename (abs path dropped -> path/machine independent).
        let args: Vec<String> = vec![
            "--extern".into(),
            "foo=/this/path/does/not/exist/libfoo-99.rlib".into(),
        ];
        let n = normalize_args_for_cache_key(&args);
        assert_eq!(n[1], "foo=<extern:libfoo-99.rlib>");
    }

    #[test]
    fn normalize_drops_dash_l() {
        // 0.33 fix (554baf38): -L is a volatile search PATH (its deps-dir listing changes every
        // build), so it's dropped from the cache key entirely — like --out-dir. The actual deps are
        // pinned content-addressably by --extern. (Replaces the old listing-hash substitution test.)
        let args: Vec<String> = vec![
            "-L".into(), "dependency=/some/target/debug/deps".into(),
            "--crate-name".into(), "demo".into(),
        ];
        let n = normalize_args_for_cache_key(&args);
        assert!(!n.iter().any(|a| a == "-L" || a.starts_with("dependency=")),
            "-L and its path must be dropped, got {:?}", n);
        assert_eq!(n, vec!["--crate-name".to_string(), "demo".to_string()]);
    }

    #[test]
    fn parse_args_extracts_separate_package_flag() {
        let raw = vec!["test".into(), "-p".into(), "flux-p2p".into()];
        let (config, rest) = parse_args(&raw);
        assert_eq!(config.package.as_deref(), Some("flux-p2p"));
        assert_eq!(rest, vec!["test"]);
    }

    #[test]
    fn parse_args_extracts_equals_package_flag() {
        let raw = vec!["test".into(), "--package=flux-p2p".into()];
        let (config, rest) = parse_args(&raw);
        assert_eq!(config.package.as_deref(), Some("flux-p2p"));
        assert_eq!(rest, vec!["test"]);
    }

    #[test]
    fn normalize_preserves_non_path_args() {
        let args: Vec<String> = vec![
            "--crate-name".into(), "mycrate".into(),
            "--crate-type".into(), "lib".into(),
            "--edition".into(), "2021".into(),
            "-C".into(), "opt-level=1".into(),
            "--emit".into(), "metadata".into(),
            "src/lib.rs".into(),
        ];
        let n = normalize_args_for_cache_key(&args);
        // All input args present (none dropped/substituted). `--emit metadata` normalizes to itself.
        assert_eq!(n, args);
    }

    #[test]
    fn t1_emit_aware_key_distinguishes_passes() {
        // T1 (the rc=101 root): a metadata pass and a link pass of the SAME crate must produce
        // DIFFERENT normalized args, so the cache can't serve a .rmeta-only entry for a link lookup.
        let meta = normalize_args_for_cache_key(&[
            "--crate-name".into(), "demo".into(), "--emit=metadata".into()]);
        let link = normalize_args_for_cache_key(&[
            "--crate-name".into(), "demo".into(), "--emit=link,dep-info=/abs/foo.d".into()]);
        assert_ne!(meta, link, "metadata vs link passes MUST differ in the cache key");
        // ...and the volatile =output-path is dropped, so the link key is path-independent:
        let link_other = normalize_args_for_cache_key(&[
            "--crate-name".into(), "demo".into(), "--emit=link,dep-info=/OTHER/foo.d".into()]);
        assert_eq!(link, link_other, "emit normalization must drop the volatile =output-path");
        assert_eq!(normalize_emit("link,dep-info=/x.d"), "dep-info,link");
        assert_eq!(normalize_emit("metadata"), "metadata");
    }
}
