// flux-driver — rustc driver integration
//
// v0.17 (rocky-sigil-78, patch 3 of FLUXC_CACHE_GAP.md): real byte-level
// caching of rustc outputs. Before this version, `collect_outputs` stored
// constructed paths like `<crate>.rmeta` that don't match rustc's actual
// output names (`lib<crate>-<hash>.rmeta`), so cache entries arrived with
// empty `outputs` maps — useless for restore. And `apply_cached_outputs`
// only wrote a stub `.d` marker. Both fixed here.
//
// New layout (v0.36: rooted in the SHARED flux_cache::cache_dir(), i.e.
// $HOME/.flux/cache by default / FLUX_CACHE_DIR override — no longer under
// the workspace's target/, so `rm -rf target` keeps the cache):
//
//   <cache_dir>/blobs/<source_hash>/
//       lib<crate>-<rustc-hash>.rmeta    ← byte copy of rustc's output
//       lib<crate>-<rustc-hash>.rlib     ← byte copy
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

/// Where blobs live for a given source-hash key.
///
/// v0.36: rooted in `flux_cache::cache_dir()` — the SAME directory the entry
/// index lives in ($HOME/.flux/cache by default, FLUX_CACHE_DIR to override).
/// Pre-0.36 this was a LITERAL CWD-relative `target/flux-cache` while
/// flux-cache itself walked up from CWD looking for a `target/` dir — two
/// subtly different roots that could disagree (and both were wiped by
/// `rm -rf target`, which is why cold builds never restored).
pub fn blob_dir_for(source_hash: &str) -> PathBuf {
    flux_cache::cache_dir().join("blobs").join(source_hash)
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

/// (filename, full-path) for each `--extern name=path` whose path is present. The filename is the
/// dep's stable identity (lib<crate>-<metahash>.<ext>); the path is where its rmeta/rlib lives now.
fn extern_dep_paths(args: &[String]) -> Vec<(String, String)> {
    let mut v = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--extern" && i + 1 < args.len() {
            if let Some((_name, path)) = args[i + 1].split_once('=') {
                if let Some(base) = std::path::Path::new(path).file_name() {
                    v.push((base.to_string_lossy().to_string(), path.to_string()));
                }
            }
        }
        i += 1;
    }
    v
}

/// T2c closure-consistency sidecar: `<cache_dir>/closure/<cache_key>` records the content-hash of
/// every --extern dep the entry was built against, so a restore can verify the CURRENT deps still
/// match. v0.36: rooted in the shared `flux_cache::cache_dir()`, same as the blobs.
fn closure_sidecar_for(cache_key: &str) -> PathBuf {
    flux_cache::cache_dir().join("closure").join(cache_key)
}

/// Recognised emit extensions. The empty string covers static-libs / binaries
/// that rustc emits without an extension (e.g. an `--emit=link` binary).
// Cache T3 (revised): `.d` (dep-info) IS cached, but never raw-restored. The original T3 dropped it
// entirely ("rustc regenerates it for free") — false on the HIT path, where rustc never runs: cargo
// then has no dep-info to encode into .fingerprint/dep-lib-*, logs "fingerprint error", and the unit
// goes StaleItem(MissingFile dep-lib-*) DIRTY on every subsequent build forever (measured 2026-07-21:
// 19 permanently-dirty registry roots cascading into 166 dirty units, sigil "no-op" check = 216s).
// The stale-absolute-path hazard T3 was guarding against is handled at restore time instead:
// restore_dep_info() validates every dep path against THIS invocation and rewrites the volatile
// target side; anything unverifiable is a MISS, never a guess.
const CACHEABLE_EXTS: &[&str] = &["rmeta", "rlib"];

/// Cache T3-fix: where THIS invocation's dep-info (.d) lands — `Some(path)` when `--emit` includes
/// dep-info (every real cargo unit does). An explicit `dep-info=<path>` wins; otherwise rustc's
/// default `<out_dir>/<crate_name><extra-filename>.d`.
fn dep_info_dest(args: &[String]) -> Option<PathBuf> {
    let mut emit = String::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--emit" && i + 1 < args.len() { emit = args[i + 1].clone(); break; }
        if let Some(v) = args[i].strip_prefix("--emit=") { emit = v.to_string(); break; }
        i += 1;
    }
    for part in emit.split(',') {
        let mut it = part.splitn(2, '=');
        if it.next() != Some("dep-info") { continue; }
        if let Some(p) = it.next() { return Some(PathBuf::from(p)); }
        let out_dir = find_out_dir(args);
        let cn = find_crate_name(args);
        if out_dir.is_empty() || cn.is_empty() { return None; }
        return Some(PathBuf::from(out_dir).join(format!("{}{}.d", cn, find_extra_filename(args))));
    }
    None
}

/// Split the RHS of a makefile dep line on unescaped spaces (rustc escapes embedded spaces as `\ `).
fn split_dep_paths(rhs: &str) -> Vec<String> {
    let mut v = Vec::new();
    let mut cur = String::new();
    let mut chars = rhs.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&' ') => { cur.push(' '); chars.next(); }
            ' ' => { if !cur.is_empty() { v.push(std::mem::take(&mut cur)); } }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() { v.push(cur); }
    v
}

/// Cache T3-fix: validated dep-info restore. The stored .d's TARGET side embeds the PRODUCING
/// build's out-dir (volatile) and its DEP side lists the source files the unit was compiled from.
/// Parse the blob; REQUIRE the current positional .rs source among the deps and every dep path to
/// exist on this filesystem (a cross-workspace path mismatch is a MISS, never a guess — the exact
/// hazard the original T3 removal was about); then rewrite the target side to THIS invocation's
/// out-dir and write atomically. Preserves `# env-dep:` lines verbatim (cargo fingerprints env-var
/// deps from them).
fn restore_dep_info(blob_dir: &Path, stored: &str, dest: &Path, rustc_args: &[String], out_dir: &Path) -> bool {
    let Ok(raw) = fs::read_to_string(blob_dir.join(stored)) else { return false };
    let src_file = rustc_args.iter()
        .find(|a| a.ends_with(".rs") && !a.starts_with("--"))
        .cloned().unwrap_or_default();
    let mut deps: Vec<String> = Vec::new();
    let mut env_lines: Vec<&str> = Vec::new();
    for line in raw.lines() {
        if line.starts_with('#') {
            if line.starts_with("# env-dep:") { env_lines.push(line); }
            continue;
        }
        let Some((_, rhs)) = line.split_once(": ") else { continue };
        for p in split_dep_paths(rhs) {
            if !deps.contains(&p) { deps.push(p); }
        }
    }
    if deps.is_empty() { return false; }
    if !src_file.is_empty() && !deps.iter().any(|d| d == &src_file) { return false; }
    for d in &deps {
        if !Path::new(d).exists() { return false; }
    }
    let cn = find_crate_name(rustc_args);
    let extra = find_extra_filename(rustc_args);
    let dep_list = deps.iter().map(|d| d.replace(' ', "\\ ")).collect::<Vec<_>>().join(" ");
    let mut out = String::new();
    for ext in CACHEABLE_EXTS {
        let f = out_dir.join(format!("lib{}{}.{}", cn, extra, ext));
        if f.exists() { out.push_str(&format!("{}: {}\n", f.display(), dep_list)); }
    }
    out.push_str(&format!("{}: {}\n", dest.display(), dep_list));
    out.push('\n');
    for d in &deps { out.push_str(&format!("{}:\n", d.replace(' ', "\\ "))); }
    if !env_lines.is_empty() {
        out.push('\n');
        for l in env_lines { out.push_str(l); out.push('\n'); }
    }
    let tmp = dest.with_extension(format!("d.tmp.{}", std::process::id()));
    if fs::write(&tmp, out).is_err() { let _ = fs::remove_file(&tmp); return false; }
    match fs::rename(&tmp, dest) {
        Ok(()) => true,
        Err(_) => { let _ = fs::remove_file(&tmp); false }
    }
}

/// Scan `out_dir` for files belonging to this crate's compilation and return
/// the matching file names (without the directory prefix). Recognised by
/// containing `crate_name` as a substring and having a known extension.
/// rustc generates names like `lib<crate>-<metadata-hash>.<ext>`; the hash
/// is deterministic for a given (args, source) so the SAME compilation
/// re-run will land on the same file name — which is exactly what makes
/// the side-blob cache work.
// SUPERSEDED by exact-filename collection in collect_outputs: this crate_name-substring scan grabbed
// EVERY metahash-variant of a crate in the shared out_dir, polluting entries (the 223-reject bug). Kept
// only for the unit test that documents the old behavior; not used in production.
#[allow(dead_code)]
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
/// v0.36: reports the copied bytes to flux-cache's running disk total so the
/// FLUX_CACHE_DISK_CAP eviction sees blob growth (blobs are the bulk of the
/// cache by bytes; the index entries alone are ~KiB).
fn copy_output_to_blob(out_dir: &Path, blob_dir: &Path, file_name: &str) -> Option<String> {
    let src = out_dir.join(file_name);
    let dst = blob_dir.join(file_name);
    fs::create_dir_all(blob_dir).ok()?;
    let bytes = fs::copy(&src, &dst).ok()?;
    flux_cache::add_external_bytes(bytes);
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
    // ATOMIC restore (the "unknown file type" / residual-ICE fix): copy to a per-process temp then
    // rename. fs::copy alone is NOT atomic — under cargo's parallelism two restores of the same output
    // path (a crate compiled in two feature-unifications shares one filename) or a restore racing a
    // concurrent rustc/linker read leave a HALF-written file -> `mold: unknown file type` / a torn
    // rmeta -> ICE. rename(2) within the same dir is atomic: a reader sees either the old complete file
    // or the new complete one, never a torn one.
    let tmp = out_dir.join(format!(".{}.tmp.{}", file_name, std::process::id()));
    if fs::copy(&src, &tmp).is_err() {
        let _ = fs::remove_file(&tmp);
        return false;
    }
    match fs::rename(&tmp, &dst) {
        Ok(()) => true,
        Err(_) => { let _ = fs::remove_file(&tmp); false }
    }
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
            // Collect the EXACT artifact(s) THIS invocation produced — lib<crate><extra-filename>.<ext>
            // — NOT a crate_name substring scan. The substring scan grabbed EVERY metahash-variant of a
            // crate present in the shared out_dir (we measured one entry's blobdir holding 5 distinct
            // libhashbrown-*.{rmeta,rlib} variants), so outputs[ext] held an ARBITRARY/wrong variant (or
            // no rlib) -> a later link-pass lookup hit an entry whose rlib was the wrong variant or
            // absent -> T2 reject. That was the bulk of the 223 wrongly-rejected hits. Exact-name
            // collection (crate-name + -C extra-filename) stores only this pass's own artifacts.
            let extra = find_extra_filename(&raw_args);
            for ext in ["rmeta", "rlib"] {
                let fname = format!("lib{}{}.{}", crate_name, extra, ext);
                if out_dir_path.join(&fname).exists() {
                    if let Some(stored) = copy_output_to_blob(&out_dir_path, &blob_dir, &fname) {
                        outputs.insert(ext.to_string(), stored);
                    }
                }
            }
            // Cache T3-fix: ALSO capture the dep-info (.d) rustc just wrote. Without it a later
            // cache HIT skips rustc, cargo never gets dep-info to encode into .fingerprint/
            // dep-lib-*, and the unit is dirty on every subsequent build forever. Restore is
            // validated + path-rewritten (restore_dep_info), so the original T3 stale-path
            // hazard stays fixed.
            if !outputs.is_empty() {
                if let Some(d_path) = dep_info_dest(&raw_args) {
                    if d_path.exists() {
                        if let (Some(dir), Some(name)) = (
                            d_path.parent(),
                            d_path.file_name().map(|n| n.to_string_lossy().to_string()),
                        ) {
                            if let Some(stored) = copy_output_to_blob(dir, &blob_dir, &name) {
                                outputs.insert("d".to_string(), stored);
                            }
                        }
                    }
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

    // T2c: record the content-hash of every --extern dep this unit was built against. A future restore
    // verifies the CURRENT deps still match these — so a dep recompiled with different bytes (same
    // deterministic filename, non-deterministic rmeta content) forces a recompile instead of an ICE.
    if !source_hash.is_empty() && !outputs.is_empty() {
        let mut lines = String::new();
        for (base, path) in extern_dep_paths(&raw_args) {
            if let Some(h) = flux_cache::hash_file(&path) {
                lines.push_str(&format!("{} {}\n", base, h));
            }
        }
        let side = closure_sidecar_for(&source_hash);
        if let Some(parent) = side.parent() { fs::create_dir_all(parent).ok(); }
        fs::write(&side, lines).ok();
    }

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
    // Cache T3-fix gate: when this invocation emits dep-info (every real cargo unit does), a hit
    // MUST also restore a .d — a restore without one leaves the fingerprint permanently incomplete
    // (dep-lib-* never written -> unit dirty on every future build; the sigil 216s "no-op" bug).
    // Entries stored before this fix carry no "d" blob: MISS once, real rustc re-stores complete.
    let dep_dest = dep_info_dest(rustc_args);
    if dep_dest.is_some() && !entry.outputs.contains_key("d") { return false; }
    // T2c closure-consistency guard (the partial-hit ICE fix): a restored crate's rmeta embeds
    // references to the EXACT deps it was compiled against. Filename-identity keys match on a dep's
    // stable filename, but its rmeta CONTENT can still differ (rustc is non-deterministic), so a dep
    // that was a cache MISS (recompiled fresh) leaves this crate's restored rmeta inconsistent ->
    // SIGBUS / "no resolution for an import". Verify every current --extern dep is byte-identical to
    // what the entry was cached against (recorded in the closure sidecar); any mismatch -> miss, so we
    // recompile THIS crate to match its actual deps. Makes a partial hit safe.
    //
    // The sidecar is REQUIRED whenever the unit has extern deps: restore is on by
    // default now, so the old skip-for-legacy-entries would restore pre-sidecar
    // entries UNVERIFIED — the historical SIGBUS / "no resolution for an import"
    // ICE class. An unverifiable closure is a MISS (one recompile, repopulates
    // with a sidecar), never a restore. Dep-less units (no --extern) are vacuously
    // consistent and need no sidecar.
    let externs = extern_dep_paths(rustc_args);
    if !externs.is_empty() {
        match std::fs::read_to_string(closure_sidecar_for(&entry.source_hash)) {
            Ok(recorded) => {
                let want: std::collections::HashMap<&str, &str> =
                    recorded.lines().filter_map(|l| l.split_once(' ')).collect();
                for (base, path) in externs {
                    match want.get(base.as_str()) {
                        Some(&expected) => {
                            if flux_cache::hash_file(&path).as_deref() != Some(expected) {
                                return false; // a dep changed since cache time -> recompile this crate
                            }
                        }
                        // A dep the sidecar never recorded is just as unverifiable
                        // as a missing sidecar: miss, don't guess.
                        None => return false,
                    }
                }
            }
            Err(_) => return false, // no sidecar + has deps -> unverifiable -> miss
        }
    }
    let out_dir = find_out_dir(rustc_args);
    if out_dir.is_empty() { return false; }
    let blob_dir = blob_dir_for(&entry.source_hash);
    if !blob_dir.exists() { return false; }
    let out_dir_path = PathBuf::from(&out_dir);
    for (ext, file_name) in &entry.outputs {
        if ext == "d" { continue; } // dep-info is validated + rewritten below, never raw-restored
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
    // Cache T3-fix: materialize the dep-info LAST, once the hit is otherwise definitive, so cargo
    // can encode a complete fingerprint (dep-lib-*) exactly as if rustc had run. Validation failure
    // (foreign paths, missing sources) is a MISS — real rustc then rewrites everything it touched.
    if let (Some(dest), Some(stored)) = (dep_dest, entry.outputs.get("d")) {
        if !restore_dep_info(&blob_dir, stored, &dest, rustc_args, &out_dir_path) { return false; }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    // Tests below mutate the process-global CWD (source paths are relative in some fixtures);
    // serialize them so cargo's parallel runner (what a `fluxc combo` invokes) can't race them.
    // Poison-tolerant.
    static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn cwd_guard() -> std::sync::MutexGuard<'static, ()> {
        CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// v0.36: blob/closure paths now root in `flux_cache::cache_dir()` — by
    /// default the user's REAL shared cache ($HOME/.flux/cache). Pin an
    /// isolated per-process dir before any cache-touching test so tests never
    /// write into (or evict) the real cache. OnceLock semantics: the first
    /// call wins for the whole test binary.
    fn iso() {
        let _ = flux_cache::set_cache_dir(
            std::env::temp_dir().join(format!("flux-driver-test-{}", std::process::id())),
        );
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
        iso();
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

        // CWD change kept for path-relativity realism; blobs land in the iso() cache dir now.
        let orig_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&workspace).unwrap();

        let args: Vec<String> = vec![
            "--crate-name".into(), "demo".into(),
            "-C".into(), "extra-filename=-deadbeef".into(),
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

    /// T3-fix: a dep-info-emitting unit round-trips its .d through the cache — collect stores it,
    /// apply validates + rewrites it into THIS out-dir so cargo can persist a complete fingerprint.
    #[test]
    fn dep_info_roundtrip() {
        iso();
        let _cwd = cwd_guard();
        let workspace = tmp_dir("depinfo");
        let out_dir = workspace.join("out");
        fs::create_dir_all(&out_dir).unwrap();
        let src_file = workspace.join("src").join("lib.rs");
        fs::create_dir_all(src_file.parent().unwrap()).unwrap();
        fs::write(&src_file, b"// depinfo").unwrap();
        let src_str = src_file.to_string_lossy().to_string();
        fs::write(out_dir.join("libdemo-d1d1d1.rmeta"), b"rm").unwrap();
        fs::write(out_dir.join("libdemo-d1d1d1.rlib"), b"rl").unwrap();
        // A producing-build .d: volatile target side, real dep side (+ an env-dep line).
        fs::write(
            out_dir.join("demo-d1d1d1.d"),
            format!("/OLD/out/libdemo-d1d1d1.rmeta: {src_str}\n\n{src_str}:\n\n# env-dep:CARGO_PKG_NAME=demo\n"),
        ).unwrap();

        let args: Vec<String> = vec![
            "--crate-name".into(), "demo".into(),
            "--emit=dep-info,metadata,link".into(),
            "-C".into(), "extra-filename=-d1d1d1".into(),
            "--out-dir".into(), out_dir.to_string_lossy().to_string(),
            src_str.clone(),
        ];
        let key = flux_cache::compute_hash(Some(&src_str), &args);
        let entry = collect_outputs(&args, &key);
        assert!(entry.outputs.contains_key("d"), "collect must capture the .d — got {:?}", entry.outputs);

        fs::remove_dir_all(&out_dir).unwrap();
        fs::create_dir_all(&out_dir).unwrap();
        assert!(apply_cached_outputs(&entry, &args), "hit with valid dep-info must apply");
        let restored = fs::read_to_string(out_dir.join("demo-d1d1d1.d")).unwrap();
        assert!(restored.contains(&src_str), "restored .d must list the real source dep");
        assert!(restored.contains(out_dir.to_string_lossy().as_ref()), "target side must be rewritten to THIS out-dir");
        assert!(!restored.contains("/OLD/out/"), "producing build's volatile out-dir must be gone");
        assert!(restored.contains("# env-dep:CARGO_PKG_NAME=demo"), "env-dep lines must survive verbatim");
        fs::remove_dir_all(&workspace).ok();
    }

    /// T3-fix healing path: a pre-fix entry (no "d" blob) served to a dep-info-emitting invocation
    /// is a MISS — real rustc then re-stores the entry complete. Without this gate the hit would
    /// re-poison the fingerprint (the sigil 216s no-op bug).
    #[test]
    fn legacy_entry_without_dep_info_is_miss() {
        iso();
        let _cwd = cwd_guard();
        let workspace = tmp_dir("legacy-d");
        let out_dir = workspace.join("out");
        fs::create_dir_all(&out_dir).unwrap();
        let src_file = workspace.join("src").join("lib.rs");
        fs::create_dir_all(src_file.parent().unwrap()).unwrap();
        fs::write(&src_file, b"// legacy").unwrap();
        let src_str = src_file.to_string_lossy().to_string();
        fs::write(out_dir.join("libdemo-e2e2e2.rmeta"), b"rm").unwrap();
        fs::write(out_dir.join("libdemo-e2e2e2.rlib"), b"rl").unwrap();
        // NO .d in out_dir → collect stores a legacy-shaped entry without a "d" blob.
        let args: Vec<String> = vec![
            "--crate-name".into(), "demo".into(),
            "--emit=dep-info,metadata,link".into(),
            "-C".into(), "extra-filename=-e2e2e2".into(),
            "--out-dir".into(), out_dir.to_string_lossy().to_string(),
            src_str.clone(),
        ];
        let key = flux_cache::compute_hash(Some(&src_str), &args);
        let entry = collect_outputs(&args, &key);
        assert!(!entry.outputs.contains_key("d"));
        assert!(!apply_cached_outputs(&entry, &args), "dep-info-emitting hit without a .d blob must MISS");
        fs::remove_dir_all(&workspace).ok();
    }

    /// T3-fix fail-closed: a stored .d referencing paths that don't exist here (cross-workspace /
    /// deleted sources) is unverifiable — MISS, never a guess. This is the exact hazard the
    /// original T3 removal was guarding against, kept fixed.
    #[test]
    fn dep_info_foreign_paths_is_miss() {
        iso();
        let _cwd = cwd_guard();
        let workspace = tmp_dir("foreign-d");
        let out_dir = workspace.join("out");
        fs::create_dir_all(&out_dir).unwrap();
        let src_file = workspace.join("src").join("lib.rs");
        fs::create_dir_all(src_file.parent().unwrap()).unwrap();
        fs::write(&src_file, b"// foreign").unwrap();
        let src_str = src_file.to_string_lossy().to_string();
        fs::write(out_dir.join("libdemo-f3f3f3.rmeta"), b"rm").unwrap();
        fs::write(out_dir.join("libdemo-f3f3f3.rlib"), b"rl").unwrap();
        fs::write(
            out_dir.join("demo-f3f3f3.d"),
            format!("/OLD/out/libdemo-f3f3f3.rmeta: {src_str} /nonexistent-flux-test/gone.rs\n"),
        ).unwrap();
        let args: Vec<String> = vec![
            "--crate-name".into(), "demo".into(),
            "--emit=dep-info,metadata,link".into(),
            "-C".into(), "extra-filename=-f3f3f3".into(),
            "--out-dir".into(), out_dir.to_string_lossy().to_string(),
            src_str.clone(),
        ];
        let key = flux_cache::compute_hash(Some(&src_str), &args);
        let entry = collect_outputs(&args, &key);
        assert!(entry.outputs.contains_key("d"));
        fs::remove_dir_all(&out_dir).unwrap();
        fs::create_dir_all(&out_dir).unwrap();
        assert!(!apply_cached_outputs(&entry, &args), "unverifiable dep paths must MISS");
        fs::remove_dir_all(&workspace).ok();
    }

    #[test]
    fn stable_key_survives_volatile_args() {
        iso();
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
        // T2c-era update: cross-workspace reuse holds when the dep keeps its FILENAME
        // identity (lib<crate>-<metahash>) and byte-identical content at a different
        // absolute path. Two fake workspaces, same dep file in both.
        let ws1_deps = workspace.join("ws1").join("deps");
        let ws2_deps = workspace.join("ws2").join("deps");
        fs::create_dir_all(&ws1_deps).unwrap();
        fs::create_dir_all(&ws2_deps).unwrap();
        fs::write(ws1_deps.join("libserde-AAAA.rlib"), b"identical dep bytes").unwrap();
        fs::write(ws2_deps.join("libserde-AAAA.rlib"), b"identical dep bytes").unwrap();
        let orig_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&workspace).unwrap();

        let args1: Vec<String> = vec![
            "--crate-name".into(), "demo".into(),
            "-C".into(), "extra-filename=-deadbeef".into(),
            "--extern".into(), format!("serde={}", ws1_deps.join("libserde-AAAA.rlib").display()),
            "--out-dir".into(), out_dir.to_string_lossy().to_string(),
            src_file.to_string_lossy().to_string(),
        ];
        // Same logical compile, DIFFERENT absolute --extern path (a second workspace),
        // same dep identity + content — the closure sidecar verifies and passes.
        let args2: Vec<String> = vec![
            "--crate-name".into(), "demo".into(),
            "-C".into(), "extra-filename=-deadbeef".into(),
            "--extern".into(), format!("serde={}", ws2_deps.join("libserde-AAAA.rlib").display()),
            "--out-dir".into(), out_dir.to_string_lossy().to_string(),
            src_file.to_string_lossy().to_string(),
        ];
        const STABLE: &str = "stable-cache-key-deadbeef";
        let entry = collect_outputs(&args1, STABLE);
        assert_eq!(entry.source_hash, STABLE);
        assert!(blob_dir_for(STABLE).exists(), "blobs must live under the stable key");

        fs::remove_dir_all(&out_dir).unwrap();
        fs::create_dir_all(&out_dir).unwrap();
        // apply with args2 (different abs paths, same dep identity+bytes) — succeeds
        // because the blob dir is keyed on entry.source_hash == STABLE and the T2c
        // closure sidecar verifies the ws2 dep is byte-identical to what was recorded.
        assert!(apply_cached_outputs(&entry, &args2),
            "stable-key hit must restore when volatile args move but dep identity+content match");
        assert_eq!(fs::read(out_dir.join("libdemo-deadbeef.rmeta")).unwrap(), b"meta");

        std::env::set_current_dir(&orig_cwd).unwrap();
        fs::remove_dir_all(&workspace).ok();
    }

    #[test]
    fn apply_returns_false_when_blob_dir_missing() {
        iso();
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
        iso();
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

    #[test]
    fn t2c_extern_dep_paths_and_sidecar() {
        iso();
        let deps = extern_dep_paths(&[
            "--extern".into(), "libc=/x/deps/liblibc-361b.rmeta".into(),
            "--extern".into(), "noeq".into(), // malformed (no '='), skipped
            "--extern".into(), "http=/y/deps/libhttp-cc42.rlib".into(),
        ]);
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0], ("liblibc-361b.rmeta".to_string(), "/x/deps/liblibc-361b.rmeta".to_string()));
        assert_eq!(deps[1].0, "libhttp-cc42.rlib");
        assert!(closure_sidecar_for("abc123").ends_with("closure/abc123"));
    }
}
