//! MCP tool surface for flux-dj: a lightweight `ToolRegistry` (schema +
//! handler-fn pairs), the same pattern `fluxc-mcp` uses
//! (`crates/fluxc-mcp/src/handlers/mod.rs`) — reimplemented standalone here
//! (rather than depending on the `fluxc-mcp` crate, which pulls in ~40
//! unrelated path-deps) so `flux-dj-mcp` is a self-contained binary someone
//! can point a bare MCP client config at.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::camelot::{compatible_keys, Camelot};
use crate::tempo::{estimate_tempo_from_onsets, simulate_tempo_estimate, SimulateParams};
use crate::updater;
use crate::{get_history, log_track, NewTrackRequest};

pub type ToolFn = fn(&Value) -> String;

pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

pub struct ToolRegistry {
    tools: Vec<ToolDef>,
    handlers: HashMap<String, ToolFn>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        ToolRegistry { tools: Vec::new(), handlers: HashMap::new() }
    }

    pub fn register(&mut self, def: ToolDef, handler: ToolFn) {
        self.handlers.insert(def.name.to_string(), handler);
        self.tools.push(def);
    }

    pub fn tools_schema(&self) -> Vec<Value> {
        self.tools
            .iter()
            .map(|t| json!({"name": t.name, "description": t.description, "inputSchema": t.input_schema}))
            .collect()
    }

    pub fn execute(&self, name: &str, args: &Value) -> Option<String> {
        self.handlers.get(name).map(|h| h(args))
    }
}

pub fn build_registry() -> ToolRegistry {
    let mut r = ToolRegistry::new();

    r.register(
        ToolDef {
            name: "dj_log_track",
            description: "Log a track being played (now-playing). Persists to the flux-dj track history and, if FLUX_DJ_WEBHOOK_URL is set, fires an outbound webhook with the track JSON.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "artist": {"type": "string"},
                    "title": {"type": "string"},
                    "bpm": {"type": "number", "description": "Optional tempo in BPM"},
                    "key": {"type": "string", "description": "Optional Camelot key, e.g. '8A'"},
                    "timestamp": {"type": "string", "description": "Optional RFC3339 timestamp; defaults to now"}
                },
                "required": ["artist", "title"]
            }),
        },
        handle_log_track,
    );

    r.register(
        ToolDef {
            name: "dj_get_history",
            description: "Fetch recent logged tracks, newest first.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "limit": {"type": "integer", "description": "Max tracks to return (default 20)"}
                }
            }),
        },
        handle_get_history,
    );

    r.register(
        ToolDef {
            name: "dj_key_compatible",
            description: "Given a Camelot key (e.g. '8A'), return harmonically compatible keys under both the strict rule set (adjacent same-letter perfect-fifth + same-number relative major/minor) and the extended rule set (strict + same-letter ±2 'energy jump').",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "key": {"type": "string", "description": "Camelot key, e.g. '8A' or '12B'"}
                },
                "required": ["key"]
            }),
        },
        handle_key_compatible,
    );

    r.register(
        ToolDef {
            name: "dj_estimate_tempo",
            description: "Estimate tempo (BPM) via autocorrelation over an onset-strength envelope. Two input modes: (1) real path — supply 'onsets_sec' (a list of onset timestamps in seconds, e.g. from an onset detector), optional 'amplitudes'/'duration_sec'/'frame_hz'/'bpm_lo'/'bpm_hi'/'bpm_step'; (2) simulate path — supply 'simulate': {true_bpm, duration_s, frame_hz, jitter_ms, miss_prob, spurious_prob, noise_amp, seed} to Monte-Carlo synthesize a click envelope and recover its tempo (reproduces the ai_dj_live.rs paper's methodology for a single seeded trial).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "onsets_sec": {"type": "array", "items": {"type": "number"}, "description": "Onset timestamps in seconds (real path)"},
                    "amplitudes": {"type": "array", "items": {"type": "number"}, "description": "Optional per-onset amplitude, same length as onsets_sec"},
                    "duration_sec": {"type": "number", "description": "Optional envelope duration; defaults to max(onsets_sec)+2"},
                    "frame_hz": {"type": "number", "description": "Envelope frame rate (default 100.0)"},
                    "bpm_lo": {"type": "number", "description": "Search band low (default 60.0)"},
                    "bpm_hi": {"type": "number", "description": "Search band high (default 200.0)"},
                    "bpm_step": {"type": "number", "description": "Search step (default 0.5)"},
                    "simulate": {
                        "type": "object",
                        "description": "Monte-Carlo simulate path (alternative to onsets_sec)",
                        "properties": {
                            "true_bpm": {"type": "number"},
                            "duration_s": {"type": "number"},
                            "frame_hz": {"type": "number"},
                            "jitter_ms": {"type": "number"},
                            "miss_prob": {"type": "number"},
                            "spurious_prob": {"type": "number"},
                            "noise_amp": {"type": "number"},
                            "seed": {"type": "integer"}
                        },
                        "required": ["true_bpm"]
                    }
                }
            }),
        },
        handle_estimate_tempo,
    );

    r.register(
        ToolDef {
            name: "dj_check_for_update",
            description: "Check whether a newer flux-dj version is published, by asking a version-manifest endpoint (default: the co-located flux-dj-server's GET /api/v1/dj-version, override via 'update_server_url' or FLUX_DJ_UPDATE_SERVER_URL). Read-only: performs exactly one network GET and NEVER downloads, writes, or replaces anything on disk. Call this first; only call dj_apply_update after the user has explicitly confirmed they want the update installed.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "update_server_url": {"type": "string", "description": "Override the version-manifest server base URL (default from FLUX_DJ_UPDATE_SERVER_URL or http://127.0.0.1:4210)"}
                }
            }),
        },
        handle_check_for_update,
    );

    r.register(
        ToolDef {
            name: "dj_apply_update",
            description: "Download, checksum-verify, and atomically install a newer flux-dj version onto THIS running binary. DO NOT call this unless the user has explicitly confirmed they want the update applied — this is the confirmation-gated step; dj_check_for_update alone never touches anything. Re-checks the version manifest fresh (does not trust a stale prior check) and is a safe no-op if no newer version is currently published. On success the OLD process is still running the OLD code; the caller (the user, via their AI) must restart flux-dj-mcp for the new version to take effect — this tool intentionally never restarts itself, since exiting mid-call would break the MCP session.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "update_server_url": {"type": "string", "description": "Override the version-manifest server base URL (default from FLUX_DJ_UPDATE_SERVER_URL or http://127.0.0.1:4210)"}
                }
            }),
        },
        handle_apply_update,
    );

    r
}

fn err_json(msg: impl AsRef<str>) -> String {
    json!({"status": "error", "error": msg.as_ref()}).to_string()
}

fn handle_log_track(args: &Value) -> String {
    let req: NewTrackRequest = match serde_json::from_value(args.clone()) {
        Ok(r) => r,
        Err(e) => return err_json(format!("invalid arguments: {e}")),
    };
    match log_track(req) {
        Ok(track) => json!({"status": "ok", "track": track}).to_string(),
        Err(e) => err_json(format!("failed to log track: {e}")),
    }
}

fn handle_get_history(args: &Value) -> String {
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
    match get_history(limit) {
        Ok(tracks) => json!({"status": "ok", "count": tracks.len(), "tracks": tracks}).to_string(),
        Err(e) => err_json(format!("failed to read history: {e}")),
    }
}

fn handle_key_compatible(args: &Value) -> String {
    let key_str = match args.get("key").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return err_json("missing required 'key'"),
    };
    let key = match Camelot::parse(key_str) {
        Some(k) => k,
        None => return err_json(format!("could not parse Camelot key '{key_str}' (expected e.g. '8A')")),
    };
    let strict: Vec<String> = compatible_keys(key, false).iter().map(|c| c.to_string()).collect();
    let extended: Vec<String> = compatible_keys(key, true).iter().map(|c| c.to_string()).collect();
    json!({"status": "ok", "key": key.to_string(), "strict": strict, "extended": extended}).to_string()
}

fn handle_estimate_tempo(args: &Value) -> String {
    if let Some(sim) = args.get("simulate") {
        let params: SimulateParams = match serde_json::from_value(sim.clone()) {
            Ok(p) => p,
            Err(e) => return err_json(format!("invalid 'simulate' arguments: {e}")),
        };
        let est = simulate_tempo_estimate(&params);
        return json!({"status": "ok", "mode": "simulate", "estimate": est}).to_string();
    }

    let onsets: Vec<f64> = match args.get("onsets_sec").and_then(|v| v.as_array()) {
        Some(arr) => arr.iter().filter_map(|v| v.as_f64()).collect(),
        None => return err_json("provide either 'onsets_sec' (real path) or 'simulate' (Monte-Carlo path)"),
    };
    if onsets.is_empty() {
        return err_json("'onsets_sec' must be non-empty");
    }
    let amplitudes: Option<Vec<f64>> = args.get("amplitudes").and_then(|v| v.as_array()).map(|arr| arr.iter().filter_map(|v| v.as_f64()).collect());
    let duration_sec = args.get("duration_sec").and_then(|v| v.as_f64());
    let frame_hz = args.get("frame_hz").and_then(|v| v.as_f64()).unwrap_or(100.0);
    let bpm_lo = args.get("bpm_lo").and_then(|v| v.as_f64()).unwrap_or(60.0);
    let bpm_hi = args.get("bpm_hi").and_then(|v| v.as_f64()).unwrap_or(200.0);
    let bpm_step = args.get("bpm_step").and_then(|v| v.as_f64()).unwrap_or(0.5);

    match estimate_tempo_from_onsets(&onsets, amplitudes.as_deref(), duration_sec, frame_hz, bpm_lo, bpm_hi, bpm_step) {
        Some(est) => json!({"status": "ok", "mode": "onsets", "estimate": est}).to_string(),
        None => err_json("could not estimate tempo from the given onsets"),
    }
}

fn update_server_url_from_args(args: &Value) -> String {
    args.get("update_server_url").and_then(|v| v.as_str()).map(|s| s.to_string()).unwrap_or_else(updater::default_update_server_url)
}

/// Read-only: performs one network GET (`updater::check_for_update`) and
/// nothing else. This function must never call `apply_update` or any
/// filesystem-writing helper in `updater` — that separation is what
/// `tests::dj_check_for_update_never_touches_filesystem` in this module and
/// `updater::tests::check_for_update_never_touches_filesystem_or_binary`
/// both pin.
fn handle_check_for_update(args: &Value) -> String {
    let server_url = update_server_url_from_args(args);
    match updater::check_for_update(&server_url) {
        Ok(result) => json!({"status": "ok", "update_available": result.update_available, "current_version": result.current_version, "latest_version": result.latest_version, "download_url": result.download_url}).to_string(),
        Err(e) => err_json(format!("update check against {server_url} failed: {e}")),
    }
}

/// The confirmation-gated write path: downloads, verifies, and replaces the
/// running binary. Only reachable by an explicit `dj_apply_update` tool
/// call — never invoked automatically by `dj_check_for_update` or by any
/// background timer in this binary (the MCP server is a short-lived
/// stdio process with no periodic loop; periodic checking is a
/// `flux-dj-server`-only feature, see that binary's background task).
fn handle_apply_update(args: &Value) -> String {
    let server_url = update_server_url_from_args(args);
    match updater::apply_update(&server_url) {
        Ok(result) if result.applied => json!({
            "status": "ok",
            "applied": true,
            "previous_version": result.previous_version,
            "new_version": result.new_version,
            "restart_required": true,
            "message": "Binary replaced on disk. The current process is still running the old version — restart flux-dj-mcp (reconnect the MCP client) to run the new one."
        })
        .to_string(),
        Ok(result) => json!({
            "status": "ok",
            "applied": false,
            "current_version": result.previous_version,
            "message": "No newer version is currently published — nothing to apply."
        })
        .to_string(),
        Err(e) => err_json(format!("update apply against {server_url} failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serializes tests that touch FLUX_DJ_DATA_DIR / FLUX_DJ_WEBHOOK_URL env
    // vars, since env vars are process-global and cargo runs tests in
    // parallel threads by default.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn registry_has_all_six_tools() {
        let registry = build_registry();
        let names: Vec<String> = registry.tools_schema().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
        for expected in ["dj_log_track", "dj_get_history", "dj_key_compatible", "dj_estimate_tempo", "dj_check_for_update", "dj_apply_update"] {
            assert!(names.contains(&expected.to_string()), "missing tool {expected}");
        }
        assert_eq!(names.len(), 6);
    }

    #[test]
    fn dj_key_compatible_roundtrip() {
        let registry = build_registry();
        let result = registry.execute("dj_key_compatible", &json!({"key": "8A"})).unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["strict"].as_array().unwrap().len(), 3);
        assert_eq!(v["extended"].as_array().unwrap().len(), 5);
    }

    #[test]
    fn dj_key_compatible_rejects_bad_key() {
        let registry = build_registry();
        let result = registry.execute("dj_key_compatible", &json!({"key": "nope"})).unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["status"], "error");
    }

    #[test]
    fn dj_estimate_tempo_simulate_mode() {
        let registry = build_registry();
        let result = registry
            .execute(
                "dj_estimate_tempo",
                &json!({"simulate": {"true_bpm": 124.0, "jitter_ms": 5.0, "noise_amp": 0.1, "seed": 1337}}),
            )
            .unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["mode"], "simulate");
        assert!(v["estimate"]["estimated_bpm"].as_f64().unwrap() > 0.0);
    }

    #[test]
    fn dj_estimate_tempo_onsets_mode() {
        let registry = build_registry();
        let mut onsets = Vec::new();
        let mut t = 0.0;
        while t < 20.0 {
            onsets.push(t);
            t += 60.0 / 124.0;
        }
        let result = registry.execute("dj_estimate_tempo", &json!({"onsets_sec": onsets})).unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["mode"], "onsets");
        let bpm = v["estimate"]["estimated_bpm"].as_f64().unwrap();
        assert!((bpm - 124.0).abs() < 5.0, "expected near 124 BPM, got {bpm}");
    }

    #[test]
    fn dj_log_and_get_history_roundtrip() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("FLUX_DJ_DATA_DIR", dir.path());
        std::env::remove_var("FLUX_DJ_WEBHOOK_URL");

        let registry = build_registry();
        let log_result = registry
            .execute("dj_log_track", &json!({"artist": "Valeria", "title": "Test Track", "bpm": 128.0, "key": "8A"}))
            .unwrap();
        let v: Value = serde_json::from_str(&log_result).unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["track"]["artist"], "Valeria");

        let hist_result = registry.execute("dj_get_history", &json!({"limit": 5})).unwrap();
        let hv: Value = serde_json::from_str(&hist_result).unwrap();
        assert_eq!(hv["status"], "ok");
        assert_eq!(hv["count"], 1);
        assert_eq!(hv["tracks"][0]["title"], "Test Track");

        std::env::remove_var("FLUX_DJ_DATA_DIR");
    }

    #[test]
    fn unknown_tool_returns_none() {
        let registry = build_registry();
        assert!(registry.execute("dj_nonexistent", &json!({})).is_none());
    }

    /// Same one-shot HTTP responder as `updater::tests` — kept local to
    /// this module (rather than made `pub` and shared) so each test module
    /// stays self-contained; the duplication is 12 lines.
    fn spawn_one_shot_json_server(body: String) -> String {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local_addr");
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        format!("http://{addr}")
    }

    /// The hard gating requirement, exercised through the actual MCP
    /// dispatch path (registry.execute), not just the underlying updater
    /// module directly: calling `dj_check_for_update` must report an
    /// available update but must NEVER create an `.update-tmp` file or
    /// modify the running test binary. Only `dj_apply_update` is allowed to
    /// touch the binary — and this test proves `dj_check_for_update`
    /// doesn't, even when an update IS available.
    #[test]
    fn dj_check_for_update_never_touches_filesystem() {
        let manifest = serde_json::json!({
            "version": "999.0.0",
            "download_url": "/downloads/flux-dj-server-v999.0.0",
            "sha256": "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
        });
        let server_url = spawn_one_shot_json_server(manifest.to_string());

        let current_exe = std::env::current_exe().expect("current_exe");
        let before = std::fs::metadata(&current_exe).expect("metadata before");
        let temp_marker = current_exe.with_extension("update-tmp");
        assert!(!temp_marker.exists());

        let registry = build_registry();
        let result = registry.execute("dj_check_for_update", &json!({"update_server_url": server_url})).unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["update_available"], true);
        assert_eq!(v["latest_version"], "999.0.0");

        assert!(!temp_marker.exists(), "dj_check_for_update must never create an update-tmp file");
        let after = std::fs::metadata(&current_exe).expect("metadata after");
        assert_eq!(before.len(), after.len());
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());
    }

    /// `dj_apply_update` IS allowed to touch the binary when an update is
    /// genuinely available and confirmed — but this crate's own test
    /// binary must never be the target of a real self-replace (that would
    /// corrupt the running `cargo`/`fluxc` test process). This test
    /// exercises the real `dj_apply_update` code path end-to-end through a
    /// live mock server that reports the CURRENT (not newer) version, so
    /// `updater::apply_update` takes its genuine no-op branch — proving the
    /// tool is reachable and correctly wired without ever reaching
    /// `self_replace::self_replace`.
    #[test]
    fn dj_apply_update_is_a_safe_noop_when_already_current() {
        let manifest = serde_json::json!({
            "version": crate::updater::CURRENT_VERSION,
            "download_url": "/downloads/flux-dj-server-current",
            "sha256": null
        });
        let server_url = spawn_one_shot_json_server(manifest.to_string());

        let current_exe = std::env::current_exe().expect("current_exe");
        let before = std::fs::metadata(&current_exe).expect("metadata before");

        let registry = build_registry();
        let result = registry.execute("dj_apply_update", &json!({"update_server_url": server_url})).unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["applied"], false);

        let after = std::fs::metadata(&current_exe).expect("metadata after");
        assert_eq!(before.len(), after.len(), "no-op apply must not touch the binary");
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());
    }
}
