//! flux-webcam MCP handler — consent-gated frame capture for an agent.
//!
//! # The security shape of this file
//!
//! Five tools are registered. Note what is **absent**:
//!
//! | tool | effect on access | exposed? |
//! |---|---|---|
//! | `flux_webcam_status`      | none (read-only)   | ✅ |
//! | `flux_webcam_capture`     | spends consent     | ✅ (gated) |
//! | `flux_webcam_revoke`      | **reduces**        | ✅ |
//! | `flux_webcam_panic_stop`  | **reduces**        | ✅ |
//! | `flux_webcam_audit`       | none (read-only)   | ✅ |
//! | `flux_webcam_grant`       | **increases**      | ❌ **never** |
//!
//! Every tool an agent can reach either observes access or *reduces* it. The
//! single operation that would widen access — issuing a grant — has no MCP
//! surface at all. It is operator-only, by hand, on the box. That asymmetry is
//! the entire security design: an agent cannot talk its way into permission,
//! because there is no verb for it to say.
//!
//! # What this actually guarantees, and what it does not
//!
//! Guaranteed: capture through this surface is **default-denied**, time-boxed,
//! budget-limited, revocable mid-session, killable by a file, and every attempt
//! — permitted or refused — lands in a BLAKE3 hash-chained audit log that
//! cannot be edited without breaking the chain.
//!
//! **Not** guaranteed: immunity from a process running as root on the same box.
//! Anyone with root can bypass this gate entirely by invoking a capture tool
//! directly, and no amount of Rust in this file changes that. What the audit
//! chain buys is that such a bypass is *detectable*, not that it is impossible.
//! The only true prevention is physical: unplug the camera, or cover the lens.
//! That is stated here rather than buried, because a security control whose
//! limits are undocumented is worse than none — it buys false confidence.

use crate::handlers::{ToolDef, ToolRegistry};
use flux_webcam::{
    CommandSource, ConsentGate, FileSource, SecureWebcam, SyntheticSource, WebcamEngine,
};
use serde_json::{json, Value};
use std::sync::{Mutex, OnceLock};

/// Process-wide capture engine.
///
/// Held across calls so the measured telemetry (latency percentiles, success
/// rate, the SAP score derived from them) accumulates over a session instead of
/// resetting to a meaningless single sample on every tool invocation. The
/// consent state itself lives on disk, not here — restarting the MCP server
/// must not hand anyone a fresh budget.
fn cam() -> &'static Mutex<SecureWebcam> {
    static CAM: OnceLock<Mutex<SecureWebcam>> = OnceLock::new();
    CAM.get_or_init(|| Mutex::new(SecureWebcam::with_env_gate(build_engine())))
}

/// Resolve the frame source from the environment.
///
/// Order is deliberate: an explicitly configured real device wins, then an
/// explicit file drop, and only then the synthetic fallback. The synthetic
/// source is last so that a misconfigured device silently degrading into
/// "here is a test pattern" cannot be mistaken for a working camera — the
/// source name is always reported in the status output.
fn build_engine() -> WebcamEngine {
    let agent = std::env::var("FLUX_WEBCAM_AGENT").unwrap_or_else(|_| "grogu".into());

    if let Ok(device) = std::env::var("FLUX_WEBCAM_DEVICE") {
        let out = frames_dir().join("capture.jpg");
        return WebcamEngine::new(Box::new(CommandSource::ffmpeg_v4l2(&device, out)), agent);
    }
    if let Ok(path) = std::env::var("FLUX_WEBCAM_FILE") {
        return WebcamEngine::new(Box::new(FileSource::new(path)), agent);
    }
    let w = env_u32("FLUX_WEBCAM_WIDTH", 640);
    let h = env_u32("FLUX_WEBCAM_HEIGHT", 480);
    WebcamEngine::new(Box::new(SyntheticSource::new(w, h)), agent)
}

fn env_u32(key: &str, default: u32) -> u32 {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// Where captured frames are written for the agent to read back.
fn frames_dir() -> std::path::PathBuf {
    let root = std::env::var("FLUX_WEBCAM_OUT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| ConsentGate::resolve().paths.root.join("frames"));
    let _ = std::fs::create_dir_all(&root);
    root
}

pub fn register(registry: &mut ToolRegistry) {
    registry.register(
        ToolDef {
            name: "flux_webcam_status",
            description: "Consent + capture status for flux-webcam. READ-ONLY: reports whether a capture would be permitted right now and why, without spending any of the grant budget. Shows the active grant (remaining captures, seconds left, intent confidence), kill-switch state, the resolved frame source, measured capture telemetry and the SAP score. Call this before attempting a capture.",
            input_schema: json!({"type": "object", "properties": {
                "stake_qug": {"type": "integer", "description": "Stake to use for the SAP stake component (default 0)"}
            }}),
        },
        flux_webcam_status,
    );
    registry.register(
        ToolDef {
            name: "flux_webcam_capture",
            description: "Capture ONE frame, if and only if the operator's consent gate permits it. Default-denied: with no active grant this returns a refusal and NEVER touches the camera. On success writes a PNG/JPEG to disk and returns its absolute path (read it with the Read tool), plus the BLAKE3 content hash and remaining budget. Spends exactly one capture from the grant. There is no way to raise your own permission — grants are issued by the operator on the box, never over MCP.",
            input_schema: json!({"type": "object", "properties": {
                "label": {"type": "string", "description": "Short label recorded in the audit log, e.g. 'yoga pose check'"}
            }}),
        },
        flux_webcam_capture,
    );
    registry.register(
        ToolDef {
            name: "flux_webcam_revoke",
            description: "Revoke the active consent grant immediately. Always safe to call — it can only REDUCE access, never widen it. Any in-flight budget is discarded; the next capture is denied until the operator issues a new grant by hand.",
            input_schema: json!({"type": "object", "properties": {
                "who": {"type": "string", "description": "Who is revoking (recorded in the audit log)"}
            }}),
        },
        flux_webcam_revoke,
    );
    registry.register(
        ToolDef {
            name: "flux_webcam_panic_stop",
            description: "Engage the kill switch: writes a DENY file that refuses ALL capture regardless of any live grant. Overrides everything. Deny-only, so always safe to call — but it can only be cleared by the operator on the box, NOT over MCP. Use if anything looks wrong.",
            input_schema: json!({"type": "object", "properties": {
                "who": {"type": "string", "description": "Who engaged it (recorded in the audit log)"}
            }}),
        },
        flux_webcam_panic_stop,
    );
    registry.register(
        ToolDef {
            name: "flux_webcam_audit",
            description: "Verify and read the tamper-evident audit log. Every grant, capture, denial and revocation is appended with a BLAKE3 hash chain; this re-derives the whole chain and reports the first entry that does not check out, if any. Use it to prove no capture happened outside the record.",
            input_schema: json!({"type": "object", "properties": {
                "tail": {"type": "integer", "description": "How many recent entries to show (default 20, max 200)"}
            }}),
        },
        flux_webcam_audit,
    );
}

fn flux_webcam_status(args: &Value) -> String {
    let stake = args.get("stake_qug").and_then(|v| v.as_u64()).unwrap_or(0);
    let Ok(mut c) = cam().lock() else {
        return "❌ flux_webcam_status: engine lock poisoned".into();
    };
    let mut status = c.status_json(stake);
    status["policy"] = json!({
        "grant_over_mcp": false,
        "note": "grants are operator-only and cannot be issued through any MCP tool",
        "mcp_tools_that_widen_access": [],
        "physical_guarantee": "software cannot stop root on this host; unplugging the camera can",
    });
    serde_json::to_string_pretty(&status).unwrap_or_else(|e| format!("❌ {e}"))
}

fn flux_webcam_capture(args: &Value) -> String {
    let label = args.get("label").and_then(|v| v.as_str()).unwrap_or("(no label)");
    let Ok(mut c) = cam().lock() else {
        return "❌ flux_webcam_capture: engine lock poisoned".into();
    };

    // Cheap pre-check purely so a refusal can explain itself richly. The
    // authoritative decision is still made inside `capture()`, which spends the
    // budget atomically — this peek peeks and nothing more.
    let peek = c.peek();
    if !peek.is_allowed() {
        c.gate().audit("DENY_MCP", label);
        return format!(
            "🔒 CAPTURE DENIED — {}\n\n\
             The camera was NOT touched. No frame exists.\n\n\
             This is the default state. To permit a capture, the operator must issue a grant \
             ON THE BOX — there is deliberately no MCP tool that can do it:\n\
             \n    fluxc webcam grant --seconds 300 --captures 5 --reason \"yoga\"\n\n\
             Grants are time-boxed, budget-limited, and revocable at any moment.",
            peek.reason_str()
        );
    }

    match c.capture() {
        Ok(frame) => {
            let name = format!("frame-{}-{}.{}", frame.captured_at_ms, &frame.hash[..12], frame.format.extension());
            let path = frames_dir().join(&name);
            if let Err(e) = std::fs::write(&path, &frame.data) {
                return format!("⚠ captured but could not write to disk: {e}");
            }
            c.gate().audit("CAPTURE_MCP", label);
            let remaining = c
                .gate()
                .load()
                .map(|g| g.remaining())
                .unwrap_or(0);
            format!(
                "📷 CAPTURED\n\n\
                 path        : {}\n\
                 dimensions  : {}×{}\n\
                 format      : {}\n\
                 bytes       : {}\n\
                 blake3      : {}\n\
                 label       : {}\n\
                 remaining   : {} capture(s) left on this grant\n\n\
                 Read the file above to view it. The hash content-addresses these exact bytes.",
                path.display(),
                frame.width,
                frame.height,
                frame.format.as_str(),
                frame.len(),
                frame.hash,
                label,
                remaining
            )
        }
        Err(e) => format!("❌ capture failed after consent was spent: {e}"),
    }
}

fn flux_webcam_revoke(args: &Value) -> String {
    let who = args.get("who").and_then(|v| v.as_str()).unwrap_or("agent");
    let Ok(c) = cam().lock() else {
        return "❌ flux_webcam_revoke: engine lock poisoned".into();
    };
    match c.revoke(who) {
        Ok(()) => format!(
            "🔒 Consent revoked by {who}. Capture is denied until the operator issues a new grant."
        ),
        Err(e) => format!("❌ revoke failed: {e}"),
    }
}

fn flux_webcam_panic_stop(args: &Value) -> String {
    let who = args.get("who").and_then(|v| v.as_str()).unwrap_or("agent");
    let Ok(c) = cam().lock() else {
        return "❌ flux_webcam_panic_stop: engine lock poisoned".into();
    };
    match c.panic_stop(who) {
        Ok(()) => format!(
            "🛑 KILL SWITCH ENGAGED by {who}.\n\n\
             All capture is now refused, overriding any live grant.\n\
             It can only be cleared by the operator on the box — no MCP tool can lift it."
        ),
        Err(e) => format!("❌ could not engage kill switch: {e}"),
    }
}

fn flux_webcam_audit(args: &Value) -> String {
    let tail = args
        .get("tail")
        .and_then(|v| v.as_u64())
        .unwrap_or(20)
        .clamp(1, 200) as usize;
    let gate = ConsentGate::resolve();
    let verdict = match gate.verify_audit() {
        Ok(n) => format!("✅ chain intact — {n} entries, all hashes check out"),
        Err(seq) => format!(
            "🚨 CHAIN BROKEN at entry {seq} — the audit log has been edited or truncated. \
             Treat every later entry as untrusted."
        ),
    };
    let log = std::fs::read_to_string(gate.paths.audit()).unwrap_or_default();
    let lines: Vec<&str> = log.lines().filter(|l| !l.trim().is_empty()).collect();
    let shown: Vec<&str> = lines.iter().rev().take(tail).rev().copied().collect();
    format!(
        "{verdict}\n\nlog: {}\ntotal entries: {}\n\n--- last {} ---\n{}",
        gate.paths.audit().display(),
        lines.len(),
        shown.len(),
        if shown.is_empty() { "(empty)".to_string() } else { shown.join("\n") }
    )
}
