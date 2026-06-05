//! flux-email — substrate email subsystem (EMAIL-A scaffold).
//!
//! Outbound transactional path: chain layer fires an [`EmailEvent`] when
//! something interesting happens on-chain (incoming swap output, large
//! balance change, validator produced a block, web-UI OTP). Chain looks up
//! the wallet's email opt-in. If opted in, hands `(EmailAddress, EmailEvent)`
//! to an [`EmailSender`] impl which renders + delivers.
//!
//! Inbound MX path (EMAIL-F/G/H, not in this scaffold): operator runs a
//! Flux node with `flux-email::smtp::Server` started on :25 / :587. The
//! server accepts SMTP from peer MTAs (incoming) and authenticated SMTP
//! from chain users (submission). Wallet-keyed AUTH via SQIsign challenge.
//! Spec: `flux/docs/flux-email-spec.md`.
//!
//! Design lock from project_sigil_chain memory #22: substrate primitives
//! live in `flux/`, chain code lives in the chain workspace (e.g. `sigil/`).
//! flux-email is the substrate; `sigil-email` is the SIGIL consumer.

#![warn(missing_docs)]

mod event;
mod send;

pub use event::NotificationEvent;
pub use event::{
    AuthCodeEvent,
    BlockProducedEvent,
    EmailEvent,
    EnabledEventFlags,
    IncomingSwapOutputEvent,
    LargeBalanceChangeEvent,
    RenderedEmail,
};
pub use send::{
    EmailSender, InMemorySender, RelayConfig, RelaySender, SendError,
};

/// 32-byte wallet address — opaque to flux-email. Chain semantics belong to
/// the chain. We just need an ID to look up nicknames + thread events.
pub type WalletId = [u8; 32];

/// 32-byte token identifier.
pub type TokenId = [u8; 32];

/// 32-byte pool identifier.
pub type PoolId = [u8; 32];

/// 32-byte block hash.
pub type BlockHash = [u8; 32];

/// A validated email address. Construct via [`EmailAddress::parse`] to enforce
/// the basic RFC 5321 shape (`<local-part>@<domain>`, lengths under the spec
/// limits, no whitespace, exactly one `@`). Production wants the full
/// fastmail-grade validator; v0 is "good enough to reject the obvious junk."
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct EmailAddress(String);

impl EmailAddress {
    /// Parse a string into a validated address. Returns `Err` for anything
    /// that isn't shaped like an RFC 5321 address.
    pub fn parse(s: impl Into<String>) -> Result<Self, AddressError> {
        let s = s.into();
        if s.is_empty() {
            return Err(AddressError::Empty);
        }
        if s.len() > 254 {
            return Err(AddressError::TooLong);
        }
        let at_count = s.matches('@').count();
        if at_count != 1 {
            return Err(AddressError::WrongAtCount);
        }
        let (local, domain) = s.split_once('@').expect("at_count == 1");
        if local.is_empty() || domain.is_empty() {
            return Err(AddressError::EmptyPart);
        }
        if local.len() > 64 {
            return Err(AddressError::LocalPartTooLong);
        }
        if s.chars().any(|c| c.is_whitespace()) {
            return Err(AddressError::ContainsWhitespace);
        }
        // RFC 5321 allows lots of weird local-part chars; we accept anything
        // non-whitespace for v0 + leave strict pre-flight to the chain layer.
        Ok(Self(s))
    }

    /// Underlying string, e.g. for printing or constructing a lettre
    /// `Mailbox`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Address-parsing errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AddressError {
    /// Empty input.
    #[error("empty address")]
    Empty,
    /// > 254 chars (RFC 5321 §4.5.3.1.1).
    #[error("address exceeds 254-char RFC 5321 limit")]
    TooLong,
    /// Zero `@` or more than one.
    #[error("address must contain exactly one '@'")]
    WrongAtCount,
    /// Empty local or empty domain.
    #[error("address has an empty local-part or domain")]
    EmptyPart,
    /// Local-part > 64 chars (RFC 5321 §4.5.3.1.1).
    #[error("local-part exceeds 64-char RFC 5321 limit")]
    LocalPartTooLong,
    /// Contains whitespace.
    #[error("address must not contain whitespace")]
    ContainsWhitespace,
}

#[cfg(test)]
mod address_tests {
    use super::*;

    #[test]
    fn parses_a_clean_address() {
        assert_eq!(
            EmailAddress::parse("rocky@sigilgraph.com").unwrap().as_str(),
            "rocky@sigilgraph.com"
        );
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(EmailAddress::parse("").unwrap_err(), AddressError::Empty);
    }

    #[test]
    fn rejects_double_at() {
        assert_eq!(
            EmailAddress::parse("rocky@@sigilgraph.com").unwrap_err(),
            AddressError::WrongAtCount
        );
    }

    #[test]
    fn rejects_no_at() {
        assert_eq!(
            EmailAddress::parse("rocky_sigilgraph.com").unwrap_err(),
            AddressError::WrongAtCount
        );
    }

    #[test]
    fn rejects_empty_local() {
        assert_eq!(
            EmailAddress::parse("@sigilgraph.com").unwrap_err(),
            AddressError::EmptyPart
        );
    }

    #[test]
    fn rejects_empty_domain() {
        assert_eq!(
            EmailAddress::parse("rocky@").unwrap_err(),
            AddressError::EmptyPart
        );
    }

    #[test]
    fn rejects_whitespace() {
        assert_eq!(
            EmailAddress::parse("rocky @sigilgraph.com").unwrap_err(),
            AddressError::ContainsWhitespace
        );
    }

    #[test]
    fn rejects_too_long_local() {
        let local = "x".repeat(65);
        let addr = format!("{local}@sigilgraph.com");
        assert_eq!(
            EmailAddress::parse(addr).unwrap_err(),
            AddressError::LocalPartTooLong
        );
    }

    #[test]
    fn rejects_too_long_full() {
        let local = "x".repeat(100);
        let domain = "y".repeat(200);
        let addr = format!("{local}@{domain}.com");
        assert_eq!(
            EmailAddress::parse(addr).unwrap_err(),
            AddressError::TooLong
        );
    }

    #[test]
    fn nickname_aliased_addresses_pass() {
        // Per [[user-viktor-nicknames]] — short nicknames are the canonical
        // form. Verify the parser accepts them.
        for nick in ["rocky", "codex", "adrian", "viktor"] {
            let addr = format!("{nick}@sigilgraph.com");
            EmailAddress::parse(addr).expect("clean nickname address must parse");
        }
    }
}
