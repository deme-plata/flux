//! Consent gate — default-deny access control for capture.
//!
//! # Threat model, stated honestly
//!
//! This gate defends against **an agent, a tool, or a bug capturing a frame
//! when the operator did not ask for one**. It does *not* — and cannot —
//! defend against an attacker who already has root on the host. Root can read
//! `/dev/video0` directly and never touch this code. Software cannot fix that.
//!
//! So the guarantees are layered, strongest first:
//!
//! | Layer | Stops | Bypassable by root? |
//! |---|---|---|
//! | Physical disconnect / lens cap | everything | **no** |
//! | `uvcvideo` unloaded, no `/dev/video*` | all software capture | yes, but noisily |
//! | Kill switch file | this crate | yes |
//! | Default-deny consent gate | this crate | yes |
//! | Hash-chained audit log | nothing — makes bypass *detectable* | tamper is detectable |
//!
//! **If you want a guarantee rather than a control, unplug the camera.** Every
//! layer below that is a control: it makes unauthorised capture default-denied
//! and, crucially, *leaves evidence*.
//!
//! # Design rules
//!
//! - **Fail closed, always.** A missing, unreadable, corrupt or unparseable
//!   grant is a DENY. There is no code path where an error results in ALLOW.
//! - **The grant is not issuable over MCP.** Nothing in this crate's MCP
//!   surface can mint consent; that is CLI-only, by the operator. An agent can
//!   *spend* consent and *revoke* it, never *create* it. Revoking is always
//!   safe to expose; granting never is.
//! - **Consent is spent before the frame is taken**, so a crash mid-capture
//!   cannot yield a free frame.
//! - **Confidence decays.** Beyond the hard expiry, a Fisher-information model
//!   (`flux-science`) shrinks confidence that the operator still means it. This
//!   can only ever *tighten* the decision, never extend a grant.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use flux_science::fisher::{ArrivalModel, StaleObservation};

/// No grant may last longer than this, whatever the caller asks for.
pub const MAX_GRANT_SECS: u64 = 3600;
/// No grant may authorise more than this many frames.
pub const MAX_GRANT_CAPTURES: u32 = 500;
/// Below this confidence the gate denies even inside the hard expiry window.
pub const MIN_CONFIDENCE: f64 = 0.25;
/// Intrinsic uncertainty σ² about intent at the moment of granting.
const INTENT_BASE_VARIANCE: f64 = 1.0;
/// λ — intent drift per second. σ²/λ = 300 s, so confidence halves in 5 minutes.
const INTENT_DRIFT_PER_S: f64 = 1.0 / 300.0;

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Where the gate keeps its state.
#[derive(Clone, Debug)]
pub struct ConsentPaths {
    pub root: PathBuf,
}

impl ConsentPaths {
    /// `FLUX_WEBCAM_HOME`, else `$HOME/.flux/webcam`, else `./.flux-webcam`.
    pub fn resolve() -> Self {
        let root = std::env::var("FLUX_WEBCAM_HOME")
            .map(PathBuf::from)
            .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".flux/webcam")))
            .unwrap_or_else(|_| PathBuf::from("./.flux-webcam"));
        ConsentPaths { root }
    }

    pub fn at(root: impl AsRef<Path>) -> Self {
        ConsentPaths { root: root.as_ref().to_path_buf() }
    }

    pub fn grant(&self) -> PathBuf {
        self.root.join("consent.json")
    }
    /// Presence of this file denies everything, unconditionally.
    pub fn kill_switch(&self) -> PathBuf {
        self.root.join("DENY")
    }
    pub fn audit(&self) -> PathBuf {
        self.root.join("audit.log")
    }
    /// High-water mark for the audit log: `<count>|<last_hash>`.
    ///
    /// Exists because a bare hash chain does **not** detect tail truncation.
    /// Prev-links catch edits and interior deletions, but lopping entries off
    /// the end leaves a shorter chain that still verifies perfectly — so a
    /// process that captured and then deleted its own trailing CAPTURE lines
    /// would look clean. Recording the expected length separately makes that
    /// visible. (Verified by test: `truncating_the_tail_is_detected`.)
    pub fn audit_head(&self) -> PathBuf {
        self.root.join("audit.head")
    }
}

/// A time-boxed, count-bounded authorisation to capture.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConsentGrant {
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
    pub max_captures: u32,
    pub used_captures: u32,
    /// Free text from the operator: why this was granted.
    pub reason: String,
    /// Random-ish token binding this grant; changes on every new grant so a
    /// replayed old file is distinguishable in the audit trail.
    pub nonce: String,
}

impl ConsentGrant {
    pub fn remaining(&self) -> u32 {
        self.max_captures.saturating_sub(self.used_captures)
    }

    pub fn age_s(&self, now_ms_val: u64) -> f64 {
        now_ms_val.saturating_sub(self.issued_at_ms) as f64 / 1000.0
    }

    /// Confidence that the operator still means it, via Fisher information.
    ///
    /// A grant is an observation of intent made `Δ` seconds ago. Its variance
    /// grows as `λΔ + σ²`, so its information `I = 1/(λΔ + σ²)` decays. The
    /// confidence reported here is the ratio `I(Δ)/I(0) = σ²/(λΔ + σ²)`, which
    /// starts at 1.0 and falls monotonically toward 0.
    ///
    /// This can only ever *reduce* access. It is never consulted to extend a
    /// grant past its hard expiry — [`ConsentGate::evaluate`] checks the wall
    /// clock first and returns before this is reached.
    pub fn confidence(&self, now_ms_val: u64) -> f64 {
        let model = ArrivalModel::new(INTENT_DRIFT_PER_S);
        let obs = StaleObservation {
            value: 1.0,
            staleness_s: self.age_s(now_ms_val),
            base_variance: INTENT_BASE_VARIANCE,
        };
        let info_now = obs.fisher_information(&model);
        let info_fresh = StaleObservation {
            value: 1.0,
            staleness_s: 0.0,
            base_variance: INTENT_BASE_VARIANCE,
        }
        .fisher_information(&model);
        if !info_fresh.is_finite() || info_fresh <= 0.0 {
            return 0.0; // fail closed
        }
        (info_now / info_fresh).clamp(0.0, 1.0)
    }
}

/// The gate's verdict. There is no third state — anything not `Allow` is a deny.
#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    Allow { remaining: u32, confidence: f64, expires_in_s: u64 },
    Deny { reason: DenyReason },
}

impl Decision {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Decision::Allow { .. })
    }

    pub fn reason_str(&self) -> String {
        match self {
            Decision::Allow { .. } => "allowed".into(),
            Decision::Deny { reason } => reason.to_string(),
        }
    }
}

// No `Eq`: `StaleIntent` carries an f64 confidence, and f64 is only PartialEq.
#[derive(Clone, Debug, PartialEq)]
pub enum DenyReason {
    /// The DENY file exists. Overrides everything.
    KillSwitch,
    /// No grant file at all — the default state.
    NoGrant,
    /// Present but unreadable or not valid JSON. Failing closed.
    Unreadable(String),
    Expired { ago_s: u64 },
    Exhausted { max: u32 },
    /// Inside the window but confidence has decayed too far.
    StaleIntent { confidence: f64 },
    /// Clock went backwards / grant issued in the future.
    ImplausibleClock,
}

impl std::fmt::Display for DenyReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DenyReason::KillSwitch => write!(f, "kill switch engaged (DENY file present)"),
            DenyReason::NoGrant => write!(f, "no consent grant — capture is default-denied"),
            DenyReason::Unreadable(e) => write!(f, "grant unreadable, failing closed: {e}"),
            DenyReason::Expired { ago_s } => write!(f, "grant expired {ago_s}s ago"),
            DenyReason::Exhausted { max } => write!(f, "grant exhausted ({max} captures used)"),
            DenyReason::StaleIntent { confidence } => {
                write!(f, "intent confidence {confidence:.3} below {MIN_CONFIDENCE} — re-grant to continue")
            }
            DenyReason::ImplausibleClock => write!(f, "grant timestamps implausible; refusing"),
        }
    }
}

/// Default-deny gate over the consent state on disk.
pub struct ConsentGate {
    pub paths: ConsentPaths,
}

impl ConsentGate {
    pub fn new(paths: ConsentPaths) -> Self {
        ConsentGate { paths }
    }

    pub fn resolve() -> Self {
        ConsentGate::new(ConsentPaths::resolve())
    }

    /// Read the current grant, if one parses.
    pub fn load(&self) -> Result<ConsentGrant, DenyReason> {
        let p = self.paths.grant();
        if !p.exists() {
            return Err(DenyReason::NoGrant);
        }
        let raw = std::fs::read_to_string(&p)
            .map_err(|e| DenyReason::Unreadable(e.to_string()))?;
        serde_json::from_str(&raw).map_err(|e| DenyReason::Unreadable(e.to_string()))
    }

    /// Decide whether a capture may proceed. Pure — spends nothing.
    pub fn evaluate(&self) -> Decision {
        // 1. Kill switch first, before anything else can matter.
        if self.paths.kill_switch().exists() {
            return Decision::Deny { reason: DenyReason::KillSwitch };
        }

        let grant = match self.load() {
            Ok(g) => g,
            Err(reason) => return Decision::Deny { reason },
        };

        let now = now_ms();

        // 2. Sanity: a grant from the future, or expiring before it was issued,
        //    means either a clock problem or a forged file. Refuse either way.
        if grant.issued_at_ms > now.saturating_add(60_000)
            || grant.expires_at_ms < grant.issued_at_ms
        {
            return Decision::Deny { reason: DenyReason::ImplausibleClock };
        }

        // 3. Hard wall-clock expiry.
        if now >= grant.expires_at_ms {
            return Decision::Deny {
                reason: DenyReason::Expired {
                    ago_s: (now - grant.expires_at_ms) / 1000,
                },
            };
        }

        // 4. Budget.
        if grant.remaining() == 0 {
            return Decision::Deny { reason: DenyReason::Exhausted { max: grant.max_captures } };
        }

        // 5. Only now the soft, decaying check — it can tighten, never extend.
        let confidence = grant.confidence(now);
        if confidence < MIN_CONFIDENCE {
            return Decision::Deny { reason: DenyReason::StaleIntent { confidence } };
        }

        Decision::Allow {
            remaining: grant.remaining(),
            confidence,
            expires_in_s: (grant.expires_at_ms - now) / 1000,
        }
    }

    /// Spend one capture. Call **before** taking the frame.
    ///
    /// Returns the decision that was in force. On `Allow` the counter has been
    /// incremented and persisted; if persisting fails the call degrades to a
    /// deny rather than handing out an unmetered capture.
    pub fn consume(&self) -> Decision {
        let decision = self.evaluate();
        if !decision.is_allowed() {
            self.audit("DENY", &decision.reason_str());
            return decision;
        }

        let mut grant = match self.load() {
            Ok(g) => g,
            Err(reason) => {
                self.audit("DENY", &reason.to_string());
                return Decision::Deny { reason };
            }
        };
        grant.used_captures = grant.used_captures.saturating_add(1);

        match self.persist(&grant) {
            Ok(()) => {
                self.audit(
                    "CAPTURE",
                    &format!("remaining={} nonce={}", grant.remaining(), grant.nonce),
                );
                decision
            }
            Err(e) => {
                let reason = DenyReason::Unreadable(format!("could not record spend: {e}"));
                self.audit("DENY", &reason.to_string());
                Decision::Deny { reason }
            }
        }
    }

    fn persist(&self, grant: &ConsentGrant) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.paths.root)?;
        let json = serde_json::to_string_pretty(grant)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        // Write-then-rename so a crash cannot leave a half-written grant that
        // parses as something more permissive.
        let tmp = self.paths.grant().with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, self.paths.grant())
    }

    /// Issue a grant. **Deliberately not reachable from MCP** — operator only.
    pub fn grant(
        &self,
        duration_s: u64,
        max_captures: u32,
        reason: &str,
    ) -> std::io::Result<ConsentGrant> {
        let duration_s = duration_s.clamp(1, MAX_GRANT_SECS);
        let max_captures = max_captures.clamp(1, MAX_GRANT_CAPTURES);
        let now = now_ms();
        // Nonce from the clock + the address of a fresh allocation; enough to
        // distinguish successive grants in the audit trail without adding an RNG
        // dependency to a security-relevant path.
        let salt = Box::new(0u8);
        let nonce = blake3::hash(
            format!("{now}-{:p}-{reason}", Box::as_ref(&salt) as *const u8).as_bytes(),
        )
        .to_hex()
        .to_string()[..16]
            .to_string();

        let grant = ConsentGrant {
            issued_at_ms: now,
            expires_at_ms: now + duration_s * 1000,
            max_captures,
            used_captures: 0,
            reason: reason.to_string(),
            nonce: nonce.clone(),
        };
        // Granting clears a previously-engaged kill switch only if the operator
        // removes it themselves — we never delete DENY on their behalf.
        self.persist(&grant)?;
        self.audit(
            "GRANT",
            &format!("duration={duration_s}s max={max_captures} nonce={nonce} reason={reason}"),
        );
        Ok(grant)
    }

    /// Revoke immediately. Safe to expose anywhere, including MCP — it can only
    /// reduce access.
    pub fn revoke(&self, who: &str) -> std::io::Result<()> {
        let p = self.paths.grant();
        if p.exists() {
            std::fs::remove_file(&p)?;
        }
        self.audit("REVOKE", who);
        Ok(())
    }

    /// Engage the kill switch: deny everything until the operator clears it.
    pub fn engage_kill_switch(&self, who: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.paths.root)?;
        std::fs::write(self.paths.kill_switch(), format!("engaged by {who}\n"))?;
        self.audit("KILL_SWITCH_ON", who);
        Ok(())
    }

    /// Clear the kill switch. Operator action; note it still leaves no grant
    /// behind, so capture stays denied until one is explicitly issued.
    pub fn clear_kill_switch(&self, who: &str) -> std::io::Result<()> {
        let p = self.paths.kill_switch();
        if p.exists() {
            std::fs::remove_file(&p)?;
        }
        self.audit("KILL_SWITCH_OFF", who);
        Ok(())
    }

    // ---------------------------------------------------------------------
    // Audit log — append-only, BLAKE3-chained
    // ---------------------------------------------------------------------

    /// Append `event` to the tamper-evident log.
    ///
    /// Each line is `seq|ts_ms|event|detail|prev_hash|hash`, where `hash`
    /// commits to every earlier field including `prev_hash`. Editing or
    /// deleting any line breaks the chain from that point on, which
    /// [`ConsentGate::verify_audit`] detects. This does not *prevent* tampering
    /// — nothing in software does against root — it makes it evident.
    pub fn audit(&self, event: &str, detail: &str) {
        let _ = std::fs::create_dir_all(&self.paths.root);
        let (seq, prev) = self.audit_tail();
        let ts = now_ms();
        let detail = detail.replace('|', "/").replace('\n', " ");
        let body = format!("{seq}|{ts}|{event}|{detail}|{prev}");
        let hash = blake3::hash(body.as_bytes()).to_hex().to_string();
        let line = format!("{body}|{hash}\n");
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.paths.audit())
        {
            let _ = f.write_all(line.as_bytes());
        }
        // Advance the high-water mark. Written after the append so a crash
        // between the two understates the count (which reads as truncation and
        // fails loudly) rather than overstating it (which would hide an entry).
        let _ = std::fs::write(self.paths.audit_head(), format!("{}|{hash}", seq + 1));
    }

    /// `(expected_count, expected_last_hash)` from the high-water mark, if present.
    fn audit_head(&self) -> Option<(u64, String)> {
        let raw = std::fs::read_to_string(self.paths.audit_head()).ok()?;
        let (count, hash) = raw.trim().split_once('|')?;
        Some((count.parse().ok()?, hash.to_string()))
    }

    /// `(next_seq, last_hash)`.
    fn audit_tail(&self) -> (u64, String) {
        let genesis = "0".repeat(64);
        let Ok(content) = std::fs::read_to_string(self.paths.audit()) else {
            return (0, genesis);
        };
        match content.lines().filter(|l| !l.trim().is_empty()).next_back() {
            Some(last) => {
                let parts: Vec<&str> = last.split('|').collect();
                if parts.len() == 6 {
                    let seq = parts[0].parse::<u64>().unwrap_or(0);
                    (seq + 1, parts[5].to_string())
                } else {
                    (0, genesis)
                }
            }
            None => (0, genesis),
        }
    }

    /// Verify the whole chain. `Ok(n)` = n entries all consistent.
    /// `Err(seq)` = the first line whose hash does not check out.
    pub fn verify_audit(&self) -> Result<u64, u64> {
        let Ok(content) = std::fs::read_to_string(self.paths.audit()) else {
            return Ok(0);
        };
        let mut prev = "0".repeat(64);
        let mut count = 0u64;
        for line in content.lines().filter(|l| !l.trim().is_empty()) {
            let parts: Vec<&str> = line.split('|').collect();
            if parts.len() != 6 {
                return Err(count);
            }
            let body = parts[..5].join("|");
            let expect = blake3::hash(body.as_bytes()).to_hex().to_string();
            if expect != parts[5] || parts[4] != prev {
                return Err(count);
            }
            prev = parts[5].to_string();
            count += 1;
        }

        // The chain is internally consistent. Now the part a chain alone cannot
        // tell you: is anything missing from the END? Compare against the
        // high-water mark. A log shorter than its own recorded length has been
        // truncated, even though every surviving link verifies.
        if let Some((expected, expected_hash)) = self.audit_head() {
            if count < expected || (count == expected && prev != expected_hash && count > 0) {
                // Report the last surviving entry so the caller can say how much
                // is gone.
                return Err(count);
            }
        }
        Ok(count)
    }

    /// How many entries the log *should* contain, per the high-water mark.
    /// `None` when no mark exists yet (a fresh store).
    pub fn expected_audit_len(&self) -> Option<u64> {
        self.audit_head().map(|(n, _)| n)
    }

    /// Human-readable status for the MCP surface and the panel.
    pub fn status_json(&self) -> serde_json::Value {
        let decision = self.evaluate();
        let grant = self.load().ok();
        let now = now_ms();
        serde_json::json!({
            "allowed": decision.is_allowed(),
            "reason": decision.reason_str(),
            "kill_switch": self.paths.kill_switch().exists(),
            "grant": grant.as_ref().map(|g| serde_json::json!({
                "issued_at_ms": g.issued_at_ms,
                "expires_at_ms": g.expires_at_ms,
                "expires_in_s": (g.expires_at_ms.saturating_sub(now)) / 1000,
                "age_s": g.age_s(now),
                "max_captures": g.max_captures,
                "used_captures": g.used_captures,
                "remaining": g.remaining(),
                "confidence": g.confidence(now),
                "reason": g.reason,
                "nonce": g.nonce,
            })),
            "audit": match self.verify_audit() {
                Ok(n) => serde_json::json!({ "entries": n, "intact": true }),
                Err(at) => serde_json::json!({ "entries": at, "intact": false, "broken_at": at }),
            },
            "policy": {
                "default": "deny",
                "max_grant_secs": MAX_GRANT_SECS,
                "max_grant_captures": MAX_GRANT_CAPTURES,
                "min_confidence": MIN_CONFIDENCE,
                "grant_over_mcp": false,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_gate(name: &str) -> ConsentGate {
        let dir = std::env::temp_dir().join(format!("flux_webcam_consent_{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        ConsentGate::new(ConsentPaths::at(dir))
    }

    #[test]
    fn default_state_is_deny() {
        let g = tmp_gate("default");
        assert_eq!(g.evaluate(), Decision::Deny { reason: DenyReason::NoGrant });
    }

    #[test]
    fn a_grant_allows_then_exhausts() {
        let g = tmp_gate("exhaust");
        g.grant(600, 2, "yoga").unwrap();
        assert!(g.consume().is_allowed());
        assert!(g.consume().is_allowed());
        let third = g.consume();
        assert!(!third.is_allowed());
        assert!(matches!(third, Decision::Deny { reason: DenyReason::Exhausted { .. } }));
    }

    #[test]
    fn kill_switch_overrides_a_valid_grant() {
        let g = tmp_gate("kill");
        g.grant(600, 100, "testing").unwrap();
        assert!(g.evaluate().is_allowed());
        g.engage_kill_switch("viktor").unwrap();
        assert_eq!(g.evaluate(), Decision::Deny { reason: DenyReason::KillSwitch });
        // And clearing it does NOT silently re-open: the grant is still there,
        // but the operator had to act to clear the switch.
        g.clear_kill_switch("viktor").unwrap();
        assert!(g.evaluate().is_allowed());
    }

    #[test]
    fn expired_grant_denies() {
        let g = tmp_gate("expired");
        let mut grant = g.grant(600, 10, "x").unwrap();
        // Model a grant that was genuinely issued in the past and has since run
        // out: issued 10s ago, expired 5s ago. Moving ONLY `expires_at` into the
        // past would leave `expires_at < issued_at`, which is a forged/skewed
        // grant and is (correctly) refused by the ImplausibleClock guard before
        // expiry is ever considered — so that fixture tests a different branch
        // than the one it names.
        let now = now_ms();
        grant.issued_at_ms = now - 10_000;
        grant.expires_at_ms = now - 5_000;
        g.persist(&grant).unwrap();
        assert!(
            matches!(g.evaluate(), Decision::Deny { reason: DenyReason::Expired { .. } }),
            "a lapsed grant must deny with Expired, got {:?}",
            g.evaluate()
        );
    }

    #[test]
    fn a_grant_that_expires_before_it_was_issued_is_refused_as_forged() {
        // The branch the old `expired_grant_denies` fixture was accidentally
        // exercising. Worth its own test: it is the forged-file / clock-skew
        // guard, and it must deny rather than fall through to any softer check.
        let g = tmp_gate("forged");
        let mut grant = g.grant(600, 10, "x").unwrap();
        grant.expires_at_ms = grant.issued_at_ms - 1;
        g.persist(&grant).unwrap();
        assert!(matches!(
            g.evaluate(),
            Decision::Deny { reason: DenyReason::ImplausibleClock }
        ));
    }

    #[test]
    fn corrupt_grant_fails_closed() {
        let g = tmp_gate("corrupt");
        std::fs::create_dir_all(&g.paths.root).unwrap();
        std::fs::write(g.paths.grant(), "{ this is not json").unwrap();
        assert!(matches!(
            g.evaluate(),
            Decision::Deny { reason: DenyReason::Unreadable(_) }
        ));
    }

    #[test]
    fn future_dated_grant_is_refused() {
        let g = tmp_gate("future");
        let mut grant = g.grant(600, 10, "x").unwrap();
        grant.issued_at_ms = now_ms() + 10 * 60 * 1000;
        grant.expires_at_ms = grant.issued_at_ms + 600_000;
        g.persist(&grant).unwrap();
        assert_eq!(g.evaluate(), Decision::Deny { reason: DenyReason::ImplausibleClock });
    }

    #[test]
    fn revoke_is_immediate() {
        let g = tmp_gate("revoke");
        g.grant(600, 10, "x").unwrap();
        assert!(g.evaluate().is_allowed());
        g.revoke("grogu").unwrap();
        assert_eq!(g.evaluate(), Decision::Deny { reason: DenyReason::NoGrant });
    }

    #[test]
    fn grant_caps_are_enforced() {
        let g = tmp_gate("caps");
        let grant = g.grant(u64::MAX, u32::MAX, "greedy").unwrap();
        assert_eq!(grant.max_captures, MAX_GRANT_CAPTURES);
        assert!(grant.expires_at_ms - grant.issued_at_ms <= MAX_GRANT_SECS * 1000);
    }

    #[test]
    fn confidence_decays_monotonically_with_age() {
        let g = tmp_gate("decay");
        let grant = g.grant(MAX_GRANT_SECS, 100, "x").unwrap();
        let t0 = grant.issued_at_ms;
        let fresh = grant.confidence(t0);
        let five_min = grant.confidence(t0 + 300_000);
        let hour = grant.confidence(t0 + 3_600_000);
        assert!((fresh - 1.0).abs() < 1e-9, "a fresh grant is full confidence");
        assert!((five_min - 0.5).abs() < 0.01, "half-life is 300s, got {five_min}");
        assert!(hour < five_min && hour < 0.1, "confidence must keep decaying");
    }

    #[test]
    fn stale_intent_denies_even_inside_the_window() {
        let g = tmp_gate("stale");
        // A long grant, but backdated so intent confidence has decayed away.
        let mut grant = g.grant(MAX_GRANT_SECS, 100, "old").unwrap();
        grant.issued_at_ms = now_ms() - 3_000_000; // 50 min ago
        grant.expires_at_ms = now_ms() + 600_000; // still nominally valid
        g.persist(&grant).unwrap();
        match g.evaluate() {
            Decision::Deny { reason: DenyReason::StaleIntent { confidence } } => {
                assert!(confidence < MIN_CONFIDENCE);
            }
            other => panic!("expected StaleIntent deny, got {other:?}"),
        }
    }

    #[test]
    fn confidence_never_extends_a_grant() {
        // Even at full confidence, hard expiry wins. Ordering matters.
        let g = tmp_gate("noextend");
        let mut grant = g.grant(600, 10, "x").unwrap();
        grant.issued_at_ms = now_ms(); // maximal confidence
        grant.expires_at_ms = now_ms(); // but already expired
        g.persist(&grant).unwrap();
        assert!(matches!(
            g.evaluate(),
            Decision::Deny { reason: DenyReason::Expired { .. } }
        ));
    }

    #[test]
    fn truncating_the_tail_is_detected() {
        // Regression guard for a real gap found by hand-testing the shipped
        // binary: a prev-linked hash chain detects edits and interior deletions,
        // but NOT entries lopped off the end — the survivors still link
        // perfectly. Before the high-water mark existed, deleting the last line
        // reported "chain intact", which would let a process capture and then
        // erase its own CAPTURE records.
        let g = tmp_gate("truncate");
        g.audit("ONE", "a");
        g.audit("TWO", "b");
        g.audit("THREE", "c");
        assert_eq!(g.verify_audit(), Ok(3));
        assert_eq!(g.expected_audit_len(), Some(3));

        // Lop off the last entry, leaving a perfectly-linked 2-entry chain.
        let log = std::fs::read_to_string(g.paths.audit()).unwrap();
        let kept: Vec<&str> = log.lines().filter(|l| !l.trim().is_empty()).take(2).collect();
        std::fs::write(g.paths.audit(), format!("{}\n", kept.join("\n"))).unwrap();

        assert!(
            g.verify_audit().is_err(),
            "a truncated log must NOT report as intact — every surviving link still verifies, \
             which is exactly why the length has to be checked separately"
        );
    }

    #[test]
    fn interior_deletion_is_detected_by_the_chain_itself() {
        let g = tmp_gate("interior");
        g.audit("ONE", "a");
        g.audit("TWO", "b");
        g.audit("THREE", "c");
        let log = std::fs::read_to_string(g.paths.audit()).unwrap();
        let lines: Vec<&str> = log.lines().filter(|l| !l.trim().is_empty()).collect();
        // Drop the middle entry; the third's prev-link no longer matches.
        std::fs::write(g.paths.audit(), format!("{}\n{}\n", lines[0], lines[2])).unwrap();
        assert!(g.verify_audit().is_err(), "a broken prev-link must be caught");
    }

    #[test]
    fn audit_chain_verifies_and_detects_tampering() {
        let g = tmp_gate("audit");
        g.grant(600, 5, "yoga").unwrap();
        g.consume();
        g.consume();
        g.revoke("grogu").unwrap();
        let n = g.verify_audit().expect("chain should be intact");
        assert!(n >= 4, "grant + 2 captures + revoke, got {n}");

        // Rewrite history: blank out a capture line's detail.
        let content = std::fs::read_to_string(g.paths.audit()).unwrap();
        let tampered: Vec<String> = content
            .lines()
            .map(|l| if l.contains("|CAPTURE|") { l.replace("CAPTURE", "NOTHING") } else { l.to_string() })
            .collect();
        std::fs::write(g.paths.audit(), tampered.join("\n") + "\n").unwrap();
        assert!(g.verify_audit().is_err(), "editing the log must break the chain");
    }

    #[test]
    fn denials_are_recorded_not_silent() {
        let g = tmp_gate("denylog");
        g.consume(); // no grant -> deny
        let log = std::fs::read_to_string(g.paths.audit()).unwrap();
        assert!(log.contains("|DENY|"), "a refused capture must leave a trace");
    }

    #[test]
    fn status_json_advertises_that_mcp_cannot_grant() {
        let g = tmp_gate("status");
        let s = g.status_json();
        assert_eq!(s["policy"]["default"], "deny");
        assert_eq!(s["policy"]["grant_over_mcp"], false);
        assert_eq!(s["allowed"], false);
    }
}
