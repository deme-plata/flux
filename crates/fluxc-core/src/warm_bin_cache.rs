//! warm_bin_cache — attack the COLD first-hit (H&P "locality + caches" leg of v2).
//!
//! A combo's cold path rebuilds a crate's dep tree + test binary from scratch on a
//! fresh `target/`. This module makes that cold hit WARM by content-addressing the
//! crate's compiled artifacts:
//!
//!   key   = blake3(crate src + Cargo.toml + workspace Cargo.lock + rustc version)
//!   store = tar the crate's `target/debug/{deps,.fingerprint,build}` entries → ~/.fluxc/warm_bins/<key>.tgz
//!   restore = extract them back (GNU tar restores the archived MTIMES), so cargo's
//!             fingerprint check passes and it SKIPS the recompile — same trick CI
//!             rust-cache uses. No fragile fingerprint surgery.
//!
//! Plus `reverse_dep_prewarm`: after a green combo, background-build the crates most
//! likely to be comboed next (dependents, via flux-graph), so THEIR next combo is warm
//! too (H&P speculation/prefetch).
//!
//! Pure std + blake3 + system `tar`/`cp` — no new crate deps. All side effects are
//! best-effort: a cache miss or a tar error degrades to a normal (cold) build, never
//! an error.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Cache root: `$FLUX_WARM_CACHE` or `~/.fluxc/warm_bins`.
pub fn cache_dir() -> PathBuf {
    if let Ok(p) = std::env::var("FLUX_WARM_CACHE") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    PathBuf::from(home).join(".fluxc").join("warm_bins")
}

/// The cargo target dir cargo will actually use: `$CARGO_TARGET_DIR` else `<ws>/target`.
pub fn target_dir(workspace_root: &Path) -> PathBuf {
    if let Ok(p) = std::env::var("CARGO_TARGET_DIR") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    workspace_root.join("target")
}

/// Cargo mangles `-` to `_` in artifact filenames.
fn mangle(pkg: &str) -> String {
    pkg.replace('-', "_")
}

fn rustc_version() -> String {
    Command::new(std::env::var("REAL_RUSTC").unwrap_or_else(|_| "rustc".to_string()))
        .arg("--version")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "rustc-unknown".to_string())
}

/// Content key for a crate: hashes its source tree + manifest + the workspace lock +
/// rustc version. Same inputs → same key → safe restore. Returns short hex (16 chars).
pub fn crate_key(workspace_root: &Path, pkg: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(rustc_version().as_bytes());
    // workspace lock (dep graph) — changes invalidate every crate's artifacts
    if let Ok(lock) = std::fs::read(workspace_root.join("Cargo.lock")) {
        hasher.update(&lock);
    }
    let crate_dir = workspace_root.join("crates").join(pkg);
    // manifest
    if let Ok(m) = std::fs::read(crate_dir.join("Cargo.toml")) {
        hasher.update(&m);
    }
    // every .rs under the crate's src/, in sorted order for determinism
    let mut files: Vec<PathBuf> = Vec::new();
    collect_rs(&crate_dir.join("src"), &mut files);
    files.sort();
    for f in &files {
        if let Ok(b) = std::fs::read(f) {
            hasher.update(f.to_string_lossy().as_bytes());
            hasher.update(&b);
        }
    }
    hasher.finalize().to_hex()[..16].to_string()
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect_rs(&p, out);
            } else if p.extension().map(|x| x == "rs").unwrap_or(false) {
                out.push(p);
            }
        }
    }
}

fn cache_file(key: &str, pkg: &str) -> PathBuf {
    cache_dir().join(format!("{}-{}.tgz", mangle(pkg), key))
}

/// Relative paths (under `target/debug`) of a crate's artifacts: deps, fingerprints,
/// build-script outputs. Globs by the mangled crate name. Returns paths relative to
/// `<target>/debug` (the tar `-C` base) so store/restore are symmetric.
fn artifact_rel_paths(target: &Path, pkg: &str) -> Vec<String> {
    let m = mangle(pkg);
    let debug = target.join("debug");
    let mut rels = Vec::new();
    for (sub, want_dir) in [("deps", false), (".fingerprint", true), ("build", true)] {
        let d = debug.join(sub);
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                // artifact names are `<mangled>-<hash>` or `lib<mangled>-<hash>.rlib`
                let stem = name.trim_start_matches("lib");
                if stem.starts_with(&format!("{m}-")) {
                    if want_dir || !e.path().is_dir() {
                        rels.push(format!("{sub}/{name}"));
                    }
                }
            }
        }
    }
    rels
}

/// Store a crate's compiled artifacts under its content key. Best-effort; returns the
/// number of top-level artifact entries archived (0 = nothing to cache / skipped).
pub fn store(workspace_root: &Path, pkg: &str, key: &str) -> usize {
    let target = target_dir(workspace_root);
    let rels = artifact_rel_paths(&target, pkg);
    if rels.is_empty() {
        return 0;
    }
    let cd = cache_dir();
    let _ = std::fs::create_dir_all(&cd);
    let out = cache_file(key, pkg);
    let tmp = out.with_extension("tgz.tmp");
    // tar -C <target>/debug -czf tmp <rels...>  (mtimes captured; restored on extract)
    let mut cmd = Command::new("tar");
    cmd.arg("-C").arg(target.join("debug"));
    cmd.arg("-czf").arg(&tmp);
    for r in &rels {
        cmd.arg(r);
    }
    match cmd.status() {
        Ok(s) if s.success() => {
            let _ = std::fs::rename(&tmp, &out);
            rels.len()
        }
        _ => {
            let _ = std::fs::remove_file(&tmp);
            0
        }
    }
}

/// Restore a crate's artifacts for `key` into the target (cold → warm). Returns true on
/// a cache HIT (archive existed and extracted). GNU tar restores archived mtimes, so
/// cargo's fingerprint check then SKIPS recompiling this crate.
pub fn restore(workspace_root: &Path, pkg: &str, key: &str) -> bool {
    let f = cache_file(key, pkg);
    if !f.exists() {
        return false;
    }
    let target = target_dir(workspace_root);
    let debug = target.join("debug");
    if std::fs::create_dir_all(&debug).is_err() {
        return false;
    }
    let ok = Command::new("tar")
        .arg("-C").arg(&debug)
        .arg("-xzf").arg(&f)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        // bump the archive's mtime so LRU eviction treats a restored entry as fresh
        let _ = Command::new("touch").arg(&f).status();
    }
    ok
}

/// LRU eviction: keep total cache under `max_bytes` (default 10 GiB), dropping the
/// oldest `.tgz` files first. Best-effort.
pub fn evict(max_bytes: u64) {
    let cd = cache_dir();
    let mut entries: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&cd) {
        for e in rd.flatten() {
            if e.path().extension().map(|x| x == "tgz").unwrap_or(false) {
                if let Ok(md) = e.metadata() {
                    let mt = md.modified().unwrap_or(std::time::UNIX_EPOCH);
                    entries.push((e.path(), md.len(), mt));
                }
            }
        }
    }
    let mut total: u64 = entries.iter().map(|(_, sz, _)| *sz).sum();
    if total <= max_bytes {
        return;
    }
    entries.sort_by_key(|(_, _, mt)| *mt); // oldest first
    for (p, sz, _) in entries {
        if total <= max_bytes {
            break;
        }
        if std::fs::remove_file(&p).is_ok() {
            total = total.saturating_sub(sz);
        }
    }
}

/// After a green combo on `pkg`, background-build the crates most likely to be comboed
/// next — its direct dependents (from flux-graph) — so their next combo is warm.
/// Fire-and-forget, capped at `max` crates, niced. Returns the prewarmed crate names.
pub fn reverse_dep_prewarm(workspace_root: &Path, pkg: &str, max: usize) -> Vec<String> {
    let deps = direct_dependents(workspace_root, pkg);
    let flux_exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("fluxc"));
    let mut fired = Vec::new();
    for dep in deps.into_iter().take(max) {
        // nice + detached `fluxc test -p <dep>` — warms its test artifacts; ignore result
        let ok = Command::new("nice")
            .arg("-n").arg("15")
            .arg(&flux_exe)
            .args(["test", "-p", &dep])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .is_ok();
        if ok {
            fired.push(dep);
        }
    }
    fired
}

/// Direct dependents of `pkg`: workspace crates whose Cargo.toml path-depends on it.
/// Lightweight scan (no full graph build) so prewarm stays cheap.
fn direct_dependents(workspace_root: &Path, pkg: &str) -> Vec<String> {
    let crates = workspace_root.join("crates");
    let needle = format!("\"{pkg}\"");
    let needle2 = format!("path = \"../{pkg}\"");
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&crates) {
        for e in rd.flatten() {
            let manifest = e.path().join("Cargo.toml");
            let name = e.file_name().to_string_lossy().to_string();
            if name == pkg {
                continue;
            }
            if let Ok(s) = std::fs::read_to_string(&manifest) {
                // crate depends on pkg if it names it as a dep (path or name = "<pkg>")
                if s.contains(&needle2)
                    || s.lines().any(|l| l.trim_start().starts_with(&format!("{pkg} ="))
                        || l.contains(&format!("{pkg} = {{"))
                        || l.contains(&needle) && l.contains("path"))
                {
                    out.push(name);
                }
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "wbc-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn mangle_dashes() {
        assert_eq!(mangle("flux-swarm-secret"), "flux_swarm_secret");
    }

    #[test]
    fn key_is_deterministic_and_source_sensitive() {
        let ws = tmp();
        let cdir = ws.join("crates").join("demo").join("src");
        std::fs::create_dir_all(&cdir).unwrap();
        std::fs::write(ws.join("Cargo.lock"), b"lockv1").unwrap();
        std::fs::write(ws.join("crates").join("demo").join("Cargo.toml"), b"[package]\nname=\"demo\"").unwrap();
        std::fs::write(cdir.join("lib.rs"), b"fn a(){}").unwrap();
        let k1 = crate_key(&ws, "demo");
        let k2 = crate_key(&ws, "demo");
        assert_eq!(k1, k2, "same inputs → same key");
        assert_eq!(k1.len(), 16);
        // change source → key changes
        std::fs::write(cdir.join("lib.rs"), b"fn a(){ /*x*/ }").unwrap();
        assert_ne!(k1, crate_key(&ws, "demo"), "source change → new key");
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn store_restore_roundtrip_preserves_artifacts() {
        let ws = tmp();
        // fake an artifact in target/debug/deps
        let deps = ws.join("target").join("debug").join("deps");
        std::fs::create_dir_all(&deps).unwrap();
        std::fs::write(deps.join("libdemo-abc123.rlib"), b"ARTIFACT-BYTES").unwrap();
        let fp = ws.join("target").join("debug").join(".fingerprint").join("demo-abc123");
        std::fs::create_dir_all(&fp).unwrap();
        std::fs::write(fp.join("lib-demo"), b"fp").unwrap();
        // isolate the cache under this tmp
        std::env::set_var("FLUX_WARM_CACHE", ws.join("cache"));

        let n = store(&ws, "demo", "key123");
        assert!(n >= 1, "stored at least the rlib + fingerprint");
        // wipe target → simulate a COLD checkout
        std::fs::remove_dir_all(ws.join("target")).unwrap();
        assert!(!deps.join("libdemo-abc123.rlib").exists());

        let hit = restore(&ws, "demo", "key123");
        assert!(hit, "cache hit");
        let restored = std::fs::read(deps.join("libdemo-abc123.rlib")).unwrap();
        assert_eq!(restored, b"ARTIFACT-BYTES", "artifact bytes restored");
        assert!(fp.join("lib-demo").exists(), "fingerprint restored");

        // miss for a different key
        assert!(!restore(&ws, "demo", "otherkey"));
        std::env::remove_var("FLUX_WARM_CACHE");
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn dependents_scan_finds_path_dep() {
        let ws = tmp();
        let c = ws.join("crates");
        std::fs::create_dir_all(c.join("base")).unwrap();
        std::fs::create_dir_all(c.join("user")).unwrap();
        std::fs::write(c.join("base").join("Cargo.toml"), b"[package]\nname=\"base\"").unwrap();
        std::fs::write(
            c.join("user").join("Cargo.toml"),
            b"[package]\nname=\"user\"\n[dependencies]\nbase = { path = \"../base\" }",
        ).unwrap();
        let deps = direct_dependents(&ws, "base");
        assert!(deps.contains(&"user".to_string()), "user depends on base");
        assert!(!deps.contains(&"base".to_string()), "crate is not its own dependent");
        let _ = std::fs::remove_dir_all(&ws);
    }
}
