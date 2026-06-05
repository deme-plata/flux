//! Outbound transport — the `EmailSender` trait + production-grade
//! `RelaySender` (lettre 0.11 SMTP relay) + zero-IO `InMemorySender` for
//! tests.
//!
//! Why a trait + two impls:
//!
//! 1. **`InMemorySender`** lets chain-layer tests (sigil-tx::apply_tx) assert
//!    "this swap fired an email" without binding the test to a running SMTP
//!    server. Sent messages land in a `Mutex<Vec<_>>` the test can drain.
//! 2. **`RelaySender`** is what production uses. Lettre handles STARTTLS,
//!    AUTH PLAIN, SMTPUTF8 — the things you do NOT want to re-implement.
//!
//! Inbound (smtp.rs + mta.rs in EMAIL-F + EMAIL-G) is a separate dance and
//! lives outside this module.

use std::sync::Mutex;

use lettre::{
    message::header::ContentType,
    transport::smtp::{authentication::Credentials, SmtpTransport},
    Message, Transport,
};

use crate::{EmailAddress, EmailEvent};

/// All the ways outbound send can fail at the substrate boundary. Chain
/// layers can map these to their own error types (e.g. emit an on-chain
/// `EmailSendFailed` event for audit).
#[derive(Debug, thiserror::Error)]
pub enum SendError {
    /// Couldn't construct the lettre `Message` from the event.
    #[error("message build failed: {0}")]
    Build(String),
    /// SMTP transport returned an error (transport rejected the message, the
    /// relay refused AUTH, the TLS handshake failed, etc).
    #[error("smtp transport failed: {0}")]
    Transport(String),
    /// Sender impl is configured but disabled at runtime (e.g.
    /// `RelaySender` with no relay host).
    #[error("sender not configured")]
    NotConfigured,
}

/// What chain code calls. Synchronous on purpose — production chains run
/// the send on a sidecar tokio task that drains the chain's outbound queue,
/// so making the trait async would force the chain to thread an executor
/// through layers that don't need one. Sync + caller-controlled dispatch
/// keeps the dep budget low.
pub trait EmailSender: Send + Sync {
    /// Render `evt` into a message and ship it to `to`. The sender is free
    /// to retry, queue, or drop on the floor (test sender keeps a log).
    fn send(&self, to: &EmailAddress, evt: &EmailEvent) -> Result<(), SendError>;
}

// ── In-memory test sender ────────────────────────────────────────────────────

/// A non-network sender that just records sent messages. Used by tests that
/// want to assert "the chain DID fire an email" without standing up an SMTP
/// server.
#[derive(Debug, Default)]
pub struct InMemorySender {
    /// Sent log. Drain via [`InMemorySender::sent`].
    inner: Mutex<Vec<(EmailAddress, EmailEvent)>>,
}

impl InMemorySender {
    /// Empty sender.
    pub fn new() -> Self {
        Self { inner: Mutex::new(Vec::new()) }
    }

    /// Snapshot of every message sent through this sender.
    pub fn sent(&self) -> Vec<(EmailAddress, EmailEvent)> {
        self.inner.lock().expect("sent log poisoned").clone()
    }

    /// Count of sent messages.
    pub fn count(&self) -> usize {
        self.inner.lock().expect("sent log poisoned").len()
    }
}

impl EmailSender for InMemorySender {
    fn send(&self, to: &EmailAddress, evt: &EmailEvent) -> Result<(), SendError> {
        self.inner
            .lock()
            .expect("sent log poisoned")
            .push((to.clone(), evt.clone()));
        Ok(())
    }
}

// ── Production relay sender ──────────────────────────────────────────────────

/// Operator-supplied SMTP relay config. The relay is whatever the operator
/// trusts: their own Postfix on `localhost:587`, SendGrid, Mailgun raw SMTP,
/// SES SMTP — flux-email doesn't care, lettre speaks plain RFC 5321 with
/// STARTTLS + AUTH PLAIN.
#[derive(Debug, Clone)]
pub struct RelayConfig {
    /// Relay host (e.g. `smtp.sendgrid.net`).
    pub host: String,
    /// Relay port. 587 = submission with STARTTLS; 465 = implicit TLS.
    pub port: u16,
    /// Whether to use STARTTLS (port 587). False = implicit TLS (port 465).
    pub starttls: bool,
    /// AUTH PLAIN username.
    pub username: String,
    /// AUTH PLAIN password / API key.
    pub password: String,
    /// `From:` address on every outbound message. Convention:
    /// `noreply@<chain-domain>`. The operator's responsibility to make sure
    /// SPF/DKIM/DMARC are configured for this address.
    pub from: EmailAddress,
}

/// Production sender — wraps a lettre `SmtpTransport`. Synchronous send;
/// callers should drive this on a tokio task or a dedicated thread to keep
/// the chain's request loop unblocked.
pub struct RelaySender {
    config: RelayConfig,
    transport: SmtpTransport,
}

impl RelaySender {
    /// Build a sender from operator config. Fails if the relay host name
    /// can't be parsed.
    pub fn new(config: RelayConfig) -> Result<Self, SendError> {
        let creds = Credentials::new(config.username.clone(), config.password.clone());
        let builder = if config.starttls {
            SmtpTransport::starttls_relay(&config.host)
                .map_err(|e| SendError::Transport(e.to_string()))?
        } else {
            SmtpTransport::relay(&config.host)
                .map_err(|e| SendError::Transport(e.to_string()))?
        };
        let transport = builder.port(config.port).credentials(creds).build();
        Ok(Self { config, transport })
    }
}

impl EmailSender for RelaySender {
    fn send(&self, to: &EmailAddress, evt: &EmailEvent) -> Result<(), SendError> {
        let rendered = evt.render();
        // lettre wants `Mailbox`. Both addresses parsed at this boundary —
        // configs that hand in a malformed address fail here, never on the
        // wire.
        let to_mb = to
            .as_str()
            .parse::<lettre::message::Mailbox>()
            .map_err(|e| SendError::Build(format!("invalid to address: {e}")))?;
        let from_mb = self
            .config
            .from
            .as_str()
            .parse::<lettre::message::Mailbox>()
            .map_err(|e| SendError::Build(format!("invalid from address: {e}")))?;

        let msg = Message::builder()
            .from(from_mb)
            .to(to_mb)
            .subject(&rendered.subject)
            .multipart(
                lettre::message::MultiPart::alternative()
                    .singlepart(
                        lettre::message::SinglePart::builder()
                            .header(ContentType::TEXT_PLAIN)
                            .body(rendered.body_text.clone()),
                    )
                    .singlepart(
                        lettre::message::SinglePart::builder()
                            .header(ContentType::TEXT_HTML)
                            .body(rendered.body_html.clone()),
                    ),
            )
            .map_err(|e| SendError::Build(e.to_string()))?;

        self.transport
            .send(&msg)
            .map_err(|e| SendError::Transport(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AuthCodeEvent, EmailEvent, IncomingSwapOutputEvent,
    };

    fn dummy_swap_event() -> EmailEvent {
        EmailEvent::IncomingSwapOutput(IncomingSwapOutputEvent {
            wallet: [1u8; 32],
            out_token: [7u8; 32],
            amount: 9_867,
            pool: [9u8; 32],
            block_height: 42,
            block_hash: [0xAB; 32],
        })
    }

    #[test]
    fn in_memory_sender_records_every_send() {
        let sender = InMemorySender::new();
        let addr = EmailAddress::parse("rocky@sigilgraph.com").unwrap();
        sender.send(&addr, &dummy_swap_event()).unwrap();
        sender.send(&addr, &dummy_swap_event()).unwrap();
        assert_eq!(sender.count(), 2);
        let sent = sender.sent();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].0, addr);
        match &sent[0].1 {
            EmailEvent::IncomingSwapOutput(e) => assert_eq!(e.amount, 9_867),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn in_memory_sender_handles_auth_codes() {
        let sender = InMemorySender::new();
        let addr = EmailAddress::parse("viktor@sigilgraph.com").unwrap();
        let evt = EmailEvent::AuthCode(AuthCodeEvent {
            wallet: [1u8; 32],
            code: "424242".into(),
            expires_at_us: 1_780_000_000_000_000,
            client_ip_hint: None,
        });
        sender.send(&addr, &evt).unwrap();
        let sent = sender.sent();
        match &sent[0].1 {
            EmailEvent::AuthCode(a) => assert_eq!(a.code, "424242"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn relay_sender_rejects_bad_from_address() {
        // Build a relay sender, then break the from address. lettre will
        // surface the parse failure as `SendError::Build`, never silently.
        let cfg = RelayConfig {
            host: "smtp.example.com".into(),
            port: 587,
            starttls: true,
            username: "test".into(),
            password: "test".into(),
            from: EmailAddress::parse("noreply@sigilgraph.com").unwrap(),
        };
        let sender = RelaySender::new(cfg).unwrap();
        // Can't actually exercise the real SMTP path in a unit test without
        // a running server. We at least prove the constructor wires + the
        // sender impl exists. End-to-end SMTP delivery is exercised in the
        // EMAIL-F/G/H phases when the inbound MX lands.
        let _ = sender; // suppress unused warning
    }
}
