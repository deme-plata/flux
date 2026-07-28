//! The only capture path that should ever be exposed to an agent.
//!
//! [`SecureWebcam`] wraps a [`WebcamEngine`] so that **no frame can be produced
//! without spending consent first**. The bare `WebcamEngine` remains available
//! for tests and for the operator's own CLI, but the MCP surface binds to this
//! type exclusively.
//!
//! Order of operations is deliberate and load-bearing:
//!
//! 1. Evaluate the gate.
//! 2. If denied — audit and return. **The source is never touched.** No device
//!    is opened, no capture command spawned, so a denial cannot produce a frame
//!    anywhere, even transiently in a temp file.
//! 3. Spend the consent (persisted to disk).
//! 4. Only then capture.
//!
//! Steps 3 and 4 are in that order so that a crash between them costs a
//! capture from the budget rather than granting a free one.

use crate::consent::{ConsentGate, Decision};
use crate::frame::Frame;
use crate::source::CaptureError;
use crate::WebcamEngine;
use std::path::Path;

/// Why a guarded capture did not produce a frame.
#[derive(Debug)]
pub enum SecureError {
    /// The consent gate said no. Carries the human-readable reason.
    Denied(String),
    /// Consent was spent but the capture itself failed.
    Capture(CaptureError),
}

impl std::fmt::Display for SecureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecureError::Denied(r) => write!(f, "denied: {r}"),
            SecureError::Capture(e) => write!(f, "capture failed: {e}"),
        }
    }
}

impl std::error::Error for SecureError {}

/// A capture engine that cannot be used without consent.
pub struct SecureWebcam {
    engine: WebcamEngine,
    gate: ConsentGate,
}

impl SecureWebcam {
    pub fn new(engine: WebcamEngine, gate: ConsentGate) -> Self {
        SecureWebcam { engine, gate }
    }

    /// Resolve the gate from the environment (`FLUX_WEBCAM_HOME`).
    pub fn with_env_gate(engine: WebcamEngine) -> Self {
        SecureWebcam { engine, gate: ConsentGate::resolve() }
    }

    pub fn gate(&self) -> &ConsentGate {
        &self.gate
    }

    /// Read-only: may a capture proceed right now? Spends nothing.
    pub fn peek(&self) -> Decision {
        self.gate.evaluate()
    }

    /// Capture one frame, if and only if consent permits.
    pub fn capture(&mut self) -> Result<Frame, SecureError> {
        // Spend first. `consume` audits both outcomes internally.
        let decision = self.gate.consume();
        if !decision.is_allowed() {
            // Note what did NOT happen: the source was never asked for a frame.
            return Err(SecureError::Denied(decision.reason_str()));
        }
        self.engine.capture().map_err(|e| {
            self.gate.audit("CAPTURE_FAILED", &e.to_string());
            SecureError::Capture(e)
        })
    }

    /// Capture straight to a file. Returns the frame hash.
    pub fn capture_to(&mut self, path: impl AsRef<Path>) -> Result<String, SecureError> {
        let frame = self.capture()?;
        std::fs::write(path.as_ref(), &frame.data)
            .map_err(|e| SecureError::Capture(CaptureError::Io(e)))?;
        Ok(frame.hash)
    }

    /// Revoke consent. Safe for an agent to call — it can only reduce access.
    pub fn revoke(&self, who: &str) -> std::io::Result<()> {
        self.gate.revoke(who)
    }

    /// Engage the kill switch. Also safe for an agent — deny-only.
    pub fn panic_stop(&self, who: &str) -> std::io::Result<()> {
        self.gate.engage_kill_switch(who)
    }

    /// Combined consent + capture status for the MCP surface.
    pub fn status_json(&mut self, stake_qug: u64) -> serde_json::Value {
        let mut consent = self.gate.status_json();
        consent["engine"] = self.engine.status_json(stake_qug);
        consent
    }

    /// Borrow the inner engine read-only (telemetry, last frame metadata).
    pub fn engine(&self) -> &WebcamEngine {
        &self.engine
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consent::{ConsentPaths, DenyReason};
    use crate::source::{FrameSource, SyntheticSource};

    struct CountingSource {
        inner: SyntheticSource,
        pub calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl FrameSource for CountingSource {
        fn name(&self) -> String {
            "counting".into()
        }
        fn capture(&mut self) -> crate::source::CaptureResult {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.capture()
        }
    }

    fn secure(name: &str) -> (SecureWebcam, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        let dir = std::env::temp_dir().join(format!("flux_webcam_secure_{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let src = CountingSource { inner: SyntheticSource::new(16, 16), calls: calls.clone() };
        let engine = WebcamEngine::new(Box::new(src), "grogu");
        let gate = ConsentGate::new(ConsentPaths::at(dir));
        (SecureWebcam::new(engine, gate), calls)
    }

    #[test]
    fn capture_is_denied_by_default() {
        let (mut cam, calls) = secure("default");
        let err = cam.capture().unwrap_err();
        assert!(matches!(err, SecureError::Denied(_)));
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "THE CRITICAL PROPERTY: a denied capture must never touch the source"
        );
    }

    #[test]
    fn a_grant_permits_exactly_the_budget() {
        let (mut cam, calls) = secure("budget");
        cam.gate().grant(600, 3, "yoga session").unwrap();
        assert!(cam.capture().is_ok());
        assert!(cam.capture().is_ok());
        assert!(cam.capture().is_ok());
        assert!(cam.capture().is_err(), "the 4th must be refused");
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "the source must be touched exactly as many times as consent allowed"
        );
    }

    #[test]
    fn revoke_mid_session_stops_capture_immediately() {
        let (mut cam, calls) = secure("revoke");
        cam.gate().grant(600, 50, "session").unwrap();
        assert!(cam.capture().is_ok());
        cam.revoke("grogu").unwrap();
        assert!(cam.capture().is_err());
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn kill_switch_beats_a_live_grant() {
        let (mut cam, calls) = secure("kill");
        cam.gate().grant(600, 50, "session").unwrap();
        cam.panic_stop("viktor").unwrap();
        assert!(cam.capture().is_err());
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(matches!(cam.peek(), Decision::Deny { reason: DenyReason::KillSwitch }));
    }

    #[test]
    fn peek_does_not_spend_consent() {
        let (mut cam, _) = secure("peek");
        cam.gate().grant(600, 1, "one shot").unwrap();
        for _ in 0..10 {
            assert!(cam.peek().is_allowed());
        }
        assert!(cam.capture().is_ok(), "the single capture must still be available");
        assert!(cam.capture().is_err());
    }

    #[test]
    fn every_capture_and_denial_is_audited() {
        let (mut cam, _) = secure("audit");
        cam.gate().grant(600, 1, "x").unwrap();
        let _ = cam.capture();
        let _ = cam.capture(); // denied
        let log = std::fs::read_to_string(cam.gate().paths.audit()).unwrap();
        assert!(log.contains("|GRANT|"));
        assert!(log.contains("|CAPTURE|"));
        assert!(log.contains("|DENY|"));
        assert!(cam.gate().verify_audit().is_ok(), "chain must stay intact");
    }

    #[test]
    fn status_reports_both_consent_and_engine() {
        let (mut cam, _) = secure("status");
        cam.gate().grant(600, 5, "yoga").unwrap();
        cam.capture().unwrap();
        let s = cam.status_json(10);
        assert_eq!(s["allowed"], true);
        assert_eq!(s["policy"]["grant_over_mcp"], false);
        assert_eq!(s["grant"]["remaining"], 4);
        assert_eq!(s["engine"]["capture"]["successes"], 1);
    }
}
