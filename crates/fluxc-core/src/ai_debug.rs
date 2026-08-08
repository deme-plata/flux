// flux_ai_debug — AI-readable diagnostic logs for Flux
//
// Produces structured JSON logs optimized for AI consumption, not humans.
// Each log entry is a machine-readable event with semantic fields that
// AI agents (DeepSeek, Claude, Grok) parse directly — no regex needed.
//
// Format: {"t":"event_type","ts":ms,"fields":{...},"agent":"deepseek"}

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static AI_LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Log an AI-readable event.
pub fn ai_log(event_type: &str, fields: &[(&str, &str)], agent: &str) {
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();
    let fields_json: String = fields.iter()
        .map(|(k, v)| format!("\"{}\":\"{}\"", k, v))
        .collect::<Vec<_>>().join(",");
    let entry = format!(r#"{{"t":"{}","ts":{},"agent":"{}",{}}}"#, event_type, ts, agent, fields_json);
    if let Ok(mut log) = AI_LOG.lock() {
        log.push(entry);
        if log.len() > 1000 { log.drain(0..200); }
    }
}

/// Dump recent AI logs as JSON array.
pub fn ai_log_dump(n: usize) -> String {
    let log = AI_LOG.lock().unwrap_or_else(|e| e.into_inner());
    let recent: Vec<&String> = log.iter().rev().take(n).collect();
    format!("[{}]", recent.iter().map(|e| e.as_str()).collect::<Vec<_>>().join(","))
}

/// Signal a build event to all consuming AI agents.
pub fn ai_build_event(pkg: &str, success: bool, elapsed_ms: u64, agent: &str) {
    ai_log("build", &[
        ("pkg", pkg),
        ("success", if success { "true" } else { "false" }),
        ("elapsed_ms", &elapsed_ms.to_string()),
    ], agent);
}

/// Signal a test event.
pub fn ai_test_event(pkg: &str, passed: u32, failed: u32, agent: &str) {
    ai_log("test", &[
        ("pkg", pkg),
        ("passed", &passed.to_string()),
        ("failed", &failed.to_string()),
    ], agent);
}

/// Signal a deploy event.
pub fn ai_deploy_event(target: &str, success: bool, agent: &str) {
    ai_log("deploy", &[
        ("target", target),
        ("success", if success { "true" } else { "false" }),
    ], agent);
}

/// Write AI logs to a file for cross-agent consumption.
pub fn ai_log_flush(path: &str) {
    let dump = ai_log_dump(100);
    let _ = std::fs::write(path, dump);
}

#[cfg(test)]
mod tests {
    use super::*;

    // AI_LOG is one shared static and tests run in PARALLEL threads — a
    // dump(1) only sees the LAST entry, so whichever test logged second made
    // the other flake (caught live 2026-08-08: green run, RED 60s later).
    // Dump a window and look for our own entry instead of asserting "last".

    #[test]
    fn test_ai_log() {
        ai_log("test_event", &[("key", "val")], "deepseek");
        let dump = ai_log_dump(100);
        assert!(dump.contains("test_event"));
        assert!(dump.contains("deepseek"));
    }

    #[test]
    fn test_ai_build_event() {
        ai_build_event("fluxmux", true, 450, "deepseek");
        let dump = ai_log_dump(100);
        assert!(dump.contains("fluxmux"));
        assert!(dump.contains("true"));
    }
}
