//! flux-dj — DJ / live-performance track logging, Camelot-wheel harmonic-
//! mixing compatibility, and autocorrelation-based tempo estimation.
//!
//! Built for a technical outreach crate accompanying "The Beat Budget"
//! (`crates/flux-arxiv-latex/src/bin/ai_dj_live.rs`): the Camelot graph and
//! beat-tracking algorithms in [`camelot`] and [`tempo`] are ported from
//! that binary (see each module's doc comment for exactly what changed vs.
//! stayed identical). This crate turns those algorithms into a small live
//! service:
//!   - an HTTP webhook receiver + history endpoint (`flux-dj-server` binary,
//!     see [`mod@outbound`] and `src/bin/flux-dj-server.rs`)
//!   - a standalone MCP tool server (`flux-dj-mcp` binary, see [`mcp`])
//!   - an outbound webhook fired whenever a track is logged, if
//!     `FLUX_DJ_WEBHOOK_URL` is configured (see [`outbound`])

pub mod camelot;
pub mod mcp;
pub mod outbound;
pub mod store;
pub mod tempo;
pub mod updater;

use serde::{Deserialize, Serialize};

pub use store::Store;

/// A logged track. Serializes to/from the JSON shape used by both the
/// `POST /webhook/now-playing` HTTP body and the `dj_log_track` MCP tool
/// arguments, and is what both persist + forward to the outbound webhook.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Track {
    pub artist: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bpm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// RFC3339 timestamp.
    pub timestamp: String,
}

/// Incoming "now playing" request — same shape as [`Track`] except
/// `timestamp` is optional (defaults to now when omitted, per spec).
#[derive(Debug, Clone, Deserialize)]
pub struct NewTrackRequest {
    pub artist: String,
    pub title: String,
    #[serde(default)]
    pub bpm: Option<f64>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
}

impl NewTrackRequest {
    pub fn into_track(self) -> Track {
        let timestamp = self.timestamp.unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
        Track { artist: self.artist, title: self.title, bpm: self.bpm, key: self.key, timestamp }
    }
}

/// Log a track: persist it to the default JSONL store and fire the
/// outbound webhook (if configured). Used by both the HTTP handler and the
/// `dj_log_track` MCP tool so the two surfaces share one code path.
pub fn log_track(req: NewTrackRequest) -> anyhow::Result<Track> {
    let track = req.into_track();
    let store = Store::open_default()?;
    store.append(&track)?;
    outbound::notify_outbound(&track);
    Ok(track)
}

/// Fetch recent history from the default store. Used by both the HTTP
/// `GET /history` handler and the `dj_get_history` MCP tool.
pub fn get_history(limit: usize) -> anyhow::Result<Vec<Track>> {
    let store = Store::open_default()?;
    store.recent(limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn new_track_request_defaults_timestamp() {
        let req = NewTrackRequest { artist: "A".into(), title: "B".into(), bpm: None, key: None, timestamp: None };
        let track = req.into_track();
        assert!(!track.timestamp.is_empty());
        // Should parse as RFC3339.
        assert!(chrono::DateTime::parse_from_rfc3339(&track.timestamp).is_ok());
    }

    #[test]
    fn new_track_request_preserves_explicit_timestamp() {
        let req = NewTrackRequest { artist: "A".into(), title: "B".into(), bpm: Some(128.0), key: Some("8A".into()), timestamp: Some("2026-08-11T12:00:00Z".into()) };
        let track = req.into_track();
        assert_eq!(track.timestamp, "2026-08-11T12:00:00Z");
        assert_eq!(track.bpm, Some(128.0));
        assert_eq!(track.key, Some("8A".to_string()));
    }

    #[test]
    fn log_track_and_get_history_share_one_store() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("FLUX_DJ_DATA_DIR", dir.path());
        std::env::remove_var("FLUX_DJ_WEBHOOK_URL");

        let req = NewTrackRequest { artist: "Valeria".into(), title: "Opener".into(), bpm: Some(122.0), key: Some("9A".into()), timestamp: None };
        let logged = log_track(req).unwrap();
        assert_eq!(logged.artist, "Valeria");

        let history = get_history(10).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0], logged);

        std::env::remove_var("FLUX_DJ_DATA_DIR");
    }
}
