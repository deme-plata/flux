//! flux-oauth2 — substrate OAuth2 provider for Flux chains.
//!
//! v0 (OAUTH2-A): opaque token mint, PKCE (S256) verifier, wallet-challenge
//! state machine. Higher phases (OAUTH2-B/C/D/E/F per
//! `flux/docs/flux-vcs-spec.md`) wire the full Authorization Code flow,
//! HTTP handlers, refresh tokens, OpenAPI spec, and the SQIsign challenge
//! against the chain's wallet registry.
//!
//! Design notes:
//!
//! 1. **Transport-agnostic.** This crate produces state transitions, not HTTP
//!    responses. A host application (flux-vcs's HTTP layer, the SIGIL wallet
//!    web app) threads the state machine into its preferred framework.
//! 2. **Wallet IS the account.** No passwords, no email/password fallback.
//!    Sign-in proves wallet ownership via SQIsign signature over a server-
//!    minted challenge. If you lose the wallet you lose the account; v1
//!    might wire a "secondary wallet" recovery path.
//! 3. **Opaque tokens.** Random 32-byte values, base64url-encoded in
//!    `Authorization: Bearer ...`, hashed-at-rest in flux-db. JWT was
//!    considered + rejected for v0 (revocation matters more than DB
//!    roundtrip cost at invite-scale).

#![warn(missing_docs)]

mod pkce;
mod token;
mod wallet_auth;

pub use pkce::{PkceChallenge, PkceError, PkceMethod, PkceVerifier};
pub use token::{
    AccessToken, RefreshToken, TokenError, TokenHash, TokenMint, TokenScope,
};
pub use wallet_auth::{WalletChallenge, WalletChallengeError};

/// 32-byte wallet identifier — mirrors `sigil_state::WalletId` without
/// importing chain crates. flux-oauth2 stays substrate-only; chains adapt
/// this type at their boundary.
pub type WalletId = [u8; 32];
