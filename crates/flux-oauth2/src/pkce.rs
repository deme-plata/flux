//! PKCE — Proof Key for Code Exchange (RFC 7636).
//!
//! Why PKCE matters here: SIGIL wallet sign-in flows happen on a public
//! browser (no client secret). Authorization Code with a plain redirect can
//! be intercepted; PKCE binds the code to a one-time secret only the
//! initiating client knows. The client generates a random `code_verifier`,
//! hashes it (SHA-256 → S256) to produce a `code_challenge`, sends the
//! challenge with the authorize request. When exchanging the code for a
//! token, the client presents the original verifier. The server SHA-256's
//! it and compares to the stored challenge. Match → exchange. Mismatch →
//! reject.
//!
//! This module implements:
//!
//! - **Generation** of `code_verifier` (43-128 chars, base64url-safe).
//! - **Transform** `verifier → challenge` (S256 only; plain is rejected as
//!   non-compliant for v0).
//! - **Verification** of a verifier against a stored challenge.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use sha2::{Digest, Sha256};

use serde::{Deserialize, Serialize};

/// RFC 7636 §4.2 transform methods. `S256` is mandatory for v0; `Plain` is
/// kept in the enum for completeness but rejected by [`PkceChallenge::verify`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum PkceMethod {
    /// SHA-256 of the verifier, base64url-no-pad. Mandatory in v0.
    S256,
    /// Verifier sent as-is — disabled in v0.
    Plain,
}

/// PKCE errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PkceError {
    /// Verifier doesn't match the stored challenge.
    #[error("PKCE verifier doesn't match challenge")]
    Mismatch,
    /// Verifier or challenge length outside RFC 7636 §4.1 bounds (43-128).
    #[error("PKCE string length must be 43-128 chars")]
    BadLength,
    /// Caller used `Plain` method which is disabled in v0.
    #[error("PKCE plain method is not supported; use S256")]
    PlainUnsupported,
}

/// A code_verifier — the client's secret. Generate via
/// [`PkceVerifier::random`] and never log it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PkceVerifier(pub String);

impl PkceVerifier {
    /// Generate a random verifier: 64 bytes of CSPRNG, base64url encoded
    /// (produces 86 chars, well within the 43-128 RFC range).
    pub fn random<R: rand::Rng>(rng: &mut R) -> Self {
        let mut bytes = [0u8; 64];
        rng.fill_bytes(&mut bytes);
        Self(URL_SAFE_NO_PAD.encode(bytes))
    }

    /// Length-check the verifier (call this when parsing from a token-
    /// exchange request).
    pub fn validate(&self) -> Result<(), PkceError> {
        let len = self.0.len();
        if !(43..=128).contains(&len) {
            return Err(PkceError::BadLength);
        }
        Ok(())
    }
}

/// A code_challenge — derived from a verifier. The server stores this in the
/// authorize-code record + checks the verifier against it at exchange time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PkceChallenge {
    /// The challenge string, base64url-no-pad.
    pub value: String,
    /// Transform method used.
    pub method: PkceMethod,
}

impl PkceChallenge {
    /// Compute the S256 challenge from a verifier.
    pub fn from_verifier_s256(verifier: &PkceVerifier) -> Result<Self, PkceError> {
        verifier.validate()?;
        let digest = Sha256::digest(verifier.0.as_bytes());
        Ok(Self {
            value: URL_SAFE_NO_PAD.encode(digest),
            method: PkceMethod::S256,
        })
    }

    /// Verify a verifier against this challenge. Rejects plain method.
    pub fn verify(&self, verifier: &PkceVerifier) -> Result<(), PkceError> {
        match self.method {
            PkceMethod::Plain => Err(PkceError::PlainUnsupported),
            PkceMethod::S256 => {
                let recomputed = Self::from_verifier_s256(verifier)?;
                if recomputed.value == self.value {
                    Ok(())
                } else {
                    Err(PkceError::Mismatch)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    fn det_rng(seed: u64) -> ChaCha8Rng {
        ChaCha8Rng::seed_from_u64(seed)
    }

    #[test]
    fn random_verifier_is_in_range() {
        let mut rng = det_rng(1);
        let v = PkceVerifier::random(&mut rng);
        assert!((43..=128).contains(&v.0.len()));
        v.validate().unwrap();
    }

    #[test]
    fn s256_roundtrip() {
        let mut rng = det_rng(7);
        let v = PkceVerifier::random(&mut rng);
        let c = PkceChallenge::from_verifier_s256(&v).unwrap();
        c.verify(&v).unwrap();
    }

    #[test]
    fn wrong_verifier_rejected() {
        let mut rng = det_rng(2);
        let v1 = PkceVerifier::random(&mut rng);
        let v2 = PkceVerifier::random(&mut rng);
        let c = PkceChallenge::from_verifier_s256(&v1).unwrap();
        assert_eq!(c.verify(&v2).unwrap_err(), PkceError::Mismatch);
    }

    #[test]
    fn plain_method_disabled() {
        let mut rng = det_rng(3);
        let v = PkceVerifier::random(&mut rng);
        let c = PkceChallenge {
            value: v.0.clone(),
            method: PkceMethod::Plain,
        };
        assert_eq!(c.verify(&v).unwrap_err(), PkceError::PlainUnsupported);
    }

    #[test]
    fn out_of_range_verifier_rejected() {
        let v = PkceVerifier("short".into());
        assert_eq!(v.validate().unwrap_err(), PkceError::BadLength);
        let v = PkceVerifier("x".repeat(129));
        assert_eq!(v.validate().unwrap_err(), PkceError::BadLength);
    }

    #[test]
    fn rfc7636_test_vector() {
        // RFC 7636 Appendix B: verifier "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
        // → S256 challenge "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        let v = PkceVerifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into());
        let c = PkceChallenge::from_verifier_s256(&v).unwrap();
        assert_eq!(c.value, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        assert_eq!(c.method, PkceMethod::S256);
        c.verify(&v).unwrap();
    }
}
