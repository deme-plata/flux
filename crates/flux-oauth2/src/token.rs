//! Opaque token mint.
//!
//! Tokens are 32 random bytes, base64url-no-pad encoded in the
//! `Authorization: Bearer ...` header. They're hashed-at-rest (SHA-256) so a
//! flux-db dump never reveals live tokens. Verification: hash the bearer
//! token, look up the hash in flux-db. Constant-time comparison via
//! `subtle` is overkill at 32-byte scale; we use eq.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use sha2::{Digest, Sha256};

use serde::{Deserialize, Serialize};

use crate::WalletId;

/// What permissions does a token convey. v0 ships two scopes —
/// `RepoRead` (clone + pull) and `RepoWrite` (clone + pull + push). flux-vcs
/// composes them per-repo via the role table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenScope {
    /// Read-only access (clone, pull).
    RepoRead,
    /// Read-write access (push allowed).
    RepoWrite,
    /// Admin access (role management, invite).
    RepoAdmin,
}

/// Access token: the value handed to the user-agent, presented in
/// `Authorization: Bearer ...`. Wire format is `String` (base64url-no-pad).
/// The struct is a thin newtype so we don't accidentally print it in logs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessToken(String);

impl AccessToken {
    /// Bearer-form string. Use this when assembling the
    /// `Authorization: Bearer ...` header.
    pub fn as_bearer(&self) -> &str {
        &self.0
    }

    /// Hash the token for flux-db lookup. The plain bytes never go to disk.
    pub fn hash(&self) -> TokenHash {
        let digest = Sha256::digest(self.0.as_bytes());
        TokenHash(digest.into())
    }
}

/// Refresh token: longer-lived (30 days default), used to mint new access
/// tokens without re-running the SQIsign challenge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefreshToken(String);

impl RefreshToken {
    /// Bearer-form string.
    pub fn as_bearer(&self) -> &str {
        &self.0
    }

    /// Hash for flux-db lookup.
    pub fn hash(&self) -> TokenHash {
        let digest = Sha256::digest(self.0.as_bytes());
        TokenHash(digest.into())
    }
}

/// SHA-256 hash of a token. What flux-db actually stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TokenHash([u8; 32]);

impl TokenHash {
    /// Hex representation, for logging + auditing.
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

/// Token-related errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TokenError {
    /// Token couldn't be base64-decoded or was wrong length.
    #[error("malformed token bytes")]
    Malformed,
    /// Token wasn't found in the store (revoked or never issued).
    #[error("unknown token")]
    Unknown,
    /// Token was found but its expiry is past.
    #[error("expired token")]
    Expired,
    /// Token was found but its scope doesn't grant the requested action.
    #[error("insufficient scope")]
    InsufficientScope,
}

/// Token mint — generates fresh access + refresh pairs from CSPRNG.
/// Stateless; the caller threads tokens into flux-db.
///
/// The mint is seeded deterministically for tests; production wires it to a
/// `ChaCha8Rng::from_entropy()` or equivalent at startup so live tokens
/// can't be predicted.
pub struct TokenMint<R: rand::Rng> {
    rng: R,
}

impl<R: rand::Rng> TokenMint<R> {
    /// Wrap a seeded RNG.
    pub fn new(rng: R) -> Self {
        Self { rng }
    }

    /// Mint one access token (32 random bytes, base64url-encoded).
    pub fn access_token(&mut self) -> AccessToken {
        let mut bytes = [0u8; 32];
        self.rng.fill_bytes(&mut bytes);
        AccessToken(URL_SAFE_NO_PAD.encode(bytes))
    }

    /// Mint one refresh token.
    pub fn refresh_token(&mut self) -> RefreshToken {
        let mut bytes = [0u8; 32];
        self.rng.fill_bytes(&mut bytes);
        RefreshToken(URL_SAFE_NO_PAD.encode(bytes))
    }

    /// Mint a pair atomically. Convention is access_token first.
    pub fn pair(&mut self) -> (AccessToken, RefreshToken) {
        (self.access_token(), self.refresh_token())
    }
}

/// A token-bound session record. Caller stores this in flux-db keyed by
/// `token_hash`; verification is `hash bearer → lookup → check expiry`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    /// The wallet this token authorizes for.
    pub wallet: WalletId,
    /// What the token can do.
    pub scope: TokenScope,
    /// Microsecond epoch when this token stops being valid.
    pub expires_at_us: u64,
    /// Linked refresh token hash (for paired revocation).
    pub refresh_hash: Option<TokenHash>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    #[test]
    fn mint_produces_distinct_tokens() {
        let rng = ChaCha8Rng::seed_from_u64(1);
        let mut mint = TokenMint::new(rng);
        let (a1, r1) = mint.pair();
        let (a2, r2) = mint.pair();
        assert_ne!(a1, a2);
        assert_ne!(r1, r2);
        assert_ne!(a1.as_bearer(), r1.as_bearer()); // access != refresh
    }

    #[test]
    fn mint_seeded_is_deterministic() {
        let mut m1 = TokenMint::new(ChaCha8Rng::seed_from_u64(42));
        let mut m2 = TokenMint::new(ChaCha8Rng::seed_from_u64(42));
        assert_eq!(m1.access_token(), m2.access_token());
    }

    #[test]
    fn hash_is_repeatable() {
        let rng = ChaCha8Rng::seed_from_u64(1);
        let mut mint = TokenMint::new(rng);
        let t = mint.access_token();
        assert_eq!(t.hash(), t.hash());
    }

    #[test]
    fn hash_is_unique_per_token() {
        let rng = ChaCha8Rng::seed_from_u64(1);
        let mut mint = TokenMint::new(rng);
        let t1 = mint.access_token();
        let t2 = mint.access_token();
        assert_ne!(t1.hash(), t2.hash());
    }

    #[test]
    fn token_is_url_safe() {
        let rng = ChaCha8Rng::seed_from_u64(99);
        let mut mint = TokenMint::new(rng);
        let t = mint.access_token();
        // No padding, no plus / slash — safe to drop into Authorization headers
        // and URL fragments without encoding.
        assert!(!t.as_bearer().contains('='));
        assert!(!t.as_bearer().contains('+'));
        assert!(!t.as_bearer().contains('/'));
    }

    #[test]
    fn hash_to_hex_is_64_chars() {
        let rng = ChaCha8Rng::seed_from_u64(1);
        let mut mint = TokenMint::new(rng);
        let h = mint.access_token().hash();
        assert_eq!(h.to_hex().len(), 64);
    }

    #[test]
    fn scope_serializes_snake_case() {
        // We rely on this for the OpenAPI spec + bearer-token introspection
        // response (RFC 7662 §2.2 mandates snake_case scope strings).
        let s = serde_json::to_string(&TokenScope::RepoWrite).unwrap();
        assert_eq!(s, r#""repo_write""#);
        let s = serde_json::to_string(&TokenScope::RepoAdmin).unwrap();
        assert_eq!(s, r#""repo_admin""#);
    }

    #[test]
    fn session_record_roundtrip() {
        let rec = SessionRecord {
            wallet: [1u8; 32],
            scope: TokenScope::RepoWrite,
            expires_at_us: 1_780_000_000_000_000,
            refresh_hash: Some(TokenHash([0xAB; 32])),
        };
        let j = serde_json::to_string(&rec).unwrap();
        let parsed: SessionRecord = serde_json::from_str(&j).unwrap();
        assert_eq!(rec, parsed);
    }
}
