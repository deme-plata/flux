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

/// Write one node record + one run stamp. Best-effort by contract.
pub fn record_unit(unit_kind: u8, unit_id: &str, record: &NodeRecord) {
    if !enabled() {
        return;
    }
    let Some(db) = open_db() else { return };
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
pub fn record_invocation(cmd: &str, package: Option<&str>, release: bool, green: bool, wall_ms: u64) {
    if !enabled() {
        return;
    }
    let pkg = package.unwrap_or("<workspace>");
    let profile = if release { "release" } else { "debug" };
    let unit_kind = if cmd == "test" { UNIT_KIND_TEST } else { UNIT_KIND_COMPILE };
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
        rustc_spawns: 0,
        created_unix: now_unix(),
        agent: agent_name(),
    };
    record_unit(unit_kind, &unit_id, &record);
}

#[cfg(test)]
mod tests {
    use super::*;

    // FLUX_TDG_DIR is process-global; cargo runs tests in parallel threads. Without
    // this lock one test's env teardown mid-another's run makes the tracer fall back
    // to the REAL shared TDG (observed: a live run_cargo breadcrumb bleeding into an
    // assertion). Serialize all env-touching tests, one unique dir each.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    fn disabled_env_writes_nothing() {
        with_tmp_tdg("disabled", || {
            std::env::set_var("FLUX_TDG", "0");
            record_invocation("build", Some("x"), false, true, 1);
            std::env::remove_var("FLUX_TDG");
            assert!(!tdg_dir().exists(), "FLUX_TDG=0 must not even create the db dir");
        });
    }
}
