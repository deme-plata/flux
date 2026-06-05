# flux-vcs + flux-oauth2 — GitHub-shape code hosting on Flux

> *"Improve flux versioning control with invite feature like github using fluxmail and oauth2."*
> — Viktor, 2026-05-29 late
>
> **Substrate-level**: lives at `flux/crates/flux-vcs/` + `flux/crates/flux-oauth2/`. Any Flux chain can host repos + invite collaborators by email + sign users in via OAuth2. The first consumer is SIGIL (`code.sigilgraph.com`); future siblings inherit it free.
>
> **Authored:** rocky, 2026-05-29. **Status:** Draft scope.

---

## What we already have

- **`code.quillon.xyz/repo.git`** — Quillon's git-http-backend on Beta. Read+write, HTTPS, no per-repo permissions, no invites, no UI. Cert expired 2026-05-08; HTTPS route is broken (CLAUDE.md). The git daemon at `git://185.182.185.227:9418` still works.
- **`code.sigilgraph.com`** — domain reserved per [[project-sigil-chain]] §17, nothing deployed.

What we don't have: a place where a user creates a repo, invites a collaborator by email, the collaborator clicks a link, signs in with their wallet, and gets push access. That's the gap.

---

## End-to-end story

1. **Viktor** (nickname `viktor`, wallet `qnkefca…0723`) opens `https://code.sigilgraph.com`, signs in via OAuth2 → Flux SQIsign challenge against his wallet pubkey → session cookie.
2. **Viktor** creates a repo `viktor/quillon-island`. flux-vcs writes `/var/flux-vcs/viktor/quillon-island.git` with a default `main` branch and stamps the owner role to Viktor's wallet in `flux-db`.
3. **Viktor** clicks "Invite a collaborator" → form: nickname OR email + role (admin/write/triage/read).
4. **Viktor** enters `adrian@sigilgraph.com` + role=write. flux-vcs:
   - Generates a one-time invite token (32-byte random, hashed at rest, plain in email).
   - Records `Invite { repo, inviter, invitee_address, role, expires_at, token_hash }` in flux-db.
   - Hands `(adrian@sigilgraph.com, InviteEmail { repo, inviter, role, accept_url })` to **flux-email**.
   - flux-email's `RelaySender` ships it.
5. **Adrian** opens the email. Subject: *"viktor invited you to quillon-island on SIGIL Code"*. Clicks accept_url.
6. **Adrian** lands on `https://code.sigilgraph.com/invites/accept?token=...`. He's not signed in → redirect to flux-oauth2 `/authorize` → SQIsign challenge.
7. **Adrian's** browser wallet signs the challenge (proving the `adrian` nickname's wallet is who's clicking). flux-oauth2 issues access + refresh tokens.
8. flux-vcs `/invites/accept` (now with Adrian's wallet attested) verifies the token, marks the invite consumed, writes `Role { repo, wallet: adrian, role: write }` to flux-db.
9. **Adrian** can now `git push` to `code.sigilgraph.com:viktor/quillon-island`. The git smart-HTTP backend reads the OAuth2 access token from `Authorization: Bearer ...`, looks up the wallet, checks the role.

Step 4's email is the *exact* same flow the existing `flux-email::InviteRepoCollab` event would already fire — see EMAIL-A scaffold today, just one new variant added to `EmailEvent`.

---

## Architecture

```
flux/crates/
├── flux-oauth2/                — RFC 6749 + RFC 7636 (PKCE) + RFC 7662
│   └── src/
│       ├── lib.rs              — Provider trait + ClientConfig + token types
│       ├── provider.rs         — /authorize, /token, /introspect, /revoke handlers
│       ├── pkce.rs             — code_challenge / code_verifier (S256)
│       ├── token.rs            — opaque token mint + verify + refresh
│       └── wallet_auth.rs      — SQIsign challenge → access token (the auth step)
├── flux-vcs/                   — repo hosting + roles + invites
│   └── src/
│       ├── lib.rs              — Repo + Role + Invite types
│       ├── repo.rs             — git wrapper (gitoxide), CRUD + clone-URL gen
│       ├── role.rs             — role enum, permission resolver
│       ├── invite.rs           — invite create / accept / expire
│       ├── http.rs             — smart-HTTP backend (info/refs, upload-pack, receive-pack)
│       ├── email_glue.rs       — InviteRepoCollab event → flux-email handoff
│       └── auth.rs             — OAuth2 access-token → wallet resolution
└── flux-vcs-cli/               — `flux-vcs` admin CLI (init store, list repos, rotate keys)
    └── src/main.rs
```

Two new flux-email event variants land in EMAIL-A's `event.rs`:

```rust
EmailEvent::InviteRepoCollab(InviteRepoCollabEvent {
    inviter_wallet: WalletId,
    inviter_nickname: String,
    repo_name: String,
    role: String,
    accept_url: String,
    expires_at_us: u64,
}),
EmailEvent::RepoAccessGranted(RepoAccessGrantedEvent {
    repo_name: String,
    role: String,
    accepted_by: String,
}),
```

A consequence: this is the first time `flux-email` ships its EMAIL-A surface to a real production flow. The substrate's email layer is now load-bearing for code hosting.

---

## Why now (and not after P6 soak)

P6 is *infra readiness*: chain survives unsupervised. flux-vcs is *workflow readiness*: contributors can join without a face-to-face SSH-key handoff. Both are pre-beta, both are foundational.

Doing them in parallel makes sense because:
- The OAuth2 work unblocks SIGIL wallet web sign-in (EMAIL-C in the email roadmap). Same crate, two consumers.
- flux-vcs unblocks distributed development on SIGIL itself (right now every PR happens through SSH-key access to Beta; that doesn't scale to external contributors).
- The invite flow exercises EMAIL-A end-to-end — first real production use of flux-email.

flux-vcs and flux-chronos are independent; flux-vcs and EMAIL-A/B/C are interlocked (vcs needs the email events; oauth2 powers the auth code flow EMAIL-C drives).

---

## Phased delivery

### flux-oauth2

| Phase | Scope | Est | Owner |
|---|---|---|---|
| OAUTH2-A | Scaffold + opaque token mint + flux-db token store + 8 unit tests | half day | rocky (claiming POC) |
| OAUTH2-B | Authorization Code + PKCE state machine (RFC 7636) | 1 day | open |
| OAUTH2-C | Wallet auth path (SQIsign challenge → access token) | 1 day | open |
| OAUTH2-D | `/authorize` `/token` `/introspect` `/revoke` HTTP handlers | 1 day | open |
| OAUTH2-E | Refresh token + revocation + session cookie (12h sliding window) | half day | open |
| OAUTH2-F | OpenAPI 3.1 spec + curl playground | half day | open |

### flux-vcs

| Phase | Scope | Est | Owner |
|---|---|---|---|
| VCS-A | Repo CRUD via gitoxide + flux-db role-store + 10 unit tests | 1 day | open (blocked by OAUTH2-A) |
| VCS-B | Smart-HTTP backend (`info/refs`, `git-upload-pack`, `git-receive-pack`) | 1.5 days | open (blocked by OAUTH2-A) |
| VCS-C | Invite create / accept / expire flow + integration with flux-email | 1 day | open (blocked by EMAIL-A consumer + VCS-A) |
| VCS-D | Web UI v0 — single-page wallet, with repo list + invite form + accept screen | 1.5 days | open (blocked by VCS-C) |
| VCS-E | Admin CLI (`flux-vcs init`, `flux-vcs add-user`, `flux-vcs rotate-host-key`) | half day | open |
| VCS-F | Operator runbook (DNS for `code.sigilgraph.com`, TLS cert via certbot, systemd unit) | half day | open |

**Total: ~9-10 days parallel across 2-3 agents.** OAUTH2-A + VCS-A can land in the same session (POC level).

---

## What flux-vcs deliberately does NOT include (yet)

- **Issues + PR review UI** — gitlab-shape collaboration. v1 is push/pull/role only. Later.
- **CI/CD pipeline integration** — flux-arena-agent style "trigger build on push" is a sibling crate; not v0.
- **Code search** — flux-search already exists in the workspace (`flux-search` 1369 LOC); future patch wires it to crawl repo HEADs. Not v0.
- **Notifications beyond invite** — push/merge events would extend the EmailEvent enum further; deferred to v1.
- **SSH protocol** — Git over SSH requires hosting an ssh server with custom auth. HTTPS-only for v0.

---

## What flux-oauth2 deliberately does NOT include

- **Federation** — login-with-Google / login-with-GitHub. v0 is wallet-only.
- **SAML / SCIM** — enterprise SSO. v0 is wallet-only.
- **Account recovery** — wallet IS the account; lose the wallet, lose the account. v1 might wire a "secondary wallet" recovery path.

---

## Naming + DNS

- `code.sigilgraph.com` — flux-vcs web UI + git HTTPS
- `auth.sigilgraph.com` — flux-oauth2 authorize / token endpoints
- `wallet.sigilgraph.com` — SIGIL browser wallet (the OAuth2 user-agent for code.sigilgraph.com sign-in)

All three terminate on Epsilon (via flux-flux reverse proxy, same pattern as `quillon.xyz`).

---

## Open questions

1. **Token format** — opaque (random bytes, flux-db lookup) vs JWT (self-contained)? JWT scales without DB roundtrips; opaque is easier to revoke. Vote: *opaque for v0 — invite scale won't bottleneck on DB, and revocation matters when collab roles change*.
2. **Storage layer for repos** — gitoxide-managed `.git/` directories on disk vs flux-db CFs storing pack files? On-disk is the well-trodden path. Vote: *on-disk under `/var/flux-vcs/`, mirrored to flux-db only for role/invite metadata*.
3. **Email branding for invites** — match flux-email's "automated notification" tone or sound chattier? Vote: *match the existing tone — consistency over personality at this stage*.
4. **Nickname uniqueness for invites** — if `viktor` invites `adrian@sigilgraph.com` but `adrian` hasn't claimed the nickname yet, does the invite go to the email address regardless? Vote: *yes — the email is the canonical identity for the invite; nickname binding comes at sign-in if Adrian hasn't claimed `adrian` yet*.

— rocky 🟠
