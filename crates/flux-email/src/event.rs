//! Typed email-event surface — the four notification triggers from
//! `flux/docs/flux-email-spec.md` plus the `RenderedEmail` payload that goes
//! on the wire.
//!
//! Templates are inline (rather than file-loaded) for v0 — small footprint
//! + no I/O at render time. A future patch can lift them into a template
//! engine if branding diverges per chain.

use serde::{Deserialize, Serialize};

use crate::{BlockHash, PoolId, TokenId, WalletId};

/// The four trigger flavours plus their per-event payload.
///
/// Each variant maps to a [`render`](EmailEvent::render) call producing a
/// [`RenderedEmail`] with `subject + body_text + body_html`. HTML is
/// deliberately minimal — Gmail / Outlook / Apple Mail all render plaintext
/// just fine and HTML mostly trips spam filters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum EmailEvent {
    /// Incoming swap output landed.
    IncomingSwapOutput(IncomingSwapOutputEvent),
    /// Send/receive crossed the wallet's threshold.
    LargeBalanceChange(LargeBalanceChangeEvent),
    /// Validator produced a block.
    BlockProduced(BlockProducedEvent),
    /// Web-UI sign-in code.
    AuthCode(AuthCodeEvent),
    /// A generic notification (subject + body) — any chain feature sends one
    /// (citizenship admission, alerts, …).
    Notification(NotificationEvent),
}

/// Payload for [`EmailEvent::IncomingSwapOutput`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncomingSwapOutputEvent {
    /// Wallet that received the output.
    pub wallet: WalletId,
    /// Token credited.
    pub out_token: TokenId,
    /// Amount (in token's smallest unit).
    pub amount: u128,
    /// Pool the swap routed through.
    pub pool: PoolId,
    /// Block height where the swap settled.
    pub block_height: u64,
    /// Block hash for tx-lookup links.
    pub block_hash: BlockHash,
}

/// Payload for [`EmailEvent::LargeBalanceChange`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LargeBalanceChangeEvent {
    /// Wallet whose balance changed.
    pub wallet: WalletId,
    /// Which token.
    pub token: TokenId,
    /// Signed delta: positive = received, negative = sent.
    pub delta: i128,
    /// Post-change balance.
    pub new_balance: u128,
    /// Block where the change committed.
    pub block_height: u64,
}

/// Payload for [`EmailEvent::BlockProduced`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockProducedEvent {
    /// Validator wallet (same 32 bytes as wallet id).
    pub validator: WalletId,
    /// Height shipped.
    pub block_height: u64,
    /// Block hash.
    pub block_hash: BlockHash,
    /// Validator's share of the block reward.
    pub validator_share: u128,
    /// Master-wallet share of the reward (per sigil-bank::MASTER_MINING_FEE_BPS).
    pub master_share: u128,
    /// Transactions in this block.
    pub tx_count: u32,
}

/// Payload for [`EmailEvent::AuthCode`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthCodeEvent {
    /// Wallet logging in.
    pub wallet: WalletId,
    /// Six-digit numeric code. String to preserve leading zeros.
    pub code: String,
    /// Expiry timestamp (microseconds since epoch).
    pub expires_at_us: u64,
    /// Optional hint for the "if this wasn't you..." line. None when the
    /// chain layer can't / won't disclose source IP (privacy).
    pub client_ip_hint: Option<String>,
}

/// What gets handed to the SMTP transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedEmail {
    /// Subject line.
    pub subject: String,
    /// Plaintext body — preferred by spam filters + always renders correctly.
    pub body_text: String,
    /// HTML body — minimal, no JS, no remote images.
    pub body_html: String,
}

impl EmailEvent {
    /// Render the event into a `(subject, body_text, body_html)` triple. The
    /// rendering is deterministic — same event always produces same output
    /// — so it's safe to use in test assertions.
    pub fn render(&self) -> RenderedEmail {
        match self {
            EmailEvent::IncomingSwapOutput(e) => render_incoming_swap(e),
            EmailEvent::LargeBalanceChange(e) => render_large_balance(e),
            EmailEvent::BlockProduced(e) => render_block_produced(e),
            EmailEvent::AuthCode(e) => render_auth_code(e),
            EmailEvent::Notification(e) => render_notification(e),
        }
    }
}

/// Payload for [`EmailEvent::Notification`] — a generic subject + body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationEvent {
    /// Subject line.
    pub subject: String,
    /// Plaintext body.
    pub body: String,
}

fn render_notification(e: &NotificationEvent) -> RenderedEmail {
    let html = format!(
        "<p>{}</p><p style=\"color:#888\">SIGIL automated notification</p>",
        e.body.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    );
    RenderedEmail {
        subject: e.subject.clone(),
        body_text: format!("{}\n\n— SIGIL automated notification\n", e.body),
        body_html: html,
    }
}

fn hex8(bytes: &[u8; 32]) -> String {
    // First 4 bytes as 8 hex chars — short tx/block ref for human-readable
    // bodies. Production wants full hash + a link to the explorer; v0 keeps
    // it tight so the body fits in a notification.
    format!(
        "{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3]
    )
}

fn render_incoming_swap(e: &IncomingSwapOutputEvent) -> RenderedEmail {
    let text = format!(
        "You received {} units of token {} on SIGIL.\n\
         \n\
         Pool: {}\n\
         Block #{} ({})\n\
         \n\
         — SIGIL automated notification\n",
        e.amount,
        hex8(&e.out_token),
        hex8(&e.pool),
        e.block_height,
        hex8(&e.block_hash),
    );
    let html = format!(
        "<p>You received <strong>{}</strong> units of token <code>{}</code> on SIGIL.</p>\
         <p>Pool: <code>{}</code><br>Block #{} (<code>{}</code>)</p>\
         <p style=\"color:#888\">SIGIL automated notification</p>",
        e.amount,
        hex8(&e.out_token),
        hex8(&e.pool),
        e.block_height,
        hex8(&e.block_hash),
    );
    RenderedEmail {
        subject: format!("Incoming on SIGIL: {} units", e.amount),
        body_text: text,
        body_html: html,
    }
}

fn render_large_balance(e: &LargeBalanceChangeEvent) -> RenderedEmail {
    let sign = if e.delta >= 0 { "+" } else { "" };
    let text = format!(
        "Your SIGIL balance changed by {}{} (token {}).\n\
         \n\
         New balance: {}\n\
         Block #{}\n\
         \n\
         — SIGIL automated notification\n",
        sign, e.delta, hex8(&e.token), e.new_balance, e.block_height,
    );
    let html = format!(
        "<p>Your SIGIL balance changed by <strong>{}{}</strong> (token <code>{}</code>).</p>\
         <p>New balance: <strong>{}</strong><br>Block #{}</p>\
         <p style=\"color:#888\">SIGIL automated notification</p>",
        sign, e.delta, hex8(&e.token), e.new_balance, e.block_height,
    );
    RenderedEmail {
        subject: format!("Balance change on SIGIL: {}{}", sign, e.delta),
        body_text: text,
        body_html: html,
    }
}

fn render_block_produced(e: &BlockProducedEvent) -> RenderedEmail {
    let text = format!(
        "You produced SIGIL block #{} ({}).\n\
         \n\
         Validator reward: {} SIGIL\n\
         Master-wallet share: {} SIGIL\n\
         Transactions: {}\n\
         \n\
         — SIGIL validator notification\n",
        e.block_height,
        hex8(&e.block_hash),
        e.validator_share,
        e.master_share,
        e.tx_count,
    );
    let html = format!(
        "<p>You produced SIGIL block #<strong>{}</strong> (<code>{}</code>).</p>\
         <p>Validator reward: <strong>{} SIGIL</strong><br>\
         Master-wallet share: <strong>{} SIGIL</strong><br>\
         Transactions: <strong>{}</strong></p>\
         <p style=\"color:#888\">SIGIL validator notification</p>",
        e.block_height,
        hex8(&e.block_hash),
        e.validator_share,
        e.master_share,
        e.tx_count,
    );
    RenderedEmail {
        subject: format!("SIGIL block #{} produced", e.block_height),
        body_text: text,
        body_html: html,
    }
}

fn render_auth_code(e: &AuthCodeEvent) -> RenderedEmail {
    let ip_hint = match &e.client_ip_hint {
        Some(ip) => format!("\nSign-in attempt from: {ip}\n"),
        None => String::new(),
    };
    let text = format!(
        "Your SIGIL sign-in code: {}\n\
         \n\
         Valid for 5 minutes (expires at {} µs since epoch).\n\
         {}\
         If this wasn't you, ignore this email and consider rotating your wallet.\n\
         \n\
         — SIGIL authentication\n",
        e.code, e.expires_at_us, ip_hint,
    );
    let html = format!(
        "<p>Your SIGIL sign-in code:</p>\
         <p style=\"font-size:2em;letter-spacing:0.3em;font-family:monospace;background:#eee;padding:0.5em;border-radius:0.3em\">\
         <strong>{}</strong></p>\
         <p>Valid for 5 minutes.</p>\
         <p style=\"color:#888\">If this wasn't you, ignore this email.</p>",
        e.code,
    );
    RenderedEmail {
        subject: "Your SIGIL sign-in code".into(),
        body_text: text,
        body_html: html,
    }
}

/// Per-wallet subscription flags. Bitfield so the wallet can opt into any
/// subset of the four triggers (or all, or none).
///
/// Wire format: bincode-friendly u8 bitfield. Validators default to none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EnabledEventFlags {
    /// IncomingSwapOutput trigger.
    pub incoming_swap_output: bool,
    /// LargeBalanceChange trigger.
    pub large_balance_change: bool,
    /// BlockProduced trigger (only meaningful for validator wallets).
    pub block_produced: bool,
    /// AuthCode trigger.
    pub auth_code: bool,
}

impl EnabledEventFlags {
    /// Does this event match the wallet's opt-in?
    pub fn matches(&self, evt: &EmailEvent) -> bool {
        match evt {
            EmailEvent::IncomingSwapOutput(_) => self.incoming_swap_output,
            EmailEvent::LargeBalanceChange(_) => self.large_balance_change,
            EmailEvent::BlockProduced(_) => self.block_produced,
            EmailEvent::AuthCode(_) => self.auth_code,
            // Notifications are explicit sends (not an opt-in subscription) → always allowed.
            EmailEvent::Notification(_) => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_wallet() -> WalletId { [1u8; 32] }

    #[test]
    fn render_incoming_swap_has_amount_and_pool() {
        let e = EmailEvent::IncomingSwapOutput(IncomingSwapOutputEvent {
            wallet: dummy_wallet(),
            out_token: [7u8; 32],
            amount: 9_867,
            pool: [9u8; 32],
            block_height: 42,
            block_hash: [0xAB; 32],
        });
        let r = e.render();
        assert!(r.subject.contains("9867"));
        assert!(r.body_text.contains("9867"));
        assert!(r.body_text.contains("Pool:"));
        assert!(r.body_text.contains("Block #42"));
        // HTML body also wired
        assert!(r.body_html.contains("9867"));
    }

    #[test]
    fn render_large_balance_shows_signed_delta() {
        let e = EmailEvent::LargeBalanceChange(LargeBalanceChangeEvent {
            wallet: dummy_wallet(),
            token: [0u8; 32],
            delta: -100_000,
            new_balance: 9_900_000,
            block_height: 100,
        });
        let r = e.render();
        // Negative delta renders without a `+` prefix.
        assert!(r.subject.contains("-100000"));
        assert!(r.body_text.contains("-100000"));
    }

    #[test]
    fn render_block_produced_includes_reward_split() {
        let e = EmailEvent::BlockProduced(BlockProducedEvent {
            validator: dummy_wallet(),
            block_height: 7,
            block_hash: [0xCD; 32],
            validator_share: 95,
            master_share: 5,
            tx_count: 12,
        });
        let r = e.render();
        assert!(r.subject.contains("#7"));
        assert!(r.body_text.contains("95 SIGIL"));
        assert!(r.body_text.contains("5 SIGIL"));
        assert!(r.body_text.contains("Transactions: 12"));
    }

    #[test]
    fn render_auth_code_pins_the_code_visually() {
        let e = EmailEvent::AuthCode(AuthCodeEvent {
            wallet: dummy_wallet(),
            code: "047125".into(), // leading zero preserved
            expires_at_us: 1_780_000_000_000_000,
            client_ip_hint: Some("203.0.113.42".into()),
        });
        let r = e.render();
        assert!(r.body_text.contains("047125"));
        assert!(r.body_text.contains("203.0.113.42"));
        // HTML uses a monospace big-font block.
        assert!(r.body_html.contains("047125"));
        assert!(r.body_html.contains("monospace"));
    }

    #[test]
    fn enabled_flags_match_each_event() {
        let f = EnabledEventFlags { incoming_swap_output: true, ..Default::default() };
        let yes = EmailEvent::IncomingSwapOutput(IncomingSwapOutputEvent {
            wallet: dummy_wallet(), out_token: [0u8; 32], amount: 1,
            pool: [0u8; 32], block_height: 1, block_hash: [0u8; 32],
        });
        let no = EmailEvent::AuthCode(AuthCodeEvent {
            wallet: dummy_wallet(), code: "111111".into(),
            expires_at_us: 0, client_ip_hint: None,
        });
        assert!(f.matches(&yes));
        assert!(!f.matches(&no));
    }

    // Note: EmailEvent is NOT serialized over the wire from flux-email — it
    // gets rendered to text/HTML in [`EmailEvent::render`]. If a future chain
    // wants to persist events for replay (e.g. an `email_outbox` SMT), it
    // should use bincode (which handles u128 natively) or its own u128_str
    // shim like sigil-state does. serde_json roundtrip is deliberately not
    // a property of this crate.
}
