//! Replay protection for the secret push bus.
//!
//! A gossipsub frame can be captured and re-published. Decryption alone does not
//! stop that — the ciphertext is still valid. This guard rejects a frame seen
//! twice, and bounds its own memory with a freshness window.
//!
//! Two integrity-bound facts make it sound:
//!   * the dedup key is a BLAKE3 fingerprint of the **envelope** (`eph_pk ‖ ct`),
//!     which cannot be altered without breaking AEAD `open`;
//!   * the freshness check uses `ts_ms`, which for authenticated frames is bound
//!     into the signature (see [`auth::seal_authed_frame`](crate::auth::seal_authed_frame))
//!     so a man-in-the-middle cannot rewrite it to dodge eviction.
//!
//! The two combine to close the eviction gap: once a fingerprint ages past the
//! window and is pruned, a replay of it carries the *same old* (authenticated)
//! `ts_ms`, so the freshness check rejects it as [`ReplayError::Stale`].

use crate::SealedEnvelope;
use std::collections::HashMap;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ReplayError {
    #[error("replayed frame (envelope fingerprint already seen within the window)")]
    Replay,
    #[error("stale frame: ts_ms={ts_ms} is older than the window relative to now={now_ms}")]
    Stale { ts_ms: u64, now_ms: u64 },
    #[error("frame timestamp from the future: ts_ms={ts_ms} > now={now_ms} + skew")]
    Future { ts_ms: u64, now_ms: u64 },
}

/// Sliding-window dedup of envelope fingerprints.
pub struct ReplayGuard {
    window_ms: u64,
    max_skew_ms: u64,
    seen: HashMap<[u8; 32], u64>, // fingerprint -> ts_ms
}

impl ReplayGuard {
    pub fn new(window_ms: u64, max_skew_ms: u64) -> Self {
        Self { window_ms, max_skew_ms, seen: HashMap::new() }
    }

    /// 5-minute accept window, 60s allowed clock skew.
    pub fn with_defaults() -> Self {
        Self::new(300_000, 60_000)
    }

    /// Integrity-bound dedup key: BLAKE3 over the envelope's ephemeral key + ciphertext.
    pub fn fingerprint(env: &SealedEnvelope) -> [u8; 32] {
        let mut h = blake3::Hasher::new();
        h.update(env.eph_pk.as_bytes());
        h.update(b"|");
        h.update(env.ct.as_bytes());
        *h.finalize().as_bytes()
    }

    pub fn len(&self) -> usize {
        self.seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }

    fn prune(&mut self, now_ms: u64) {
        let cutoff = now_ms.saturating_sub(self.window_ms);
        self.seen.retain(|_, ts| *ts >= cutoff);
    }

    /// Admit `(env, ts_ms)` observed at `now_ms`. `Ok(())` the first time within
    /// the window; `Err` on replay, stale, or future. For authenticated frames,
    /// pass the verified `ts_ms` (the one bound into the signature).
    pub fn admit(
        &mut self,
        env: &SealedEnvelope,
        ts_ms: u64,
        now_ms: u64,
    ) -> Result<(), ReplayError> {
        self.prune(now_ms);
        if ts_ms > now_ms.saturating_add(self.max_skew_ms) {
            return Err(ReplayError::Future { ts_ms, now_ms });
        }
        if ts_ms.saturating_add(self.window_ms) < now_ms {
            return Err(ReplayError::Stale { ts_ms, now_ms });
        }
        let fp = Self::fingerprint(env);
        if self.seen.contains_key(&fp) {
            return Err(ReplayError::Replay);
        }
        self.seen.insert(fp, ts_ms);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{seal, SecretIdentity};

    fn env_for(msg: &[u8]) -> SealedEnvelope {
        let bob = SecretIdentity::generate();
        seal(msg, &bob.public_key())
    }

    #[test]
    fn first_admit_ok_replay_rejected() {
        let mut g = ReplayGuard::with_defaults();
        let env = env_for(b"once");
        assert!(g.admit(&env, 1_000, 1_000).is_ok());
        assert_eq!(g.admit(&env, 1_000, 1_001), Err(ReplayError::Replay));
    }

    #[test]
    fn distinct_envelopes_both_admit() {
        let mut g = ReplayGuard::with_defaults();
        assert!(g.admit(&env_for(b"a"), 1_000, 1_000).is_ok());
        assert!(g.admit(&env_for(b"b"), 1_000, 1_000).is_ok());
        assert_eq!(g.len(), 2);
    }

    #[test]
    fn stale_frame_rejected() {
        let mut g = ReplayGuard::new(300_000, 60_000);
        let env = env_for(b"old");
        // ts is 400s before now, window is 300s → stale
        assert_eq!(
            g.admit(&env, 600_000, 1_000_000),
            Err(ReplayError::Stale { ts_ms: 600_000, now_ms: 1_000_000 })
        );
    }

    #[test]
    fn future_frame_rejected() {
        let mut g = ReplayGuard::new(300_000, 60_000);
        let env = env_for(b"ahead");
        // ts is 120s ahead of now, skew is 60s → future
        assert_eq!(
            g.admit(&env, 1_120_000, 1_000_000),
            Err(ReplayError::Future { ts_ms: 1_120_000, now_ms: 1_000_000 })
        );
    }

    #[test]
    fn eviction_then_replay_is_caught_by_freshness() {
        // Admit at t=1000; advance now well past the window so the fp is pruned;
        // a replay carries the SAME old ts → rejected as Stale, not silently re-admitted.
        let mut g = ReplayGuard::new(300_000, 60_000);
        let env = env_for(b"evict-me");
        assert!(g.admit(&env, 1_000, 1_000).is_ok());
        let later = 1_000 + 300_000 + 1; // past the window
        assert!(matches!(g.admit(&env, 1_000, later), Err(ReplayError::Stale { .. })));
        // pruning kept memory bounded
        assert_eq!(g.len(), 0);
    }

    #[test]
    fn fingerprint_changes_with_any_byte() {
        let a = env_for(b"x");
        let mut b = a.clone();
        let mut ct = hex::decode(&b.ct).unwrap();
        ct[0] ^= 0x01;
        b.ct = hex::encode(ct);
        assert_ne!(ReplayGuard::fingerprint(&a), ReplayGuard::fingerprint(&b));
    }
}
