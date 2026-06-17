# Kravspecifikation — Grok Web Chat with MCP Connectors (Flux-API backend)

**Document version:** 1.0 · **Date:** 2026-06-17 · **Owner:** Viktor / Quillon Graph
**Status:** Ready for implementation · **Backend:** flux-api (fluxc-served)

---

## Table of Contents
1. Introduction
2. Overall Description
3. Functional Requirements
4. Non-Functional Requirements
5. System Architecture & Tech Stack
6. Key UI Requirements
7. Core Data Model
8. Security & Compliance
9. Testing Strategy
10. Risks & Mitigations
11. Phased Implementation Roadmap

---

## 1. Introduction

**1.1 Purpose.** Specify a production-grade, web-based chat application powered by
**Grok** (xAI) with first-class **MCP (Model Context Protocol)** connector support, so
users bring their own remote MCP servers and Grok calls their tools server-side.

**1.2 Scope.** A multi-user web app: X-login auth, encrypted xAI key + MCP-secret
storage, per-conversation connector scoping, streaming chat with tool-call
visualization, and a **Flux-API** backend that proxies xAI and injects MCP configs.

**1.3 Definitions.**
- **Grok** — xAI's frontier model family; API at `/responses` (+ SDK).
- **MCP** — Model Context Protocol; standard for exposing tools/resources to models.
- **Remote MCP / BYO-MCP** — user-supplied MCP server config passed in the `tools`
  array; xAI handles discovery, context injection, and server-side execution.
- **flux-api** — the fluxc-generated REST/WS service that fronts xAI and MCP storage.

**1.4 References.** xAI API docs (Remote MCP Tools, `/responses`); MCP spec; this repo's
`flux-api` roadmap (`flux/docs/flux-api-roadmap.md`).

## 2. Overall Description

**2.1 Product perspective.** A thin, secure orchestration layer over xAI. The app
stores connectors and injects them per request; it does **not** run a local tool-calling
loop — xAI executes remote MCP tools server-side.

**2.2 High-level features.** Auth · connector CRUD · streaming chat · model selection ·
vision/image-gen · tool-call cards · conversation management · public connector directory.

**2.3 Personas.** *Power user* (wires personal MCP servers), *Team* (shared connectors +
audit), *Builder* (tests/discovers MCP tools, publishes to directory).

## 3. Functional Requirements

**Authentication**
- **FR-001** Sign in with X (OAuth2). Session via httpOnly secure cookie + rotating refresh.
- **FR-002** Store the user's xAI API key **encrypted at rest** (AES-256-GCM, per-user DEK
  wrapped by a KMS/root key). Key is never returned to the client after save.
- **FR-003** Allow key rotation/removal; invalidate cached derivations on change.

**MCP Connector Management (core)**
- **FR-010** CRUD on connectors: `{name, url, transport(SSE|HTTP), auth(header/bearer/none),
  secrets, description, scopes}`.
- **FR-011** Encrypt all connector secrets at rest (same scheme as FR-002).
- **FR-012** **Per-conversation scoping** — a conversation activates a subset of the user's
  connectors; only those are injected into that request's `tools`.
- **FR-013** **Test & discovery** — on save, probe the server, list discovered tools, render
  names/descriptions/input schemas; surface failures (timeout, auth, TLS) clearly.
- **FR-014** **Public directory** — optionally publish a connector (URL + schema, **never**
  secrets) for others to clone into their own account.
- **FR-015** Least-privilege: per-connector tool allowlist (user may disable individual tools).

**Chat Experience**
- **FR-020** Real-time **streaming** responses (SSE/WebSocket), token-by-token.
- **FR-021** Model selection (Grok variants) + per-conversation system prompt.
- **FR-022** Vision (image upload) and image generation where the model supports it.
- **FR-023** **Tool-call visualization** — a card per tool call showing connector, tool name,
  arguments, result, and latency; collapsible, copy-able.
- **FR-024** Conversation management — list, rename, search, delete, branch/fork, export.

**Backend Integration (flux-api)**
- **FR-030** Proxy xAI `/responses`; never expose the xAI key to the browser.
- **FR-031** **Dynamic MCP injection** — assemble the `tools` array from the conversation's
  active connectors (decrypt secrets in-process, inject, zeroize).
- **FR-032** Parse/enrich the xAI stream: split text deltas from tool-call events; forward an
  enriched event stream the frontend renders as cards.
- **FR-033** SSRF guard on every MCP URL (see NFR/Security).

## 4. Non-Functional Requirements

- **NFR-Perf** First token < 800 ms p50 / < 2 s p95; connector test < 5 s with timeout.
- **NFR-Scale** Stateless API workers (worker-per-core, flux-api), horizontal scale; chat
  state in PostgreSQL; streams not buffered server-side.
- **NFR-Sec** Encryption at rest for *all* secrets; SSRF protection; least-privilege tools;
  audit of secret access; no secret in logs.
- **NFR-Usability** WCAG 2.2 AA; responsive (mobile→desktop); light/dark + theming.
- **NFR-Maint** Typed end-to-end; OpenAPI from flux-api (`flux_api_generate`); migrations
  versioned; connectors schema-validated.
- **NFR-Reliability** Graceful degradation if a connector is down (skip + warn, don't fail
  the whole turn).

## 5. System Architecture & Tech Stack

**5.1 Stack.**
- **Frontend:** Next.js 15 + Tailwind + shadcn/ui; SSE/WebSocket client.
- **Backend:** **flux-api** (fluxc-served, worker-per-core; the Quillon-native alternative
  to FastAPI/NestJS) exposing REST + a streaming endpoint.
- **DB:** PostgreSQL + Prisma (or flux-db for the Flux-native deployment).
- **Secrets:** AES-256-GCM at rest, per-user DEK wrapped by a root key (KMS/env-sealed).

**5.2 Data flow (MCP-enabled turn).**
```
Browser ──POST /chat (conv_id, message)──▶ flux-api
  flux-api: load conv → active connectors → decrypt secrets
           → build tools[] (remote MCP configs) → call xAI /responses (stream)
  xAI: discovers MCP tools, executes server-side, streams text + tool events
  flux-api: parse stream → enrich (connector/tool/args/result/latency)
           → SSE back to Browser → persist messages + tool calls
```

**5.3 Internal MCP config schema (injected into `tools[]`).**
```json
{ "type": "mcp", "server_label": "<name>", "server_url": "<https url>",
  "headers": { "Authorization": "Bearer <decrypted>" },
  "allowed_tools": ["toolA","toolB"] }
```

## 6. Key UI Requirements

- **Login / onboarding** — X sign-in → paste xAI key (masked) → optional first connector.
- **Connectors page** — list with health badges; add/edit drawer with live test + discovered
  tools; per-tool allowlist toggles; "Publish to directory" action.
- **Chat view** — streaming message list; **tool-call cards** inline; model picker; connector
  multi-select for the conversation; drag-drop images; **command palette** (⌘K) for
  conversations/connectors/models.

## 7. Core Data Model

- **User** `(id, x_handle, xai_key_enc, created_at)`
- **Conversation** `(id, user_id, title, model, system_prompt, active_connector_ids[], created_at)`
- **Message** `(id, conv_id, role, content, created_at)`
- **ToolCall** `(id, message_id, connector_id, tool_name, args_json, result_json, latency_ms)`
- **MCP_Connector** `(id, user_id, name, url, transport, auth_type, secrets_enc,
  allowed_tools[], is_public, created_at)`
- **Audit_Log** *(optional)* `(id, user_id, action, target, ts)` — esp. secret access.

## 8. Security & Compliance

- All secrets (xAI key, MCP auth) **encrypted at rest**; decrypted only in-process per
  request, then zeroized. Never logged, never returned to client.
- **SSRF protection** on MCP URLs: HTTPS-only, public-IP-only (block RFC1918/link-local/
  metadata `169.254.169.254`), DNS-rebind guard (resolve+pin), redirect cap.
- **Least privilege:** per-connector tool allowlist; connectors scoped per conversation.
- Tenant isolation: every query bound by `user_id`; signed, short-lived sessions.
- Compliance: GDPR data-export/delete; secrets purged on key removal.

## 9. Testing Strategy

- **Unit:** encryption round-trip + zeroize; SSRF guard (allow/deny matrix); tools[] assembly.
- **Integration:** mock MCP server (discovery, auth-fail, timeout); xAI stream parsing
  (text vs tool events); per-conversation scoping correctness.
- **E2E:** login → add connector → chat with a tool call rendered as a card → fork → delete.
- **Security:** SSRF attempts (metadata IP, rebind), secret never in logs/responses,
  cross-tenant access denied.
- **Load:** streaming concurrency at target p50/p95; connector-down degradation.

## 10. Risks & Mitigations

| Risk | Mitigation |
|---|---|
| Secret leak | Encrypt-at-rest + in-proc-only decrypt + zeroize + log scrubbing |
| SSRF via MCP URL | HTTPS-only, IP allowlist, DNS-rebind pin, redirect cap |
| xAI API / MCP schema drift | Version-pin, contract tests, OpenAPI regen via flux-api |
| Connector down mid-turn | Skip + warn, never fail the whole turn (NFR-Reliability) |
| Cost runaway | Per-user rate limits + token budgets + usage dashboard |

## 11. Phased Implementation Roadmap

- **Phase 1 — MVP (≈ weeks 1–3):** X-login, encrypted xAI key, single-connector chat with
  streaming, basic tool-call card. flux-api skeleton + DB + encryption.
- **Phase 2 — Connectors (≈ weeks 3–5):** full connector CRUD, test/discovery, per-conversation
  scoping, per-tool allowlist, SSRF guard hardened.
- **Phase 3 — Experience (≈ weeks 5–7):** model picker, vision/image-gen, conversation
  search/fork/export, command palette, theming/WCAG.
- **Phase 4 — Polish & v1 (≈ weeks 7–8):** public connector directory, audit logs, load-hardening,
  observability, docs. **Target: polished v1 in ~6–8 weeks.**

---

*Backend note:* the entire API tier is intended to be **flux-api** (fluxc-served,
worker-per-core), with the OpenAPI surface generated via `flux_api_generate` and the MCP
proxy/stream-enrichment as the first flux-api service to dogfood the Grok connector path.
