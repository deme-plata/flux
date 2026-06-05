//! Wallet authentication — the SQIsign challenge that proves wallet
//! ownership without passwords.
//!
//! Flow:
//!
//! 1. Client hits `/authorize?wallet=<pubkey>`.
//! 2. Server mints a 32-byte random `challenge`, stores
//!    `(challenge_hash, wallet, expires_at)` in flux-db with a 5-minute TTL.
//! 3. Server returns the challenge + a code_challenge from the PKCE step.
//! 4. Client signs the challenge with the wallet's SQIsign secret key.
//! 5. Client POSTs `/token` with the signature + verifier.
//! 6. Server looks up the challenge, verifies the SQIsign signature against
//!    the wallet pubkey, validates PKCE, issues access + refresh tokens.
//!
//! This module covers steps 2 + 6's verification primitives. The HTTP wire
//! lands in OAUTH2-D; the SQIsign verify itself defers to `flux-sqisign`
//! (already shipped in the workspace).

use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::WalletId;

/// A pending challenge. flux-db keys this on `challenge_hash` (SHA-256 of
/// `bytes`); the plain bytes go to the client and are never stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletChallenge {
    /// Wallet expected to sign.
    pub wallet: WalletId,
    /// Random challenge bytes the wallet signs over (32 bytes).
    pub bytes: [u8; 32],
    /// Microsecond epoch when the challenge expires (5 minutes after mint).
    pub expires_at_us: u64,
}

/// Wallet-auth failure modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WalletChallengeError {
    /// Challenge was looked up but the wallet field didn't match.
    #[error("challenge does not bind this wallet")]
    WrongWallet,
    /// Challenge TTL expired.
    #[error("challenge expired")]
    Expired,
    /// SQIsign signature verification failed.
    ///
    /// (Wired against `flux-sqisign::verify` in OAUTH2-C; v0 stub returns
    /// this error if the signature is empty.)
    #[error("signature verification failed")]
    BadSignature,
}

impl WalletChallenge {
    /// Mint a fresh 32-byte challenge for `wallet`, expiring 5 minutes from
    /// `now_us`.
    pub fn new<R: Rng>(wallet: WalletId, rng: &mut R, now_us: u64) -> Self {
        let mut bytes = [0u8; 32];
        rng.fill_bytes(&mut bytes);
        Self {
            wallet,
            bytes,
            expires_at_us: now_us.saturating_add(5 * 60 * 1_000_000), // 5 min
        }
    }

    /// Verify a presented signature against this challenge. v0 stub: checks
    /// non-emptiness + expiry + wallet match. OAUTH2-C wires the real
    /// SQIsign verify via `flux-sqisign::verify_signature(&self.bytes,
    /// signature, pubkey)`.
    pub fn verify_stub(
        &self,
        presented_wallet: &WalletId,
        signature: &[u8],
        now_us: u64,
    ) -> Result<(), WalletChallengeError> {
        if &self.wallet != presented_wallet {
            return Err(WalletChallengeError::WrongWallet);
        }
        if now_us > self.expires_at_us {
            return Err(WalletChallengeError::Expired);
        }
        // OAUTH2-C wires the real SQIsign verify here. v0 stub: any non-
        // empty signature passes, so the surrounding state machine can be
        // exercised in isolation.
        if signature.is_empty() {
            return Err(WalletChallengeError::BadSignature);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    fn rng() -> ChaCha8Rng {
        ChaCha8Rng::seed_from_u64(0xCAFE)
    }

    #[test]
    fn new_challenge_binds_wallet_and_expires_5m_out() {
        let mut r = rng();
        let wallet = [1u8; 32];
        let now = 1_780_000_000_000_000;
        let c = WalletChallenge::new(wallet, &mut r, now);
        assert_eq!(c.wallet, wallet);
        assert_eq!(c.expires_at_us, now + 5 * 60 * 1_000_000);
        assert_ne!(c.bytes, [0u8; 32]);
    }

    #[test]
    fn verify_stub_accepts_matching_wallet_with_signature() {
        let mut r = rng();
        let wallet = [1u8; 32];
        let now = 1_000;
        let c = WalletChallenge::new(wallet, &mut r, now);
        c.verify_stub(&wallet, b"any-non-empty-sig", now).unwrap();
    }

    #[test]
    fn verify_stub_rejects_wrong_wallet() {
        let mut r = rng();
        let wallet = [1u8; 32];
        let other = [2u8; 32];
        let now = 1_000;
        let c = WalletChallenge::new(wallet, &mut r, now);
        assert_eq!(
            c.verify_stub(&other, b"sig", now).unwrap_err(),
            WalletChallengeError::WrongWallet
        );
    }

    #[test]
    fn verify_stub_rejects_expired_challenge() {
        let mut r = rng();
        let wallet = [1u8; 32];
        let now = 1_000;
        let c = WalletChallenge::new(wallet, &mut r, now);
        let later = c.expires_at_us + 1;
        assert_eq!(
            c.verify_stub(&wallet, b"sig", later).unwrap_err(),
            WalletChallengeError::Expired
        );
    }

    #[test]
    fn verify_stub_rejects_empty_signature() {
        let mut r = rng();
        let wallet = [1u8; 32];
        let now = 1_000;
        let c = WalletChallenge::new(wallet, &mut r, now);
        assert_eq!(
            c.verify_stub(&wallet, &[], now).unwrap_err(),
            WalletChallengeError::BadSignature
        );
    }
}
