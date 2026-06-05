# flux-email — substrate email subsystem (spec v0)

> Substrate-level email crate, lives at `flux/crates/flux-email/`. ANY chain on Flux (SIGIL, future siblings) consumes this. Both **outbound transactional** and **inbound MX** scoped — full port of Quillon's `email_smtp.rs` + `email_mta.rs` + `email_api.rs` + `email_auth_verify.rs` (2586 LOC across q-api-server).
>
> **Authored:** rocky, 2026-05-29. **Status:** draft scope — to lock with swarm before first claim.

---

## Why substrate, not chain-level

Per [[project-sigil-chain]] lock #22: `flux/` owns chain-agnostic primitives, `sigil/` owns SIGIL-specific application code. Email is general — operators of any Flux chain (SIGIL today, future siblings) want to notify users + run an MX. The crate sits in `flux/`; chain layers (`sigil-email`) thread it into their tx-apply paths.

## North star

> *"Really good at sending users emails if turned on."*

Two halves:
1. **Outbound transactional** — agent / RPC fires a templated email when something on-chain happens. Opt-in per wallet.
2. **Inbound MX** — accept incoming mail at `<nickname>@<chain-domain>` (per [[user-viktor-nicknames]]: nickname-aliased, not hash-keyed). Land it in the wallet's inbox + emit an on-chain event.

The crate exposes both. Operators can configure: outbound only, inbound only, both. Default is outbound off / inbound off (privacy-safe default).

---

## Module structure

```
flux/crates/flux-email/
├── Cargo.toml
└── src/
    ├── lib.rs            — exports trait + event types + entry points
    ├── send.rs           — outbound: trait EmailSender + lettre-based SMTP-relay impl
    ├── event.rs          — typed EmailEvent enum + templates (the 4 triggers)
    ├── address.rs        — nickname → wallet binding resolver
    ├── smtp.rs           — inbound SMTP server (port :25 MX + :587 submission)
    ├── mta.rs            — outbound MTA delivery loop (MX resolution + queueing)
    ├── auth.rs           — wallet-keyed SMTP AUTH verification
    └── tls.rs            — STARTTLS for Gmail/Outlook/Yahoo deliverability
```

Each module's first commit lands a stub + tests; subsequent commits flesh them out. The skeleton must compile in isolation so chains can path-dep `flux-email` while the inbound MX work is ongoing.

---

## Outbound: trait + lettre impl

```rust
/// What the chain hands to flux-email when it wants a notification fired.
pub trait EmailSender: Send + Sync {
    fn send(&self, to: EmailAddress, evt: &EmailEvent) -> Result<(), SendError>;
}

/// Default impl: connects to an upstream SMTP relay (operator's choice —
/// SES, Mailgun raw SMTP, Postfix-on-localhost). Production wants this.
pub struct RelaySender { /* lettre transport */ }

/// Test impl: stores sent emails in a Vec, no network. Used by sigil-tx
/// tests so the chain layer can assert "this swap fired an email" without
/// real SMTP.
pub struct InMemorySender { pub sent: Mutex<Vec<(EmailAddress, EmailEvent)>> }
```

`lettre 0.11` is the natural dep. Pure Rust, no native SMTP lib needed.

---

## EmailEvent — the four trigger types

```rust
pub enum EmailEvent {
    /// Trigger 1: incoming swap output landed in this wallet.
    /// Body: "You received {amount} {out_token_symbol} from pool {pool_id} on block {height}."
    IncomingSwapOutput {
        wallet: WalletId,
        out_token: TokenId,
        amount: u128,
        pool: PoolId,
        block_height: u64,
        block_hash: [u8; 32],
    },

    /// Trigger 2: a Send/Receive crossed the wallet's configured threshold.
    /// Default threshold: 100 SIGIL, user-tunable via opt-in config.
    /// Body: "Large balance change: {delta:+} {token}. New balance: {balance}."
    LargeBalanceChange {
        wallet: WalletId,
        token: TokenId,
        delta: i128, // signed; +ve = received, -ve = sent
        new_balance: u128,
        block_height: u64,
    },

    /// Trigger 3: this validator won VDF leader election + produced a block.
    /// Validators-only, opted in via a separate field from regular wallets.
    /// Body: "You produced block {height} ({hash}). Reward: {validator_share} SIGIL.
    ///        Master share: {master_share} SIGIL. Total TX in block: {n}."
    BlockProduced {
        validator: WalletId,
        block_height: u64,
        block_hash: [u8; 32],
        validator_share: u128,
        master_share: u128,
        tx_count: u32,
    },

    /// Trigger 4: OTP for web-UI auth. Wallet logs into wallet.sigilgraph.com,
    /// gets a 6-digit code valid for 5 min. Replaces SMS-2FA on the web flow.
    /// Body: "Your SIGIL sign-in code: {code}. Valid for 5 minutes."
    AuthCode {
        wallet: WalletId,
        code: String,           // 6-digit numeric
        expires_at_us: u64,
        client_ip_hint: Option<String>, // for "if this wasn't you..." line
    },
}
```

Each event has a `render(self) -> RenderedEmail { subject, body_text, body_html }` method. Templates live in `event.rs`, deliberately not in external files — a future patch can lift them into a template engine if branding diverges per chain.

---

## Nickname → wallet binding

Per [[user-viktor-nicknames]]: addresses should be `rocky@sigilgraph.com`, not `qnk7154…1ccb@sigilgraph.com`. The binding lives on-chain (chain owns the nickname registry) but flux-email needs to RESOLVE addresses at SMTP time.

`address.rs` exposes:

```rust
/// Chain implements this to answer "what wallet owns nickname X?"
pub trait NicknameResolver: Send + Sync {
    fn resolve(&self, nickname: &str) -> Option<WalletId>;
}

/// Reverse lookup: wallet → preferred nickname for outbound mail's From field.
pub trait NicknameLookup: Send + Sync {
    fn nickname_of(&self, wallet: &WalletId) -> Option<String>;
}
```

SIGIL ships its registry tx (`SigilTx::ClaimNickname { nickname: String, wallet: WalletId }`) and the chain's RPC handler implements `NicknameResolver` against `state.nickname_owners`. First-claim-wins, no transfers in v0 (post-MVP: SQIsign-signed transfer tx).

Reserved nicknames (rejected at registry time): `postmaster`, `admin`, `abuse`, `noreply`, `support` — the standard email role-account list. Per [[user-viktor-nicknames]] Viktor will likely want to reserve agent-protected names too (`rocky`, `codex`, `adrian`, `viktor`) at genesis.

---

## Inbound MX (port of Quillon `email_smtp.rs` + `email_mta.rs`)

**Direct port**: 811 LOC SMTP server + 335 LOC MTA. Lift modules verbatim, rename `q_types` → `flux_types` (or strip outright — flux-email shouldn't import chain types directly; chain hands it a `NicknameResolver`).

Wire-level requirements:
- **Port 25** for MX (incoming from other servers' MTAs)
- **Port 587** for submission (authenticated users sending out)
- **STARTTLS** mandatory for Gmail/Outlook/Yahoo deliverability (v8.7.5 Quillon line)
- **AUTH PLAIN/LOGIN** keyed against wallet ownership — user supplies `nickname` + a signed challenge, server verifies signature against the wallet's pubkey (per `email_auth_verify.rs`)
- **MX delivery loop** runs every 30s, drains outbound queue, resolves recipient MX records, attempts direct connect (falls back to a configured relay via `FLUX_SMTP_RELAY` env when egress port 25 is blocked — Contabo + most cloud hosts block :25 outbound by default)

**Deliverability discipline** (required to NOT land in Gmail spam):
- **DKIM** signing — operator generates RSA keypair, publishes pubkey at `default._domainkey.<chain-domain>` TXT record, flux-email signs every outbound mail
- **SPF** — operator publishes `v=spf1 ip4:<server-ip> -all` TXT record on `<chain-domain>`
- **DMARC** — operator publishes `v=DMARC1; p=quarantine; rua=mailto:postmaster@<chain-domain>` TXT record
- **Reverse DNS** — server's public IP must reverse to `<chain-domain>` for STARTTLS-issuing CAs to validate

These are operator config concerns; flux-email exposes the keys + signing hooks but doesn't manage DNS. Document the operator setup in `flux/docs/flux-email-operator-runbook.md` once the inbound module lands.

---

## Opt-in subscription model

Privacy-safe default: **all emails off until the wallet explicitly opts in**.

Chain (SIGIL) extends `SigilState`:

```rust
pub struct EmailOptIn {
    pub address: EmailAddress,
    pub enabled_events: EnabledEventFlags,  // bitfield
    pub large_balance_threshold: u128,      // for LargeBalanceChange trigger
}

pub struct SigilState {
    ...
    pub(crate) email_opt_ins: BTreeMap<WalletId, EmailOptIn>,
}
```

New tx variants:
- `SigilTx::SubscribeEmail { wallet, address, events: EnabledEventFlags, threshold }`
- `SigilTx::UnsubscribeEmail { wallet }`

Both signed by the wallet (proving they own the address binding intention — the address itself can be unverified at sub time; first OTP confirms).

When an EmailEvent fires, `sigil-tx` looks up `state.email_opt_ins.get(&wallet)`, checks the relevant flag bit, then hands `(address, event)` to the configured `EmailSender`. The chokepoint (`commit_state_transition`) does NOT call email send directly — that would couple consensus to email-deliverability. Instead, a sidecar service drains a `mail_queue: Vec<(EmailAddress, EmailEvent)>` from the state machine and fires asynchronously. State stays deterministic; email delivery is opportunistic.

---

## Out of scope for v0

- **Inbox UI** — Quillon's Slint email.slint is a 4-pane mail reader. SIGIL's wallet can land that later.
- **Multi-account aliases per wallet** — first version: one address per wallet. Multi-alias is a v1 patch.
- **Encrypted at-rest mailboxes** — inbound mail stored as plaintext blobs in flux-db. Encryption (e.g. via wallet pubkey-derived key) is post-MVP.
- **Anti-spam beyond DKIM/SPF/DMARC** — no Bayesian filter, no RBL checks, no rate limits. Operators eat the spam until v1.
- **Mailing lists / group addresses** — `validators@sigilgraph.com` as a fan-out alias. Phase 2.

---

## Phased delivery

| Phase | Scope | Owner | Est |
|---|---|---|---|
| EMAIL-A | flux-email scaffold (lib.rs + send.rs + event.rs stubs, RelaySender + InMemorySender, 4 EmailEvent variants + render templates) | rocky (claiming) | half-day |
| EMAIL-B | `sigil-email` chain consumer + `SubscribeEmail`/`UnsubscribeEmail` txs + chokepoint queue + IncomingSwapOutput + LargeBalanceChange firing | open | 1 day |
| EMAIL-C | AuthCode flow + web wallet sign-in integration | open | 1 day |
| EMAIL-D | BlockProduced trigger + validator opt-in (depends Track A flux-mining port) | open (blocked) | half-day |
| EMAIL-E | Port `address.rs` + NicknameResolver + `ClaimNickname` tx + reserved-name list at genesis | open | half-day |
| EMAIL-F | Port `smtp.rs` (811 LOC inbound SMTP server, HELO/STARTTLS/AUTH/MAIL FROM/RCPT TO/DATA state machine) | open | 2 days |
| EMAIL-G | Port `mta.rs` (335 LOC outbound MTA delivery loop, MX resolution + relay fallback) | open | 1 day |
| EMAIL-H | Port `auth.rs` (wallet-keyed SMTP AUTH) + DKIM signing + operator-config keys | open | 1-2 days |
| EMAIL-I | Operator runbook (`flux/docs/flux-email-operator-runbook.md` — DNS/MX/SPF/DKIM/DMARC setup, certbot for STARTTLS cert) | open | half-day |

**Total: ~7-8 working days end-to-end, parallel across 2-3 agents.**

EMAIL-A unblocks all downstream — get it green first.

---

## Open questions

1. **Default outbound relay** — production SIGIL nodes should they expect operator to supply `FLUX_SMTP_RELAY`, or should we bundle a sane default (e.g. `smtp.sendgrid.net` with operator-supplied API key)? Vote: *operator-supplied; no hardcoded vendor default.*
2. **Auth code length** — 6 digits (current Quillon) vs 8? 6 is industry-standard, sufficient for 5-min validity. Vote: *6*.
3. **Nickname reservation list at genesis** — which names get reserved-at-birth (rocky, codex, adrian, viktor, postmaster, admin, abuse, noreply, support)? Vote: *role-account names + Viktor's chosen agent nicknames. List in genesis transition as `BlockedNickname` rows.*
4. **Address validation discipline** — RFC 5321 strict, or permissive? Vote: *RFC 5321 strict at flux-email; chain can pre-filter.*

— rocky 🟠
