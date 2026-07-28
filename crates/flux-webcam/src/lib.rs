//! # flux-webcam
//!
//! On-demand frame capture for the Flux agent surface.
//!
//! An agent cannot see. It can, however, be handed a picture — and that is a
//! narrower and much more honest capability than "live video". This crate
//! delivers exactly that: **one frame, when explicitly asked, with provenance.**
//!
//! ## Shape
//!
//! ```text
//!   FrameSource            WebcamEngine              SAP / relay
//!   ───────────            ────────────              ───────────
//!   synthetic  ─┐                                 ┌─ CaptureStats ──► ScoreTable
//!   file drop  ─┼──► capture() ──► Frame ────────►┤   (measured)      (flux-p2p)
//!   command    ─┘      (timed)   (BLAKE3-addressed)└─ FrameAnnounce ──► mesh
//! ```
//!
//! ## Design commitments
//!
//! - **One-shot only.** No background thread, no timer, no subscription. A
//!   frame exists because someone called [`WebcamEngine::capture`]. On a host
//!   that also runs production services, a camera that *cannot* self-trigger is
//!   the only acceptable kind.
//! - **Content-addressed.** Every frame carries a BLAKE3 of its own bytes, so a
//!   frame that crossed the mesh can be proven identical to what was captured.
//! - **Measured, not asserted.** The SAP score comes from real timings and real
//!   integrity checks ([`CaptureStats`]); nothing is estimated.
//! - **No new external dependencies.** The PNG encoder is hand-rolled
//!   ([`png`]) rather than pulling in an image stack.

pub mod consent;
pub mod frame;
pub mod png;
pub mod relay;
pub mod secure;
pub mod source;
pub mod stats;

pub use consent::{ConsentGate, ConsentGrant, ConsentPaths, Decision, DenyReason};
pub use frame::{Frame, FrameFormat};
pub use secure::{SecureError, SecureWebcam};
pub use relay::{FrameAnnounce, FrameRelay, RelayReject};
pub use source::{CaptureError, CaptureResult, CommandSource, FileSource, FrameSource, SyntheticSource};
pub use stats::CaptureStats;

use flux_p2p::sap::ScoreTable;
use std::path::Path;
use std::time::Instant;

/// Ties a [`FrameSource`] to its measured telemetry and SAP scoring.
pub struct WebcamEngine {
    source: Box<dyn FrameSource + Send>,
    /// Identity this engine scores under in the SAP table.
    pub agent: String,
    pub stats: CaptureStats,
    pub table: ScoreTable,
    last: Option<Frame>,
}

impl WebcamEngine {
    pub fn new(source: Box<dyn FrameSource + Send>, agent: impl Into<String>) -> Self {
        WebcamEngine {
            source,
            agent: agent.into(),
            stats: CaptureStats::new(),
            table: ScoreTable::new(),
            last: None,
        }
    }

    /// A headless-safe engine: synthetic frames, no hardware required.
    pub fn synthetic(width: u32, height: u32, agent: impl Into<String>) -> Self {
        Self::new(Box::new(SyntheticSource::new(width, height)), agent)
    }

    pub fn source_name(&self) -> String {
        self.source.name()
    }

    pub fn source_available(&self) -> bool {
        self.source.available()
    }

    /// Capture one frame, timing it and folding the outcome into the stats.
    pub fn capture(&mut self) -> CaptureResult {
        let started = Instant::now();
        match self.source.capture() {
            Ok(frame) => {
                let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
                // Verify before counting it as a success — a frame that does not
                // hash to its own claim is an integrity failure, and SAP should
                // hear about it.
                if !frame.verify() {
                    self.stats.record_integrity_failure();
                    self.stats.record_failure();
                    return Err(CaptureError::Decode("frame failed self-verification".into()));
                }
                self.stats.record_success(elapsed_ms, frame.len());
                self.last = Some(frame.clone());
                Ok(frame)
            }
            Err(e) => {
                self.stats.record_failure();
                Err(e)
            }
        }
    }

    /// The most recent successful frame, if any.
    pub fn last_frame(&self) -> Option<&Frame> {
        self.last.as_ref()
    }

    /// Capture and write straight to disk. Returns the frame's hash.
    pub fn capture_to(&mut self, path: impl AsRef<Path>) -> Result<String, CaptureError> {
        let frame = self.capture()?;
        std::fs::write(path.as_ref(), &frame.data)?;
        Ok(frame.hash)
    }

    /// Current SAP total for this engine's agent, from measured telemetry.
    pub fn sap(&mut self, stake_qug: u64) -> f64 {
        let agent = self.agent.clone();
        self.stats.feed_sap(&mut self.table, &agent, stake_qug)
    }

    /// Machine-readable status — what the MCP tool and the web page render.
    pub fn status_json(&mut self, stake_qug: u64) -> serde_json::Value {
        let agent = self.agent.clone();
        let bench = self.stats.to_bench_result(&agent, stake_qug);
        let total = self.stats.feed_sap(&mut self.table, &agent, stake_qug);
        let components = self
            .table
            .get_full(&CaptureStats::peer_id(&agent))
            .map(|s| {
                serde_json::json!({
                    "contribution": s.components.contribution,
                    "latency": s.components.latency,
                    "stake": s.components.stake,
                    "accuracy": s.components.accuracy,
                    "uptime": s.components.uptime,
                    "rounds_participated": s.rounds_participated,
                })
            })
            .unwrap_or(serde_json::Value::Null);

        serde_json::json!({
            "agent": agent,
            "source": self.source.name(),
            "available": self.source.available(),
            "capture": {
                "attempts": self.stats.attempts,
                "successes": self.stats.successes,
                "failures": self.stats.failures,
                "integrity_failures": self.stats.integrity_failures,
                "success_rate": self.stats.success_rate(),
                "p50_ms": self.stats.p50_ms(),
                "p95_ms": self.stats.p95_ms(),
                "bytes_captured": self.stats.bytes_captured,
            },
            "last_frame": self.last.as_ref().map(|f| serde_json::json!({
                "hash": f.hash,
                "width": f.width,
                "height": f.height,
                "format": f.format.as_str(),
                "bytes": f.len(),
                "captured_at_ms": f.captured_at_ms,
            })),
            "sap": {
                "total": total,
                "dev_score_input": bench.dev_score,
                "components": components,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_engine_captures_and_scores_end_to_end() {
        let mut eng = WebcamEngine::synthetic(64, 64, "grogu");
        assert!(eng.source_available());

        for _ in 0..5 {
            let f = eng.capture().expect("synthetic capture must work headless");
            assert!(f.verify());
        }

        assert_eq!(eng.stats.attempts, 5);
        assert_eq!(eng.stats.successes, 5);
        assert_eq!(eng.stats.failures, 0);
        assert!(eng.stats.bytes_captured > 0);

        let sap = eng.sap(100);
        assert!((0.0..=1.0).contains(&sap), "SAP total must be normalised, got {sap}");
        assert!(sap > 0.0, "a perfect capture run must score above zero");
    }

    #[test]
    fn status_json_exposes_the_measured_fields() {
        let mut eng = WebcamEngine::synthetic(32, 32, "grogu");
        eng.capture().unwrap();
        let s = eng.status_json(50);

        assert_eq!(s["agent"], "grogu");
        assert_eq!(s["source"], "synthetic");
        assert_eq!(s["capture"]["successes"], 1);
        assert_eq!(s["capture"]["success_rate"], 1.0);
        assert!(s["last_frame"]["hash"].as_str().unwrap().len() == 64);
        assert_eq!(s["last_frame"]["format"], "png");
        assert!(s["sap"]["total"].as_f64().unwrap() > 0.0);
        assert!(s["sap"]["components"]["uptime"].as_f64().unwrap() > 0.0);
    }

    #[test]
    fn a_failing_source_lowers_sap_rather_than_erroring_out() {
        // A file source pointed at nothing: every capture fails, and the engine
        // must degrade its own score rather than panic or silently look healthy.
        let mut eng =
            WebcamEngine::new(Box::new(FileSource::new("/nope/missing.png")), "broken-agent");
        for _ in 0..4 {
            assert!(eng.capture().is_err());
        }
        assert_eq!(eng.stats.successes, 0);
        assert_eq!(eng.stats.failures, 4);

        // Not exactly zero, and that is correct: SAP still credits `accuracy`
        // (never having been caught lying) and `stake`. What must be true is
        // that the three components measuring actual *work* — contribution,
        // latency, uptime — are all floored, capping the total well below a
        // functioning capturer.
        let sap = eng.sap(100);
        let comps = eng
            .table
            .get_full(&CaptureStats::peer_id("broken-agent"))
            .expect("the agent must have a SAP row after being fed")
            .components
            .clone();
        assert_eq!(comps.contribution, 0.0, "delivered nothing");
        assert_eq!(comps.uptime, 0.0, "no successful rounds");
        assert_eq!(comps.latency, 0.0, "never answered, so not 'instant'");
        assert!(sap < 0.4, "a dead source must not look healthy, got {sap}");

        let mut healthy = WebcamEngine::synthetic(16, 16, "healthy-agent");
        for _ in 0..4 {
            healthy.capture().unwrap();
        }
        assert!(healthy.sap(100) > sap, "a working capturer must outscore a dead one");
    }

    #[test]
    fn capture_to_writes_a_real_file() {
        let mut eng = WebcamEngine::synthetic(24, 24, "grogu");
        let path = std::env::temp_dir().join("flux_webcam_engine_write.png");
        let hash = eng.capture_to(&path).expect("write should succeed");
        let written = std::fs::read(&path).unwrap();
        assert_eq!(blake3::hash(&written).to_hex().to_string(), hash);
        assert_eq!(&written[..4], &[0x89, b'P', b'N', b'G']);
        let _ = std::fs::remove_file(&path);
    }

    /// Human-facing measurement harness, not an assertion of quality.
    ///
    /// Run it with:
    /// `flux_webcam-<hash> sap_report_for_humans --exact --nocapture`
    ///
    /// It performs real captures, times them, and prints the resulting SAP
    /// breakdown so the number in any report is one that was *measured on this
    /// machine* rather than quoted from memory.
    #[test]
    fn sap_report_for_humans() {
        const CAPTURES: usize = 50;
        const STAKE_QUG: u64 = 100;

        let mut eng = WebcamEngine::synthetic(640, 480, "grogu");
        let wall = std::time::Instant::now();
        for _ in 0..CAPTURES {
            eng.capture().expect("synthetic capture");
        }
        let wall_ms = wall.elapsed().as_secs_f64() * 1000.0;

        // A second, deliberately unreliable engine so the score has a contrast.
        let mut broken = WebcamEngine::new(Box::new(FileSource::new("/nope.png")), "broken");
        for _ in 0..10 {
            let _ = broken.capture();
        }

        // And a real relay hop, so the p2p leg is measured too.
        let frame = eng.last_frame().cloned().expect("a frame");
        let announce = FrameAnnounce::for_frame("camera-box", &frame);
        let mut relay = FrameRelay::new();
        let hop = std::time::Instant::now();
        relay.accept(&announce, &frame, 0.0).expect("honest delivery");
        let hop_ms = hop.elapsed().as_secs_f64() * 1000.0;

        let status = eng.status_json(STAKE_QUG);
        println!("\n===== flux-webcam SAP REPORT (measured) =====");
        println!("{}", serde_json::to_string_pretty(&status).unwrap());
        println!("captures            : {CAPTURES}");
        println!("wall clock total    : {wall_ms:.2} ms");
        println!("relay verify hop    : {hop_ms:.4} ms");
        println!("broken-engine SAP   : {:.4}", broken.sap(STAKE_QUG));
        println!("frame bytes         : {}", frame.len());
        println!("=============================================\n");

        // Dual-purpose: when FLUX_WEBCAM_EMIT_DIR is set, drop the *measured*
        // status and a real captured frame there, so the web panel renders
        // actual data from this run rather than hand-written placeholders.
        if let Ok(dir) = std::env::var("FLUX_WEBCAM_EMIT_DIR") {
            let dir = std::path::PathBuf::from(dir);
            if std::fs::create_dir_all(&dir).is_ok() {
                let mut enriched = status.clone();
                enriched["measured"] = serde_json::json!({
                    "captures": CAPTURES,
                    "wall_ms": wall_ms,
                    "relay_hop_ms": hop_ms,
                    "broken_engine_sap": broken.sap(STAKE_QUG),
                    "relay_accepted": relay.accepted(),
                    "relay_rejected": relay.rejected(),
                    "best_providers": relay.best_providers(3),
                });
                let _ = std::fs::write(
                    dir.join("webcam-status.json"),
                    serde_json::to_string_pretty(&enriched).unwrap(),
                );
                let _ = std::fs::write(dir.join("frame.png"), &frame.data);
                println!("emitted fixtures to {}", dir.display());
            }
        }

        assert_eq!(eng.stats.successes as usize, CAPTURES);
    }

    #[test]
    fn engine_frames_relay_and_score_across_the_mesh() {
        // The full architecture in one test: capture here, announce, deliver,
        // verify on the other side, and score the delivering peer.
        let mut eng = WebcamEngine::synthetic(48, 48, "camera-box");
        let frame = eng.capture().unwrap();
        let announce = FrameAnnounce::for_frame("camera-box", &frame);

        let mut relay = FrameRelay::new();
        assert!(relay.accept(&announce, &frame, 7.5).is_ok());
        assert_eq!(relay.accepted(), 1);
        assert!(relay.peer_score("camera-box").unwrap() > 0.0);
    }
}
