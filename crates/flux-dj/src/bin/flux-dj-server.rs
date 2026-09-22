//! flux-dj-server — HTTP webhook receiver + set-history API + auto-update
//! publish/apply endpoints.
//!
//! Run:
//! ```bash
//! flux-dj-server                       # default :4210
//! FLUX_DJ_PORT=4210 flux-dj-server
//!
//! # one-shot, explicit opt-in update apply (never runs unless you pass this):
//! flux-dj-server --apply-update              # download+verify+replace, then exit (no restart)
//! flux-dj-server --apply-update --restart    # ...then spawn the new binary and exit
//! ```
//!
//! Routes:
//! ```text
//! GET  /health                 — health check
//! POST /webhook/now-playing    — log a track: {artist, title, bpm?, key?, timestamp?}
//! GET  /history?limit=N        — recent tracks, newest first (default limit 20)
//! GET  /api/v1/dj-version      — version manifest {version, download_url, sha256}
//!                                 (self-describing: reports this instance's own
//!                                 version unless a newer release is published
//!                                 under FLUX_DJ_DOWNLOADS_DIR)
//! GET  /downloads/:filename    — serves a published release, or this binary's
//!                                 own running bytes at "flux-dj-server-current"
//! POST /update/apply           — EXPLICIT opt-in apply (never automatic — see below)
//! ```
//!
//! **Auto-update confirmation gate**: this server checks its configured
//! `FLUX_DJ_UPDATE_SERVER_URL` (default: itself) for a newer version once
//! per `FLUX_DJ_UPDATE_CHECK_INTERVAL_SECS` (default 3600s) and ONLY LOGS
//! it — it never downloads or applies anything on its own. Applying an
//! update requires either the `--apply-update` CLI flag at startup or an
//! explicit `POST /update/apply` call; both are opt-in actions a human (or
//! their AI, for the MCP surface — see `flux_dj::mcp`) has to take
//! deliberately. See `flux_dj::updater` for the full design rationale,
//! modeled on `gui/slint-wallet/src/updater.rs`'s proven precedent.
//!
//! If `FLUX_DJ_WEBHOOK_URL` is set, every successfully logged track is also
//! POSTed there (fire-and-forget; see `flux_dj::outbound`).
//! Data is stored as JSONL at `$FLUX_DJ_DATA_DIR/tracks.jsonl`
//! (default `/home/storage/deepseek-codewhale/flux/data/flux-dj/tracks.jsonl`).

use axum::extract::{Path as AxumPath, Query};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use flux_dj::updater::{self, VersionManifest};
use flux_dj::{get_history, log_track, NewTrackRequest, Track};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct HistoryQuery {
    limit: Option<usize>,
}

async fn health_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status": "ok", "service": "flux-dj"}))
}

async fn now_playing_handler(Json(req): Json<NewTrackRequest>) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let track = log_track(req).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({"status": "ok", "track": track})))
}

async fn history_handler(Query(q): Query<HistoryQuery>) -> Result<Json<Vec<Track>>, (StatusCode, String)> {
    let limit = q.limit.unwrap_or(20).min(1000);
    let tracks = get_history(limit).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(tracks))
}

/// Build this instance's version manifest: prefer the highest published
/// release under `FLUX_DJ_DOWNLOADS_DIR` (`flux-dj-server-vX.Y.Z`); fall
/// back to self-describing (report this binary's own running version,
/// servable at `/downloads/flux-dj-server-current`) when nothing has been
/// published yet. Either way the SHA-256 is computed from the actual bytes
/// that `/downloads/...` will serve, so it's always correct, never asserted.
fn build_version_manifest() -> VersionManifest {
    let base = updater::download_base_url();
    if let Some((version, path)) = updater::detect_latest_release(&updater::downloads_dir(), "flux-dj-server-v") {
        let sha256 = updater::sha256_of_file(&path).ok();
        let download_url = updater::resolve_url(&base, &format!("/downloads/flux-dj-server-v{version}"));
        return VersionManifest { version, download_url, sha256 };
    }
    let version = updater::CURRENT_VERSION.to_string();
    let sha256 = std::env::current_exe().ok().and_then(|p| updater::sha256_of_file(&p).ok());
    let download_url = updater::resolve_url(&base, "/downloads/flux-dj-server-current");
    VersionManifest { version, download_url, sha256 }
}

async fn dj_version_handler() -> Json<VersionManifest> {
    Json(build_version_manifest())
}

async fn downloads_handler(AxumPath(filename): AxumPath<String>) -> Result<axum::response::Response, StatusCode> {
    let downloads_dir = updater::downloads_dir();
    let candidate = downloads_dir.join(&filename);
    let bytes = if candidate.exists() {
        std::fs::read(&candidate).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    } else if filename == "flux-dj-server-current" {
        let exe = std::env::current_exe().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        std::fs::read(&exe).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    } else {
        return Err(StatusCode::NOT_FOUND);
    };
    Ok(([("content-type", "application/octet-stream")], bytes).into_response())
}

#[derive(Debug, Deserialize, Default)]
struct ApplyUpdateBody {
    #[serde(default)]
    restart: bool,
    #[serde(default)]
    update_server_url: Option<String>,
}

/// The one explicit, human-initiated write path on this binary — see the
/// module doc's confirmation-gate section. Never called by the periodic
/// background checker. Body is optional JSON (`{"restart": bool,
/// "update_server_url": "..."}`) — an empty/absent body is treated as
/// defaults, so a bare `curl -X POST .../update/apply` works.
async fn apply_update_handler(raw_body: axum::body::Bytes) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let body: ApplyUpdateBody = if raw_body.is_empty() { ApplyUpdateBody::default() } else { serde_json::from_slice(&raw_body).unwrap_or_default() };
    let server_url = body.update_server_url.unwrap_or_else(updater::default_update_server_url);
    let restart = body.restart;

    let result = tokio::task::spawn_blocking(move || updater::apply_update(&server_url))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("apply task panicked: {e}")))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if result.applied && restart {
        // Give the HTTP response time to actually reach the client before
        // this process exits — restart_process() never returns.
        tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            updater::restart_process();
        });
    }

    Ok(Json(serde_json::json!({
        "status": "ok",
        "applied": result.applied,
        "previous_version": result.previous_version,
        "new_version": result.new_version,
        "restart_required": result.restart_required,
        "restarting": result.applied && restart,
    })))
}

/// Check-and-log ONLY. Never downloads or applies. Runs once per
/// `FLUX_DJ_UPDATE_CHECK_INTERVAL_SECS` in the background for the lifetime
/// of the server process.
async fn periodic_update_check_loop(update_server_url: String, interval_secs: u64) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
    loop {
        ticker.tick().await;
        let url = update_server_url.clone();
        let outcome = tokio::task::spawn_blocking(move || updater::check_for_update(&url)).await;
        match outcome {
            Ok(Ok(result)) if result.update_available => {
                println!(
                    "[flux-dj-server] update available: {} -> {} (call POST /update/apply, or dj_apply_update via MCP, to install — never automatic)",
                    result.current_version, result.latest_version
                );
            }
            Ok(Ok(_)) => { /* up to date — silent, avoid log spam */ }
            Ok(Err(e)) => eprintln!("[flux-dj-server] update check failed: {e}"),
            Err(e) => eprintln!("[flux-dj-server] update check task panicked: {e}"),
        }
    }
}

fn print_apply_result(result: &updater::ApplyResult) {
    if result.applied {
        println!("[flux-dj-server] applied update: {} -> {}", result.previous_version, result.new_version);
        println!("[flux-dj-server] restart required to run the new version.");
    } else {
        println!("[flux-dj-server] already at the latest published version ({}) — nothing to apply.", result.previous_version);
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Explicit opt-in one-shot apply path: `flux-dj-server --apply-update
    // [--restart]`. This is the CLI-flag half of the confirmation gate for
    // deployments with no AI/MCP in the loop — it NEVER runs unless the
    // flag is passed, and starting the server normally never reaches here.
    if args.iter().any(|a| a == "--apply-update") {
        let server_url = updater::default_update_server_url();
        match updater::apply_update(&server_url) {
            Ok(result) => {
                print_apply_result(&result);
                if result.applied && args.iter().any(|a| a == "--restart") {
                    println!("[flux-dj-server] restarting...");
                    updater::restart_process(); // never returns
                }
            }
            Err(e) => {
                eprintln!("[flux-dj-server] update apply FAILED: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    let port: u16 = std::env::var("FLUX_DJ_PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(4210);
    let update_check_interval: u64 = std::env::var("FLUX_DJ_UPDATE_CHECK_INTERVAL_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(3600);
    let update_server_url = updater::default_update_server_url();

    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/webhook/now-playing", post(now_playing_handler))
        .route("/history", get(history_handler))
        .route("/api/v1/dj-version", get(dj_version_handler))
        .route("/downloads/:filename", get(downloads_handler))
        .route("/update/apply", post(apply_update_handler));

    let addr = format!("0.0.0.0:{port}");
    println!("flux-dj-server v{} listening on {addr}", updater::CURRENT_VERSION);
    println!("  POST /webhook/now-playing  {{artist, title, bpm?, key?, timestamp?}}");
    println!("  GET  /history?limit=N");
    println!("  GET  /api/v1/dj-version");
    println!("  GET  /downloads/:filename");
    println!("  POST /update/apply         — EXPLICIT opt-in only, never automatic");
    println!("  data dir: {}", flux_dj::Store::default_path().display());
    println!("  downloads dir: {}", updater::downloads_dir().display());
    println!("  update checks every {update_check_interval}s against {update_server_url} (check-and-log only)");

    tokio::spawn(periodic_update_check_loop(update_server_url, update_check_interval));

    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    axum::serve(listener, app).await.expect("serve");
}
