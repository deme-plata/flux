//! Outbound webhook: fire a POST with the logged track's JSON to
//! `$FLUX_DJ_WEBHOOK_URL`, if set, whenever a track is logged.
//!
//! Fire-and-forget on a dedicated OS thread (not a tokio task) so this one
//! function works unmodified from both the async axum server AND the
//! synchronous stdio MCP loop — no runtime needs to be running for the call
//! site. Delivery failures are logged to stderr and never propagate: an
//! unreachable/misconfigured webhook must never block or fail a track-log
//! call, per the spec ("log failures, don't block/crash on a failed
//! delivery").

use crate::Track;

const ENV_WEBHOOK_URL: &str = "FLUX_DJ_WEBHOOK_URL";

/// POST `track` to the configured webhook URL, if any. Returns immediately;
/// delivery happens on a spawned thread.
pub fn notify_outbound(track: &Track) {
    let url = match std::env::var(ENV_WEBHOOK_URL) {
        Ok(u) if !u.trim().is_empty() => u,
        _ => return, // not configured — nothing to do, not an error
    };
    let track = track.clone();
    std::thread::spawn(move || {
        deliver(&url, &track);
    });
}

fn deliver(url: &str, track: &Track) {
    let client = match reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(5)).build() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[flux-dj] webhook client build failed: {e}");
            return;
        }
    };
    match client.post(url).json(track).send() {
        Ok(resp) if resp.status().is_success() => {
            eprintln!("[flux-dj] webhook delivered to {url} (HTTP {})", resp.status());
        }
        Ok(resp) => {
            eprintln!("[flux-dj] webhook to {url} returned non-success status HTTP {}", resp.status());
        }
        Err(e) => {
            eprintln!("[flux-dj] webhook delivery to {url} FAILED: {e}");
        }
    }
}
