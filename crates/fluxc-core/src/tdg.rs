// FIP-0003 phase 1 — the write-only TDG tracer.
//
// Records one node + one run stamp per fluxc build-family invocation into a
// flux-db at `<flux_cache::cache_dir()>/tdg`, using the FIP-0003 data model
// (`n/<kind>/<unit_id>` nodes, `r/<unix_ms>/<unit_id>` TTL'd run history).
// The read/query side — incremental combo scheduling over the graph — is the
// v0.37 implementation FIP; this tracer exists so real build/run data starts
// accumulating NOW, giving that work a populated graph to query on day one.
//
// Contract (FIP-0003 "Consistency & failure semantics"): the TDG is advisory,
// NEVER load-bearing. Every failure path here is swallowed — a missing,
// locked, or corrupt TDG must never break or slow a build. `FLUX_TDG=0`
// disables entirely; `FLUX_TDG_TRACE=1` prints what would otherwise be silent.
//
// Phase-1 identity honesty: invocation-level nodes cannot carry the wrapper's
// per-unit cache_key yet (that wiring is write-path #1 in the FIP and lands
// with the wrapper hook). Their `source_key` is the BLAKE3 of the invocation
// shape (cmd|package|profile), explicitly marked degraded via
// `identity_degraded: true` so the query side can distinguish real
// key-equality invalidation from phase-1 breadcrumbs.

use serde::{Deserialize, Serialize};

pub const UNIT_KIND_COMPILE: u8 = 0;
pub const UNIT_KIND_TEST: u8 = 1;
pub const UNIT_KIND_COMBO: u8 = 2;
pub const UNIT_KIND_SWARM: u8 = 3;

const RUN_TTL_SECONDS: u64 = 30 * 24 * 3600; // FIP-0003: runs CF is 30-day TTL'd

/// FIP-0002-aligned invalidation anchor. A node is valid iff every component
/// re-derives identically — key equality only, no timestamps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityKey {
    pub source_key: String,
    pub dep_idents: Vec<String>,
    pub closure_hash: String,
    pub rustc_version: String,
    pub ir_version: u32,
    /// Phase 1 marker: true when source_key is the invocation-shape hash, not
    /// the wrapper's per-unit cache_key. Query-side must not treat degraded
    /// identities as reuse-proof.
    #[serde(default)]
    pub identity_degraded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Outcome {
    Green,
    Red,
    Skipped { reused_from: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeRecord {
    pub unit_kind: u8,
    pub display: String,
    pub identity: IdentityKey,
    pub outcome: Outcome,
    pub wall_ms: u64,
    pub rustc_spawns: u32,
    pub created_unix: u64,
    pub agent: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunStamp {
    pub outcome: Outcome,
    pub wall_ms: u64,
    pub agent: String,
}

fn trace(msg: &str) {
    if std::env::var("FLUX_TDG_TRACE").map(|v| v == "1").unwrap_or(false) {
        eprintln!("[tdg] {}", msg);
    }
}

fn enabled() -> bool {
    !std::env::var("FLUX_TDG").map(|v| v == "0").unwrap_or(false)
}

/// TDG database location: shared cache dir (survives `rm -rf target`), same
/// policy family as the artifact cache. `FLUX_TDG_DIR` overrides (tests).
pub fn tdg_dir() -> std::path::PathBuf {
    std::env::var("FLUX_TDG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| flux_cache::cache_dir().join("tdg"))
}

fn open_db() -> Option<flux_db::Database> {
    match flux_db::Database::open(tdg_dir()) {
        Ok(db) => Some(db),
        Err(e) => {
            trace(&format!("open failed (advisory, continuing without TDG): {}", e));
            None
        }
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn agent_name() -> String {
    std::env::var("FLUX_AGENT")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "unknown".to_string())
}

fn write_unit(db: &flux_db::Database, unit_kind: u8, unit_id: &str, record: &NodeRecord) {
    let node_key = format!("n/{}/{}", unit_kind, unit_id);
    let Ok(node_val) = bincode::serialize(record) else {
        trace("node serialize failed");
        return;
    };
    let stamp = RunStamp {
        outcome: record.outcome.clone(),
        wall_ms: record.wall_ms,
        agent: record.agent.clone(),
    };
    let run_key = format!("r/{}/{}", now_unix() * 1000, unit_id);
    let Ok(run_val) = bincode::serialize(&stamp) else {
        trace("run serialize failed");
        return;
    };
    if let Err(e) = db.put(node_key.as_bytes(), &node_val) {
        trace(&format!("node put failed: {}", e));
        return;
    }
    if let Err(e) = db.put_ttl_seconds(run_key.as_bytes(), &run_val, RUN_TTL_SECONDS) {
        trace(&format!("run put failed: {}", e));
        return;
    }
    trace(&format!("recorded {} ({} ms, {:?})", node_key, record.wall_ms, record.outcome));
}

/// Write one node record + one run stamp. Best-effort by contract.
pub fn record_unit(unit_kind: u8, unit_id: &str, record: &NodeRecord) {
    if !enabled() {
        return;
    }
    let Some(db) = open_db() else { return };
    write_unit(&db, unit_kind, unit_id, record);
}

// ── FIP-0003 write-path #1: the wrapper hook (spool + parent ingest) ────────
//
// The wrapper runs as MANY short-lived parallel processes (one per rustc
// spawn). flux-db is a single-writer store — concurrent opens from 16 wrapper
// processes would contend or corrupt. So the wrapper NEVER touches flux-db:
// it appends one tiny JSON file per compiled unit to `<tdg>/spool/` (atomic
// tmp+rename, ~µs — the 12.4s warm-build invariant is untouched), and the
// PARENT fluxc invocation drains the spool into flux-db in one batch after
// cargo exits, inside the same open it already uses for its own breadcrumb.
// Crash-safe by construction: an unread spool survives and the next
// invocation ingests it.

fn spool_path_dir() -> std::path::PathBuf {
    tdg_dir().join("spool")
}

/// Wrapper-side writer. `outcome`: "green" (real rustc exec), "skipped"
/// (cache restore), "red" (rustc failed). `dep_idents` are the FIP-0002
/// deterministic dep identities: the `lib<crate>-<metahash>.<ext>` filenames
/// from `--extern` args. `pkg` is cargo's CARGO_PKG_NAME for the unit (maps
/// integration-test units — whose --crate-name is the test FILE — back to
/// their package); `is_test` marks `--test` harness units, the gate the
/// unit-granularity scheduler keys on.
pub fn spool_wrapper_unit(
    cache_key: &str,
    crate_name: &str,
    pkg: &str,
    is_test: bool,
    dep_idents: &[String],
    outcome: &str,
    wall_ms: u64,
) {
    if !enabled() || cache_key.is_empty() {
        return;
    }
    let dir = spool_path_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let rec = serde_json::json!({
        "cache_key": cache_key,
        "crate_name": crate_name,
        "pkg": pkg,
        "is_test": is_test,
        "dep_idents": dep_idents,
        "outcome": outcome,
        "wall_ms": wall_ms,
        "ts": now_unix(),
    });
    let base = format!("{}-{}", &cache_key[..16.min(cache_key.len())], std::process::id());
    let tmp = dir.join(format!("{}.tmp", base));
    let fin = dir.join(format!("{}.json", base));
    if std::fs::write(&tmp, rec.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &fin);
    }
}

/// One spooled wrapper unit, as read back by the scheduler's probe pass.
#[derive(Debug, Clone)]
pub struct SpoolUnit {
    pub cache_key: String,
    /// rustc --crate-name: the STABLE identity of a unit across content
    /// changes (a package's lib test vs each integration-test file).
    pub crate_name: String,
    pub pkg: String,
    pub is_test: bool,
}

/// NON-draining spool read for the unit-granularity scheduler: after a probe
/// build (`cargo test --no-run` through the wrapper), this is the set of
/// units cargo touched, with their content-addressed keys. Files stay in
/// place — the next run_cargo ingest drains them into the graph as usual.
pub fn read_spool_units() -> Vec<SpoolUnit> {
    let dir = spool_path_dir();
    let Ok(rd) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut out = Vec::new();
    for entry in rd.flatten() {
        let p = entry.path();
        if p.extension().map(|x| x != "json").unwrap_or(true) {
            continue;
        }
        let Ok(txt) = std::fs::read_to_string(&p) else { continue };
        let Ok(j) = serde_json::from_str::<serde_json::Value>(&txt) else { continue };
        let cache_key = j["cache_key"].as_str().unwrap_or("").to_string();
        if cache_key.is_empty() {
            continue;
        }
        out.push(SpoolUnit {
            cache_key,
            crate_name: j["crate_name"].as_str().unwrap_or("").to_string(),
            pkg: j["pkg"].as_str().unwrap_or("").to_string(),
            is_test: j["is_test"].as_bool().unwrap_or(false),
        });
    }
    out
}

// ── per-package "last BUILT test-unit keys" (`u/<pkg>`) ────────────────────
//
// The wrapper only spawns for units cargo actually rebuilds, so any single
// invocation observes a PARTIAL set of a package's test units. The `u/<pkg>`
// map (unit crate_name → cache_key) is MERGED at every ingest: rebuilt units
// update their entry, cargo-fresh units keep their last recorded key — which
// is sound, because cargo-fresh means that binary hasn't changed since the
// build that recorded it. The unit-granularity gate compares a hash of this
// full map ("last built") against the hash promoted at the last green test
// run ("last green"): equality proves the package's test binaries are the
// ones that already passed.

pub fn unit_map_key(pkg: &str) -> String {
    format!("u/{}", pkg)
}

pub fn read_unit_map(db: &flux_db::Database, pkg: &str) -> std::collections::BTreeMap<String, String> {
    db.get(unit_map_key(pkg).as_bytes())
        .ok()
        .flatten()
        .and_then(|v| bincode::deserialize(&v).ok())
        .unwrap_or_default()
}

/// Order-independent digest of a package's full test-unit key map.
pub fn hash_unit_map(map: &std::collections::BTreeMap<String, String>) -> String {
    let mut h = blake3::Hasher::new();
    for (name, key) in map {
        h.update(name.as_bytes());
        h.update(b"=");
        h.update(key.as_bytes());
        h.update(b"\n");
    }
    h.finalize().to_hex().to_string()
}

fn merge_unit_maps(db: &flux_db::Database, observed: &[(String, String, String)]) {
    // observed: (pkg, unit crate_name, cache_key) for --test units
    let mut by_pkg: std::collections::BTreeMap<String, Vec<(String, String)>> = Default::default();
    for (pkg, name, key) in observed {
        if !pkg.is_empty() && !name.is_empty() {
            by_pkg.entry(pkg.clone()).or_default().push((name.clone(), key.clone()));
        }
    }
    for (pkg, units) in by_pkg {
        let mut map = read_unit_map(db, &pkg);
        for (name, key) in units {
            map.insert(name, key);
        }
        if let Ok(v) = bincode::serialize(&map) {
            let _ = db.put(unit_map_key(&pkg).as_bytes(), &v);
        }
    }
}

/// Extract FIP-0002 dep identities from raw rustc args: the filename of every
/// `--extern name=path` target (`lib<crate>-<metahash>.rlib` — deterministic,
/// content-independent).
pub fn dep_idents_from_rustc_args(rustc_args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < rustc_args.len() {
        if rustc_args[i] == "--extern" {
            if let Some(spec) = rustc_args.get(i + 1) {
                if let Some((_, path)) = spec.split_once('=') {
                    if let Some(name) = std::path::Path::new(path).file_name() {
                        out.push(name.to_string_lossy().to_string());
                    }
                }
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    out
}

/// Parent-side: drain the wrapper spool into flux-db in one batch. Returns the
/// ingested unit ids so the calling invocation can edge itself to them.
pub(crate) fn ingest_spool(db: &flux_db::Database) -> Vec<String> {
    let dir = spool_path_dir();
    let Ok(rd) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut unit_ids: Vec<String> = Vec::new();
    let mut nodes: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut edges: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut runs: Vec<(String, Vec<u8>)> = Vec::new();
    let mut drained: Vec<std::path::PathBuf> = Vec::new();
    let mut test_units: Vec<(String, String, String)> = Vec::new(); // (pkg, unit, key)
    for entry in rd.flatten() {
        let p = entry.path();
        if p.extension().map(|x| x != "json").unwrap_or(true) {
            continue;
        }
        let Ok(txt) = std::fs::read_to_string(&p) else { continue };
        let Ok(j) = serde_json::from_str::<serde_json::Value>(&txt) else {
            let _ = std::fs::remove_file(&p); // undecodable spool = drop, not wedge
            continue;
        };
        let key = j["cache_key"].as_str().unwrap_or("").to_string();
        if key.is_empty() {
            let _ = std::fs::remove_file(&p);
            continue;
        }
        let deps: Vec<String> = j["dep_idents"].as_array().map(|a| {
            a.iter().filter_map(|d| d.as_str().map(String::from)).collect()
        }).unwrap_or_default();
        let outcome = match j["outcome"].as_str() {
            Some("green") => Outcome::Green,
            Some("skipped") => Outcome::Skipped { reused_from: key.clone() },
            _ => Outcome::Red,
        };
        let wall_ms = j["wall_ms"].as_u64().unwrap_or(0);
        let record = NodeRecord {
            unit_kind: UNIT_KIND_COMPILE,
            display: format!("rustc {}", j["crate_name"].as_str().unwrap_or("?")),
            identity: IdentityKey {
                source_key: key.clone(),
                dep_idents: deps.clone(),
                closure_hash: String::new(),
                rustc_version: flux_driver::RUSTC_VERSION.to_string(),
                ir_version: flux_frontend::IR_VERSION,
                identity_degraded: false, // the REAL wrapper cache_key
            },
            outcome: outcome.clone(),
            wall_ms,
            rustc_spawns: matches!(outcome, Outcome::Green | Outcome::Red) as u32,
            created_unix: j["ts"].as_u64().unwrap_or_else(now_unix),
            agent: agent_name(),
        };
        if let Ok(v) = bincode::serialize(&record) {
            nodes.push((format!("n/{}/{}", UNIT_KIND_COMPILE, key).into_bytes(), v));
        }
        for d in &deps {
            edges.push((format!("e/f/{}/{}", key, d).into_bytes(), vec![1u8]));
            edges.push((format!("e/r/{}/{}", d, key).into_bytes(), vec![1u8]));
        }
        if let Ok(v) = bincode::serialize(&RunStamp {
            outcome, wall_ms, agent: record.agent.clone(),
        }) {
            runs.push((format!("r/{}/{}", now_unix() * 1000, key), v));
        }
        if j["is_test"].as_bool().unwrap_or(false) {
            test_units.push((
                j["pkg"].as_str().unwrap_or("").to_string(),
                j["crate_name"].as_str().unwrap_or("").to_string(),
                key.clone(),
            ));
        }
        unit_ids.push(key);
        drained.push(p);
    }
    if !test_units.is_empty() {
        merge_unit_maps(db, &test_units);
    }
    if !nodes.is_empty() {
        if let Err(e) = db.put_many(&nodes) {
            trace(&format!("spool node batch failed: {}", e));
            return Vec::new(); // keep spool files — retry next invocation
        }
        if let Err(e) = db.put_many(&edges) {
            trace(&format!("spool edge batch failed: {}", e));
        }
        for (k, v) in &runs {
            let _ = db.put_ttl_seconds(k.as_bytes(), v, RUN_TTL_SECONDS);
        }
    }
    for p in drained {
        let _ = std::fs::remove_file(&p);
    }
    if !unit_ids.is_empty() {
        trace(&format!("ingested {} spooled compile units", unit_ids.len()));
    }
    unit_ids
}

/// FIP-0003 edges: both directions in one batch (`e/f/` forward, `e/r/` reverse
/// index) so "who must re-run if <id> changed?" is a single ordered prefix walk.
pub fn record_edges(from_id: &str, to_ids: &[String]) {
    if !enabled() || to_ids.is_empty() {
        return;
    }
    let Some(db) = open_db() else { return };
    // FIP-0003 writes edges as `value = ""`, but in flux-db an EMPTY VALUE IS A
    // TOMBSTONE (delete marker) — an empty-valued put stores nothing. One
    // presence byte keeps the edge alive; readers key off the key prefix only.
    let mut entries: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(to_ids.len() * 2);
    for to in to_ids {
        entries.push((format!("e/f/{}/{}", from_id, to).into_bytes(), vec![1u8]));
        entries.push((format!("e/r/{}/{}", to, from_id).into_bytes(), vec![1u8]));
    }
    if let Err(e) = db.put_many(&entries) {
        trace(&format!("edge batch failed: {}", e));
    }
}

/// The phase-1 convenience writer hooked into `run_cargo`: one node per fluxc
/// build-family invocation. `cmd` is the cargo subcommand ("build", "test", …).
/// Also drains the wrapper spool (write-path #1) in the same db open, and
/// edges this invocation to every compile unit it ingested — the combo→unit
/// containment relation the incremental scheduler will walk.
pub fn record_invocation(cmd: &str, package: Option<&str>, release: bool, green: bool, wall_ms: u64) {
    if !enabled() {
        return;
    }
    let Some(db) = open_db() else { return };
    let unit_ids = ingest_spool(&db);
    let pkg = package.unwrap_or("<workspace>");
    let profile = if release { "release" } else { "debug" };
    let unit_kind = if cmd == "test" { UNIT_KIND_TEST } else { UNIT_KIND_COMBO };
    let unit_id = blake3::hash(format!("{}|{}|{}", cmd, pkg, profile).as_bytes())
        .to_hex()
        .to_string();
    let record = NodeRecord {
        unit_kind,
        display: format!("{} {} ({})", cmd, pkg, profile),
        identity: IdentityKey {
            source_key: unit_id.clone(),
            dep_idents: vec![],
            closure_hash: String::new(),
            rustc_version: flux_driver::RUSTC_VERSION.to_string(),
            ir_version: flux_frontend::IR_VERSION,
            identity_degraded: true,
        },
        outcome: if green { Outcome::Green } else { Outcome::Red },
        wall_ms,
        rustc_spawns: unit_ids.len() as u32,
        created_unix: now_unix(),
        agent: agent_name(),
    };
    write_unit(&db, unit_kind, &unit_id, &record);
    if !unit_ids.is_empty() {
        let mut entries: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(unit_ids.len() * 2);
        for u in &unit_ids {
            entries.push((format!("e/f/{}/{}", unit_id, u).into_bytes(), vec![1u8]));
            entries.push((format!("e/r/{}/{}", u, unit_id).into_bytes(), vec![1u8]));
        }
        if let Err(e) = db.put_many(&entries) {
            trace(&format!("invocation edge batch failed: {}", e));
        }
    }
}

fn kind_name(k: u8) -> &'static str {
    match k {
        UNIT_KIND_COMPILE => "compile",
        UNIT_KIND_TEST => "test",
        UNIT_KIND_COMBO => "combo",
        UNIT_KIND_SWARM => "swarm",
        _ => "?",
    }
}

/// `fluxc tdg` — the phase-1 read side: what has the tracer recorded?
/// Prints node counts by kind, edge count, and the most recent runs. This is
/// deliberately a REPORT, not the incremental scheduler (that query engine is
/// the v0.37 implementation FIP) — it exists so the accumulating graph is
/// visible to operators and agents from day one.
pub fn run_tdg_report(limit: usize) {
    println!("⚡ fluxc tdg — task-dependency graph (FIP-0003 phase 1, write-only tracer)");
    println!("  db: {}", tdg_dir().display());
    if !enabled() {
        println!("  (FLUX_TDG=0 — tracing disabled)");
        return;
    }
    let Some(db) = open_db() else {
        println!("  (no TDG yet — it appears after the first fluxc build/check/test)");
        return;
    };

    let mut by_kind: std::collections::BTreeMap<u8, u64> = std::collections::BTreeMap::new();
    let mut green: u64 = 0;
    let mut red: u64 = 0;
    for (k, v) in db.iter_from(b"n/") {
        if !k.starts_with(b"n/") { break; }
        if let Ok(rec) = bincode::deserialize::<NodeRecord>(&v) {
            *by_kind.entry(rec.unit_kind).or_insert(0) += 1;
            match rec.outcome {
                Outcome::Green => green += 1,
                Outcome::Red => red += 1,
                Outcome::Skipped { .. } => {}
            }
        }
    }
    let mut edges: u64 = 0;
    for (k, _) in db.iter_from(b"e/") {
        if !k.starts_with(b"e/f/") { break; } // forward direction only — reverse mirrors it
        edges += 1;
    }
    let total: u64 = by_kind.values().sum();
    print!("  nodes: {} (", total);
    let parts: Vec<String> = by_kind.iter()
        .map(|(k, n)| format!("{} {}", n, kind_name(*k))).collect();
    println!("{}) · {} green / {} red · {} edges", parts.join(", "), green, red, edges);

    // Recent runs: r/<unix_ms>/<unit_id>, ordered — walk from the back.
    let now = flux_db::ttl::now_unix();
    let runs: Vec<(Vec<u8>, Vec<u8>)> = db.iter_from(b"r/")
        .take_while(|(k, _)| k.starts_with(b"r/"))
        .collect();
    println!("  recent runs (last {} of {}):", limit.min(runs.len()), runs.len());
    for (k, v) in runs.iter().rev().take(limit) {
        let key = String::from_utf8_lossy(k);
        let mut it = key.splitn(3, '/');
        let (_, ms, unit) = (it.next(), it.next().unwrap_or("?"), it.next().unwrap_or("?"));
        let when = ms.parse::<u64>().map(|m| {
            let ago = now.saturating_sub(m / 1000);
            if ago < 120 { format!("{}s ago", ago) }
            else if ago < 7200 { format!("{}m ago", ago / 60) }
            else { format!("{}h ago", ago / 3600) }
        }).unwrap_or_else(|_| "?".to_string());
        match flux_db::ttl::unwrap(v, now)
            .and_then(|live| bincode::deserialize::<RunStamp>(&live).ok()) {
            Some(stamp) => {
                let mark = match stamp.outcome {
                    Outcome::Green => "✓",
                    Outcome::Red => "✗",
                    Outcome::Skipped { .. } => "⏭",
                };
                // Look the display name up from the node record (any kind).
                let display = (0u8..=3).find_map(|kind| {
                    db.get(format!("n/{}/{}", kind, unit).as_bytes()).ok().flatten()
                        .and_then(|nv| bincode::deserialize::<NodeRecord>(&nv).ok())
                        .map(|r| r.display)
                }).unwrap_or_else(|| format!("{}…", &unit[..unit.len().min(16)]));
                println!("    {} {:<40} {:>8}ms  {:>9}  {}", mark, display, stamp.wall_ms, when, stamp.agent);
            }
            None => println!("    (expired/undecodable run {})", when),
        }
    }
}

// FLUX_TDG_DIR is process-global; cargo runs tests in parallel threads. Without
// this lock one test's env teardown mid-another's run makes the tracer fall back
// to the REAL shared TDG (observed: a live run_cargo breadcrumb bleeding into an
// assertion). Serialize ALL env-touching tests — tdg_sched's too — one dir each.
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    use super::TEST_ENV_LOCK as ENV_LOCK;

    fn with_tmp_tdg<F: FnOnce()>(name: &str, f: F) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir()
            .join(format!("flux-tdg-test-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("FLUX_TDG_DIR", &dir);
        f();
        std::env::remove_var("FLUX_TDG_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tracer_roundtrips_node_and_run_records() {
        with_tmp_tdg("roundtrip", || {
            record_invocation("build", Some("flux-db"), false, true, 1234);
            let db = flux_db::Database::open(tdg_dir()).expect("tdg opens");
            // Node record decodes and carries the phase-1 degraded-identity marker.
            let mut nodes = 0;
            let mut it = db.iter_from(b"n/");
            while let Some((k, v)) = it.next() {
                if !k.starts_with(b"n/") {
                    break;
                }
                let rec: NodeRecord = bincode::deserialize(&v).expect("node decodes");
                assert!(matches!(rec.outcome, Outcome::Green));
                assert_eq!(rec.wall_ms, 1234);
                assert!(rec.identity.identity_degraded, "phase-1 nodes must self-mark degraded");
                assert_eq!(rec.identity.rustc_version, flux_driver::RUSTC_VERSION);
                nodes += 1;
            }
            assert_eq!(nodes, 1, "exactly one node record");
            // Run stamp landed under the r/ prefix. TTL'd values are envelope-
            // wrapped by flux-db — readers must ttl::unwrap before decoding.
            let mut runs = 0;
            let mut it = db.iter_from(b"r/");
            while let Some((k, v)) = it.next() {
                if !k.starts_with(b"r/") {
                    break;
                }
                let live = flux_db::ttl::unwrap(&v, flux_db::ttl::now_unix())
                    .expect("run stamp must not be expired");
                let stamp: RunStamp = bincode::deserialize(&live).expect("run decodes");
                assert_eq!(stamp.wall_ms, 1234);
                runs += 1;
            }
            assert_eq!(runs, 1, "exactly one run stamp");
        });
    }

    #[test]
    fn edges_materialize_both_directions() {
        with_tmp_tdg("edges", || {
            record_edges("combo-x", &["unit-a".to_string(), "unit-b".to_string()]);
            let db = flux_db::Database::open(tdg_dir()).expect("tdg opens");
            for key in ["e/f/combo-x/unit-a", "e/r/unit-a/combo-x", "e/f/combo-x/unit-b", "e/r/unit-b/combo-x"] {
                assert!(db.get(key.as_bytes()).expect("get ok").is_some(), "missing edge {}", key);
            }
        });
    }

    #[test]
    fn wrapper_spool_ingests_into_graph() {
        with_tmp_tdg("spool", || {
            // Two wrapper units: a real compile with deps, and a cache restore.
            spool_wrapper_unit("aaaa1111bbbb2222cccc", "flux_db", "flux-db", false,
                &["libserde-abc123.rlib".to_string()], "green", 900);
            spool_wrapper_unit("dddd3333eeee4444ffff", "flux_cache", "flux-cache", true,
                &[], "skipped", 2);
            assert_eq!(std::fs::read_dir(spool_path_dir()).unwrap().count(), 2);

            // The probe reader sees both units, non-draining, with pkg + test markers.
            let units = read_spool_units();
            assert_eq!(units.len(), 2);
            let t = units.iter().find(|u| u.pkg == "flux-cache").expect("test unit present");
            assert!(t.is_test, "--test marker must round-trip through the spool");
            assert_eq!(std::fs::read_dir(spool_path_dir()).unwrap().count(), 2,
                "read_spool_units must not drain");

            // The parent invocation drains the spool and edges itself to the units.
            record_invocation("build", Some("flux-db"), false, true, 5000);

            let db = flux_db::Database::open(tdg_dir()).expect("tdg opens");
            // Spool is drained.
            assert_eq!(std::fs::read_dir(spool_path_dir()).map(|d| d.count()).unwrap_or(0), 0,
                "spool must be drained after ingest");
            // Compile units landed with REAL (non-degraded) identities.
            let n = db.get(b"n/0/aaaa1111bbbb2222cccc").expect("get ok")
                .expect("green unit node exists");
            let rec: NodeRecord = bincode::deserialize(&n).expect("node decodes");
            assert!(!rec.identity.identity_degraded, "wrapper units carry the real cache_key");
            assert_eq!(rec.identity.dep_idents, vec!["libserde-abc123.rlib".to_string()]);
            assert!(matches!(rec.outcome, Outcome::Green));
            let n2 = db.get(b"n/0/dddd3333eeee4444ffff").expect("get ok")
                .expect("skipped unit node exists");
            let rec2: NodeRecord = bincode::deserialize(&n2).expect("node decodes");
            assert!(matches!(rec2.outcome, Outcome::Skipped { .. }));
            // Dep edges, both directions.
            assert!(db.get(b"e/f/aaaa1111bbbb2222cccc/libserde-abc123.rlib").unwrap().is_some());
            assert!(db.get(b"e/r/libserde-abc123.rlib/aaaa1111bbbb2222cccc").unwrap().is_some());
            // Invocation → unit containment edges (reverse index present).
            let mut inv_edges = 0;
            for (k, _) in db.iter_from(b"e/r/aaaa1111bbbb2222cccc/") {
                if !k.starts_with(b"e/r/aaaa1111bbbb2222cccc/") { break; }
                inv_edges += 1;
            }
            assert_eq!(inv_edges, 1, "the invocation must edge to its ingested unit");
        });
    }

    #[test]
    fn dep_idents_extracts_extern_filenames() {
        let args: Vec<String> = ["--crate-name", "x", "--extern",
            "serde=/deep/path/libserde-abc.rlib", "--extern", "noeq", "-L", "dep=/x"]
            .iter().map(|s| s.to_string()).collect();
        assert_eq!(dep_idents_from_rustc_args(&args), vec!["libserde-abc.rlib".to_string()]);
    }

    #[test]
    fn disabled_env_writes_nothing() {
        with_tmp_tdg("disabled", || {
            std::env::set_var("FLUX_TDG", "0");
            record_invocation("build", Some("x"), false, true, 1);
            std::env::remove_var("FLUX_TDG");
            assert!(!tdg_dir().exists(), "FLUX_TDG=0 must not even create the db dir");
        });
    }
}
