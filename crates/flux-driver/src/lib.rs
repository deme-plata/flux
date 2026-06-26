// flux-driver — rustc driver integration
//
// v0.17 (rocky-sigil-78, patch 3 of FLUXC_CACHE_GAP.md): real byte-level
// caching of rustc outputs. Before this version, `collect_outputs` stored
// constructed paths like `<crate>.rmeta` that don't match rustc's actual
// output names (`lib<crate>-<hash>.rmeta`), so cache entries arrived with
// empty `outputs` maps — useless for restore. And `apply_cached_outputs`
// only wrote a stub `.d` marker. Both fixed here.
//
// New layout:
//
//   target/flux-cache/blobs/<source_hash>/
//       lib<crate>-<rustc-hash>.rmeta    ← byte copy of rustc's output
//       lib<crate>-<rustc-hash>.rlib     ← byte copy
//       <crate>-<rustc-hash>.d           ← byte copy
//
// `collect_outputs` scans --out-dir AFTER rustc ran, finds files matching
// the crate name + recognised extensions, copies them to the blob dir,
// and stores the file names (relative) in CacheEntry.outputs.
//
// `apply_cached_outputs` reads the blobs back and writes them to the new
// --out-dir with the same file names. Returns `bool` so callers can fall
// through to a real rustc invocation when the cache can't restore cleanly.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// FIP-0001 keep-A-open #2: the pinned rustc whose `--emit=mir` dialect Flux parses and whose std
/// rlibs the cache is keyed to. This is the operational half of the "contracted frontend" — a single
/// source of truth instead of a magic string. The CI MIR-diff job (.github/workflows/mir-diff.yml)
/// regenerates MIR for a fixed corpus against THIS version and fails the build on any dialect drift,
/// turning the rustc dependency into an enforced contract rather than an implicit assumption.
pub const RUSTC_VERSION: &str = "1.93.1";

/// Where blobs live for a given source-hash key. Relative to current
/// working directory's `target/` — matches `flux_cache::clean`'s convention.
fn blob_dir_for(source_hash: &str) -> PathBuf {
    PathBuf::from("target")
        .join("flux-cache")
        .join("blobs")
        .join(source_hash)
}

/// Extract `--crate-name foo` from rustc args. Returns empty string when
/// missing (rare — cargo always sets it).
fn find_crate_name(args: &[String]) -> String {
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--crate-name" && i + 1 < args.len() {
            return args[i + 1].clone();
        }
        i += 1;
    }
    String::new()
}

/// Extract `--out-dir` from rustc args.
fn find_out_dir(args: &[String]) -> String {
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--out-dir" && i + 1 < args.len() {
            return args[i + 1].clone();
        }
        i += 1;
    }
    String::new()
}

/// Cache T2: the artifact extensions THIS invocation must produce, derived from `--emit`.
/// A `--emit=link` lib pass requires an `.rlib`; a `--emit=metadata` pass requires an `.rmeta`.
/// apply_cached_outputs refuses a cached entry that doesn't cover these (it would otherwise ship a
/// build missing the artifact downstream needs -> "extern location does not exist" rc=101).
fn required_exts(args: &[String]) -> Vec<&'static str> {
    let mut emit = String::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--emit" && i + 1 < args.len() { emit = args[i + 1].clone(); break; }
        if let Some(v) = args[i].strip_prefix("--emit=") { emit = v.to_string(); break; }
        i += 1;
    }
    let kinds: Vec<&str> = emit.split(',').map(|p| p.split('=').next().unwrap_or(p)).collect();
    let mut req = Vec::new();
    if kinds.contains(&"metadata") { req.push("rmeta"); }
    if kinds.contains(&"link") { req.push("rlib"); } // a lib link pass; ext-less bins aren't cached anyway
    req
}

/// Cache T2b: the `-C extra-filename=<suffix>` (e.g. "-361b6c44…") that determines this crate's output
/// filename `lib<crate><suffix>.<ext>`. Cargo passes it as `-C extra-filename=…` or `-Cextra-filename=…`.
fn find_extra_filename(args: &[String]) -> String {
    let mut i = 0;
    while i < args.len() {
        if args[i] == "-C" && i + 1 < args.len() {
            if let Some(v) = args[i + 1].strip_prefix("extra-filename=") { return v.to_string(); }
        }
        if let Some(v) = args[i].strip_prefix("-Cextra-filename=") { return v.to_string(); }
        i += 1;
    }
    String::new()
}

/// Recognised emit extensions. The empty string covers static-libs / binaries
/// that rustc emits without an extension (e.g. an `--emit=link` binary).
// Cache T3: `.d` (dep-info) is deliberately NOT cached. It's a Makefile fragment of ABSOLUTE
// source/dep/out paths from the producing build; restoring it into another out-dir makes cargo
// either error or silently mark the unit dirty. rustc regenerates it for free, so we never restore it.
const CACHEABLE_EXTS: &[&str] = &["rmeta", "rlib"];

/// Scan `out_dir` for files belonging to this crate's compilation and return
/// the matching file names (without the directory prefix). Recognised by
/// containing `crate_name` as a substring and having a known extension.
/// rustc generates names like `lib<crate>-<metadata-hash>.<ext>`; the hash
/// is deterministic for a given (args, source) so the SAME compilation
/// re-run will land on the same file name — which is exactly what makes
/// the side-blob cache work.
fn scan_outputs(out_dir: &Path, crate_name: &str) -> Vec<String> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(out_dir) else { return found; };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.contains(crate_name) { continue; }
        // Quickly filter by recognised extension.
        if let Some(dot) = name.rfind('.') {
            let ext = &name[dot + 1..];
            if !CACHEABLE_EXTS.contains(&ext) {
                continue;
            }
        } else {
            // No extension — could be a binary. Skip for safety.
            continue;
        }
        found.push(name);
    }
    found
}

/// Side-blob a single output file. Returns the file name on success.
fn copy_output_to_blob(out_dir: &Path, blob_dir: &Path, file_name: &str) -> Option<String> {
    let src = out_dir.join(file_name);
    let dst = blob_dir.join(file_name);
    fs::create_dir_all(blob_dir).ok()?;
    fs::copy(&src, &dst).ok()?;
    Some(file_name.to_string())
}

/// Restore a single output file from the blob dir. Returns true if the
/// file was successfully written to out_dir.
fn restore_output_from_blob(blob_dir: &Path, out_dir: &Path, file_name: &str) -> bool {
    let src = blob_dir.join(file_name);
    let dst = out_dir.join(file_name);
    if !src.exists() { return false; }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::copy(&src, &dst).is_ok()
}

/// Collect outputs from a successful rustc invocation, side-blob the file
/// contents, and return a CacheEntry pointing at the blob file names.
///
/// MUST be called AFTER rustc has run successfully (the function reads the
/// files rustc just emitted).
pub fn collect_outputs(rustc_args: &[String], cache_key: &str) -> flux_cache::CacheEntry {
    let raw_args: Vec<String> = rustc_args.to_vec();

    // Find source file (positional .rs arg).
    let source_file = rustc_args.iter()
        .find(|a| a.ends_with(".rs") && !a.starts_with("--"))
        .cloned()
        .unwrap_or_default();

    let crate_name = find_crate_name(&raw_args);
    let out_dir = find_out_dir(&raw_args);

    let mut outputs = HashMap::new();

    if !source_file.is_empty() && !crate_name.is_empty() && !out_dir.is_empty() {
        // 0.33: key blobs on the wrapper's STABLE normalized cache_key (same key the
        // entry is stored/looked-up under), not a raw-args hash that embeds absolute
        // --extern/-L/--out-dir paths. The raw-args fallback fires only for keyless
        // direct callers (the unit tests). This is the fix that makes apply_cached_
        // outputs actually find its blobs (was 0% effective over 753 builds).
        let source_hash = if !cache_key.is_empty() {
            cache_key.to_string()
        } else {
            flux_cache::compute_hash(Some(&source_file), &raw_args)
        };
        if !source_hash.is_empty() {
            let blob_dir = blob_dir_for(&source_hash);
            let out_dir_path = PathBuf::from(&out_dir);
            for file_name in scan_outputs(&out_dir_path, &crate_name) {
                if let Some(stored) = copy_output_to_blob(&out_dir_path, &blob_dir, &file_name) {
                    // Key by extension (or empty for no extension) so the
                    // entry's `outputs` map matches the historic shape of
                    // emit_type → name. Multiple files of the same ext for
                    // one crate are unusual; pick the last one scanned.
                    let key = stored.rsplit_once('.')
                        .map(|(_, ext)| ext.to_string())
                        .unwrap_or_else(|| "link".to_string());
                    outputs.insert(key, stored);
                }
            }
        }
    }

    // entry.source_hash is what apply_cached_outputs uses to reconstruct the blob
    // dir on a hit — it MUST equal the key the blobs were written under (cache_key).
    let source_hash = if !cache_key.is_empty() {
        cache_key.to_string()
    } else if !source_file.is_empty() {
        flux_cache::compute_hash(Some(&source_file), &raw_args)
    } else {
        String::new()
    };

    flux_cache::CacheEntry {
        source_hash: source_hash.clone(),
        args_hash: source_hash,
        outputs,
        rustc_version: RUSTC_VERSION.into(),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    }
}

/// Restore cached outputs into the rustc-expected out_dir. Returns `true`
/// when every file in `entry.outputs` restored successfully (cache hit
/// can safely skip rustc); `false` otherwise (caller falls through to
/// running rustc for real).
pub fn apply_cached_outputs(entry: &flux_cache::CacheEntry, rustc_args: &[String]) -> bool {
    if entry.outputs.is_empty() || entry.source_hash.is_empty() {
        return false;
    }
    // T2 required-ext guard: a cache hit MUST carry every artifact this --emit produces (an rlib for
    // a link pass, an rmeta for a metadata pass). A metadata-only entry served to a link pass would
    // skip rustc and leave no .rlib -> downstream "extern location does not exist" rc=101. Refuse it;
    // the wrapper then runs real rustc. (entry.outputs is keyed by extension.)
    for ext in required_exts(rustc_args) {
        if !entry.outputs.contains_key(ext) { return false; }
    }
    let out_dir = find_out_dir(rustc_args);
    if out_dir.is_empty() { return false; }
    let blob_dir = blob_dir_for(&entry.source_hash);
    if !blob_dir.exists() { return false; }
    let out_dir_path = PathBuf::from(&out_dir);
    for file_name in entry.outputs.values() {
        if !restore_output_from_blob(&blob_dir, &out_dir_path, file_name) {
            return false;
        }
    }
    // T2b post-restore NAME guard (the real rc=101 fix): verify the EXACT artifact this invocation
    // expects — lib<crate><extra-filename>.<ext> — now exists. The cache is content-keyed, but a hit
    // can restore a DIFFERENT metadata-hash variant of a multi-feature external dep (we saw libc as
    // both -361b6c44 and -cf571322); the restored .rmeta then carries the wrong embedded hash + name,
    // so a consumer's `--extern <dep>=lib<dep>-<expected>.rmeta` can't find it -> rc=101. If the exact
    // expected file isn't present after restore, treat it as a MISS and run real rustc.
    let cn = find_crate_name(rustc_args);
    let extra = find_extra_filename(rustc_args);
    if !cn.is_empty() {
        for ext in required_exts(rustc_args) {
            if !out_dir_path.join(format!("lib{}{}.{}", cn, extra, ext)).exists() {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    // Tests below mutate the process-global CWD (blob_dir_for is CWD-relative); serialize them so
    // cargo's parallel runner (what a `fluxc combo` invokes) can't race them. Poison-tolerant.
    static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn cwd_guard() -> std::sync::MutexGuard<'static, ()> {
        CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn tmp_dir(name: &str) -> PathBuf {
        let t = std::env::temp_dir().join(format!("flux-driver-test-{}-{}", name, SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir_all(&t).unwrap();
        t
    }

    #[test]
    fn find_crate_name_and_out_dir() {
        let args: Vec<String> = vec![
            "--crate-name".into(), "mycrate".into(),
            "--out-dir".into(), "/tmp/flux-test".into(),
            "src/lib.rs".into(),
        ];
        assert_eq!(find_crate_name(&args), "mycrate");
        assert_eq!(find_out_dir(&args), "/tmp/flux-test");
    }

    #[test]
    fn scan_outputs_finds_crate_files() {
        let dir = tmp_dir("scan");
        fs::write(dir.join("libfoo-abc123.rmeta"), b"r").unwrap();
        fs::write(dir.join("libfoo-abc123.rlib"), b"l").unwrap();
        fs::write(dir.join("foo-abc123.d"), b"d").unwrap();      // T3: .d is no longer cached
        fs::write(dir.join("libbar-xyz.rmeta"), b"x").unwrap();   // wrong crate
        fs::write(dir.join("libfoo-abc123.so"), b"so").unwrap();  // unsupported ext
        let found = scan_outputs(&dir, "foo");
        assert_eq!(found.len(), 2, "T3: only rmeta+rlib, NOT .d — got {found:?}");
        assert!(found.iter().any(|f| f == "libfoo-abc123.rmeta"));
        assert!(found.iter().any(|f| f == "libfoo-abc123.rlib"));
        assert!(!found.iter().any(|f| f == "foo-abc123.d"), "T3: .d must NOT be collected");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn collect_then_apply_roundtrip() {
        let _cwd = cwd_guard();
        // We can't run rustc in unit tests — simulate by:
        // 1. pre-populating an `out_dir` with fake "outputs"
        // 2. calling collect_outputs (which side-blobs them)
        // 3. clearing out_dir
        // 4. calling apply_cached_outputs and asserting the files come back

        let workspace = tmp_dir("roundtrip");
        let out_dir = workspace.join("out");
        fs::create_dir_all(&out_dir).unwrap();
        let src_file = workspace.join("src").join("lib.rs");
        fs::create_dir_all(src_file.parent().unwrap()).unwrap();
        fs::write(&src_file, b"// hello").unwrap();
        let rmeta_content = b"fake rmeta bytes";
        let rlib_content = b"fake rlib bytes longer content here";
        fs::write(out_dir.join("libdemo-deadbeef.rmeta"), rmeta_content).unwrap();
        fs::write(out_dir.join("libdemo-deadbeef.rlib"), rlib_content).unwrap();

        // CWD has to be the workspace for blob_dir_for("target/...") to land somewhere we can clean up.
        let orig_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&workspace).unwrap();

        let args: Vec<String> = vec![
            "--crate-name".into(), "demo".into(),
            "--out-dir".into(), out_dir.to_string_lossy().to_string(),
            src_file.to_string_lossy().to_string(),
        ];

        let key = flux_cache::compute_hash(Some(&src_file.to_string_lossy()), &args);
        let entry = collect_outputs(&args, &key);
        assert!(!entry.source_hash.is_empty(), "entry has no source_hash");
        assert_eq!(entry.source_hash, key, "0.33: entry.source_hash must equal the cache_key");
        assert_eq!(entry.outputs.len(), 2, "expected 2 outputs, got {:?}", entry.outputs);

        // Clear the out_dir to prove apply actually restores.
        fs::remove_dir_all(&out_dir).unwrap();
        fs::create_dir_all(&out_dir).unwrap();
        assert!(!out_dir.join("libdemo-deadbeef.rmeta").exists());

        let applied = apply_cached_outputs(&entry, &args);
        assert!(applied, "apply_cached_outputs returned false");
        assert_eq!(fs::read(out_dir.join("libdemo-deadbeef.rmeta")).unwrap(), rmeta_content);
        assert_eq!(fs::read(out_dir.join("libdemo-deadbeef.rlib")).unwrap(), rlib_content);

        std::env::set_current_dir(&orig_cwd).unwrap();
        fs::remove_dir_all(&workspace).ok();
    }

    #[test]
    fn stable_key_survives_volatile_args() {
        let _cwd = cwd_guard();
        // 0.33 core property: blobs+entry are keyed on the STABLE cache_key, so a
        // hit succeeds even when the raw rustc args differ (e.g. a different absolute
        // --extern path) — the cross-workspace / post-clean reuse case. Pre-fix the
        // blob dir was keyed on a raw-args hash that embedded those paths, so apply
        // could never find the blobs when the args varied.
        let workspace = tmp_dir("stablekey");
        let out_dir = workspace.join("out");
        fs::create_dir_all(&out_dir).unwrap();
        let src_file = workspace.join("src").join("lib.rs");
        fs::create_dir_all(src_file.parent().unwrap()).unwrap();
        fs::write(&src_file, b"// x").unwrap();
        fs::write(out_dir.join("libdemo-deadbeef.rmeta"), b"meta").unwrap();
        fs::write(out_dir.join("libdemo-deadbeef.rlib"), b"lib").unwrap();
        let orig_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&workspace).unwrap();

        let args1: Vec<String> = vec![
            "--crate-name".into(), "demo".into(),
            "--extern".into(), "serde=/ws1/target/libserde-AAAA.rlib".into(),
            "--out-dir".into(), out_dir.to_string_lossy().to_string(),
            src_file.to_string_lossy().to_string(),
        ];
        // Same logical compile, DIFFERENT absolute --extern path (a second workspace).
        let args2: Vec<String> = vec![
            "--crate-name".into(), "demo".into(),
            "--extern".into(), "serde=/ws2/target/libserde-BBBB.rlib".into(),
            "--out-dir".into(), out_dir.to_string_lossy().to_string(),
            src_file.to_string_lossy().to_string(),
        ];
        const STABLE: &str = "stable-cache-key-deadbeef";
        let entry = collect_outputs(&args1, STABLE);
        assert_eq!(entry.source_hash, STABLE);
        assert!(blob_dir_for(STABLE).exists(), "blobs must live under the stable key");

        fs::remove_dir_all(&out_dir).unwrap();
        fs::create_dir_all(&out_dir).unwrap();
        // apply with args2 (different abs paths) — succeeds because the blob dir is
        // keyed on entry.source_hash == STABLE, independent of the raw args.
        assert!(apply_cached_outputs(&entry, &args2),
            "stable-key hit must restore regardless of volatile raw args");
        assert_eq!(fs::read(out_dir.join("libdemo-deadbeef.rmeta")).unwrap(), b"meta");

        std::env::set_current_dir(&orig_cwd).unwrap();
        fs::remove_dir_all(&workspace).ok();
    }

    #[test]
    fn apply_returns_false_when_blob_dir_missing() {
        let _cwd = cwd_guard();
        let workspace = tmp_dir("missing");
        let orig_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&workspace).unwrap();

        let args: Vec<String> = vec![
            "--crate-name".into(), "demo".into(),
            "--out-dir".into(), workspace.join("out").to_string_lossy().to_string(),
            workspace.join("src.rs").to_string_lossy().to_string(),
        ];
        let entry = flux_cache::CacheEntry {
            source_hash: "fakehash".into(),
            args_hash: "fakehash".into(),
            outputs: vec![("rmeta".into(), "libdemo-xyz.rmeta".into())].into_iter().collect(),
            rustc_version: RUSTC_VERSION.into(),
            timestamp: 0,
        };
        let applied = apply_cached_outputs(&entry, &args);
        assert!(!applied, "apply succeeded when blob dir doesn't exist");

        std::env::set_current_dir(&orig_cwd).unwrap();
        fs::remove_dir_all(&workspace).ok();
    }

    #[test]
    fn apply_returns_false_when_entry_empty() {
        let args: Vec<String> = vec!["--out-dir".into(), "/tmp/x".into()];
        let entry = flux_cache::CacheEntry {
            source_hash: "".into(),
            args_hash: "".into(),
            outputs: HashMap::new(),
            rustc_version: RUSTC_VERSION.into(),
            timestamp: 0,
        };
        assert!(!apply_cached_outputs(&entry, &args));
    }

    #[test]
    fn t2_required_exts_and_link_rejection() {
        // T2: --emit drives required artifacts, and a metadata-only entry is REFUSED for a link pass
        // (the rc=101 root: serving a .rmeta-only entry to a --emit=link unit leaves no .rlib).
        assert_eq!(required_exts(&["--emit=link".to_string()]), vec!["rlib"]);
        assert_eq!(required_exts(&["--emit=metadata".to_string()]), vec!["rmeta"]);
        assert_eq!(required_exts(&["--emit".to_string(), "link,dep-info=/x.d".to_string()]), vec!["rlib"]);
        let meta_only = flux_cache::CacheEntry {
            source_hash: "k".into(), args_hash: "k".into(),
            outputs: vec![("rmeta".to_string(), "libdemo-x.rmeta".to_string())].into_iter().collect(),
            rustc_version: RUSTC_VERSION.into(), timestamp: 0,
        };
        let link: Vec<String> = vec!["--crate-name".into(), "demo".into(), "--emit=link".into(),
                                     "--out-dir".into(), "/tmp/x".into()];
        assert!(!apply_cached_outputs(&meta_only, &link),
            "T2: a metadata-only entry must NOT satisfy a --emit=link pass");
        // T2b: extra-filename extraction (both forms), used for the post-restore exact-name guard.
        assert_eq!(find_extra_filename(&["-C".into(), "extra-filename=-361b6c44".into()]), "-361b6c44");
        assert_eq!(find_extra_filename(&["-Cextra-filename=-cf571322".into()]), "-cf571322");
        assert_eq!(find_extra_filename(&["--crate-name".into(), "demo".into()]), "");
    }
}
