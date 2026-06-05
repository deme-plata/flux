# flux-api roadmap — v0.12 → v0.17

**Goal:** Close the gap to Stainless on REST surface, **win decisively on the live event-surface axis** (WebSocket discriminated unions from Rust enums — Stainless doesn't ship this).

**Scope:** This roadmap covers the `flux-api` crate at `crates/flux-api/`. Each version is a swarm-coordinated milestone with clean per-claim file scope so two agents can work in parallel without collisions.

**Authoring conventions:**
- Every sub-task lists `Files:` so swarm claims don't collide. Two agents working the same version pick disjoint sub-tasks.
- Every version has an **Acceptance** block. Don't `flux_swarm_complete` your sub-task until the version's tests for it pass via the test binary (`./target/debug/deps/flux_api-<hash>`) — not via `flux_combo`, which silently zeros tests when test build fails (see `feedback-flux-combo-zero-tests-means-test-build-failed`).
- Bumping the workspace version `[workspace.package] version` is the LAST step of each milestone, owned by whoever ships the final sub-task.

**Coordination protocol:**
1. Both agents register with `flux_swarm_register` using distinct `agent_id`s.
2. Both register a webhook with `flux_webhook_register` (per `feedback-flux-swarm-use-webhooks`).
3. Before editing, `flux_swarm_claim crates=["flux-api"] files=[<the files you'll touch>] priority=<7..9>`.
4. Heartbeat `flux_swarm_status` every ~10 min during long stretches.
5. On finish: `flux_swarm_complete agent_id=<you> task_id=<id> success=true`.
6. On stuck: `flux_swarm_release` and post why in a memory or comment.

The swarm state lives at `/tmp/flux-swarm.json` — same box only. If the partner session is on a different machine, fall back to claim-by-PR / claim-by-message.

---

## v0.12.0 — Schema-first OpenAPI (foundation)

**Why:** Today `ApiSchema { type_name, format, nullable }` is too flat to describe a real REST surface. v0.12 makes the schema model rich enough that generated OpenAPI passes a `openapi-validator`, and SDKs can carry typed bodies/responses.

**Pre-step (rocky owns, blocks A/B/C):** **Module split.**
Move from monolithic `src/lib.rs` to:
```
src/
  lib.rs            ← re-exports + tiny glue
  schema.rs         ← ApiSchema, ApiParameter, ApiResponse, ApiEndpoint, HttpMethod
  discover.rs       ← discover_endpoints, known_patterns, heuristic_scan
  openapi.rs        ← generate_openapi
  sdk_ts.rs         ← generate_typescript_sdk
  sdk_python.rs     ← generate_python_sdk
  event_types.rs    ← EventVariant, flux_ue_bridge_events, generate_ts_event_types
```
This unblocks parallel work — sub-task A/B/C/D claim disjoint files.

### Sub-tasks

| ID | Files | What | Est QUG |
|---|---|---|---|
| `v0.12-A` | `src/schema.rs` | Extend `ApiSchema` to a sum type: `Primitive { ty, format }` \| `Object { props: BTreeMap<String, ApiSchema>, required: Vec<String> }` \| `Array { items }` \| `Enum { values }` \| `OneOf { variants }` \| `Ref { name }`. Keep `nullable` as a wrapper. Tests for each variant. | 0.5 |
| `v0.12-B` | `src/discover.rs` | Rework `ApiParameter`/`ApiResponse` to carry the new `ApiSchema`. Update `known_patterns` to attach real schemas (path/query params + JSON body for POSTs). | 0.5 |
| `v0.12-C` | `src/openapi.rs` | Rewrite `generate_openapi` to emit `components/schemas` + `$ref` for objects, plus `parameters` array with `in` + `schema`. | 0.5 |
| `v0.12-D` | `src/tests/openapi_golden.rs` (new) | Golden-snapshot tests: run the existing known_patterns through the new pipeline; check output validates against an `openapi-validator` (depend on `oas3` crate). | 0.5 |

**Acceptance:**
- `oas3::from_str(&generate_openapi(...).to_string()).is_ok()` for every known crate.
- All v0.11 tests still pass + new schema tests.
- Bump `version = "0.12.0"` in workspace Cargo.toml — last sub-task to merge owns the bump.

---

## v0.13.0 — Macro-driven endpoint registration

**Why:** Heuristic `known_patterns` doesn't scale. v0.13 makes endpoints declare themselves where they're defined.

### Sub-tasks

| ID | Files | What | Est QUG |
|---|---|---|---|
| `v0.13-A` | new crate `crates/flux-api-macros/` | Proc-macro crate exporting `#[flux::api(GET, "/path", summary = "...")]`. Expands into an `inventory::submit!` of an `ApiEndpointDescriptor`. | 0.8 |
| `v0.13-B` | `crates/flux-api/src/discover.rs` | `discover_endpoints_static() -> Vec<ApiEndpoint>` that drains the inventory and merges with `known_patterns`. | 0.5 |
| `v0.13-C` | `crates/flux-ue-bridge/src/main.rs` | Dogfood: annotate the 3 bridge endpoints with `#[flux::api]`, drop their `known_patterns` entry. | 0.5 |
| `v0.13-D` | `crates/flux-api/tests/macro_e2e.rs` | trybuild test that the macro expands; integration test that the bridge's endpoints survive a round-trip. | 0.5 |

**Acceptance:**
- `cargo expand` on the bridge shows the inventory submission.
- `discover_endpoints_static()` returns the same 3 bridge endpoints as v0.12's `discover_endpoints`.
- Bump to `0.13.0`.

---

## v0.14.0 — Multi-language SDK gen (Go + Rust client + Kotlin)

**Why:** TS + Python only is the baseline. Stainless ships 6+; we should match at least 5.

### Sub-tasks

| ID | Files | What | Est QUG |
|---|---|---|---|
| `v0.14-A` | `src/sdk_go.rs` | `generate_go_sdk(&[ApiEndpoint], base_url) -> String`. Per-tag struct client, idiomatic Go (Context-first, `*http.Client` injectable). | 0.6 |
| `v0.14-B` | `src/sdk_rust.rs` | `generate_rust_client_sdk(...)`. `reqwest::Client` based, typed `Result<T, Error>`. | 0.6 |
| `v0.14-C` | `src/sdk_kotlin.rs` | `generate_kotlin_sdk(...)`. Coroutine-based, OkHttp. | 0.6 |
| `v0.14-D` | `tests/sdk_compile_each.rs` | For each language, write the generated SDK to a tempdir, invoke the language's compiler (go/cargo/kotlinc) on a smoke main, assert exit 0. Skip language if compiler missing. | 0.5 |

**Acceptance:**
- Each `generate_X_sdk` produces a file that the corresponding compiler accepts.
- `0.14.0` bump.

---

## v0.15.0 — Middleware: auth, retry, pagination, streaming

**Why:** A bare `fetch()` isn't an SDK. Real users want bearer auth, 429-retry, cursor pagination, streaming responses.

### Sub-tasks

| ID | Files | What | Est QUG |
|---|---|---|---|
| `v0.15-A` | `src/middleware/mod.rs` (new dir) | `MiddlewareSpec { auth: AuthKind, retry: RetryPolicy, pagination: PaginationStyle, streaming: StreamKind }`. Wire into `ApiEndpoint`. | 0.5 |
| `v0.15-B` | `src/sdk_ts.rs`, `src/sdk_python.rs` | Emit interceptor / Authorization header, exponential-backoff retry on 429+5xx, `iter_pages()` helper when pagination=Cursor, `async for chunk in client.stream(...)` when streaming=SSE. | 0.8 |
| `v0.15-C` | `src/sdk_go.rs`, `src/sdk_rust.rs`, `src/sdk_kotlin.rs` | Same middleware concepts, lang-idiomatic. | 0.8 |
| `v0.15-D` | `tests/middleware_smoke.rs` | Stand up a small `wiremock`/`mockito` server in tests; verify retry actually retries, auth header attaches, pagination iterates. | 0.5 |

**Acceptance:**
- Retry test sees 3 attempts on a 429-streak.
- Auth header lands on every request.
- Pagination iterates a 3-page mock cleanly.
- `0.15.0` bump.

---

## v0.16.0 — Event surface as first-class (THE MOAT)

**Why:** Stainless is REST-centric. v0.16 makes flux-api the canonical way to generate typed WebSocket / SSE event clients from a Rust enum across all 5 languages. This is the axis we win on.

### Sub-tasks

| ID | Files | What | Est QUG |
|---|---|---|---|
| `v0.16-A` | `src/event_types.rs` | Extend `EventVariant`: nested `EventField { name, ty: EventFieldType }` where `EventFieldType` mirrors `ApiSchema` (Primitive/Object/Array/Enum/Optional). Lower from a Rust `#[serde(tag = "type")]` enum via a sibling proc-macro `#[flux::event]`. | 1.0 |
| `v0.16-B` | `src/event_sdk/{ts,py,go,rust,kotlin}.rs` | One discriminated-union generator per language. TS uses union types, Python uses `typing.Union` + `pydantic`, Go uses interface + type-switch, Rust re-exports the original enum, Kotlin uses sealed classes. | 1.0 |
| `v0.16-C` | `crates/flux-ue-bridge/src/main.rs` | Replace the hand-written `Event` enum maintenance with `#[flux::event]` on the variants; regenerate `flux-api`'s `flux_ue_bridge_events()` from the same enum (single source of truth). | 0.5 |
| `v0.16-D` | `tests/event_roundtrip.rs` | For each language, gen the union, write a tiny program that decodes a sample JSON event and pattern-matches the tag, assert it runs. | 0.5 |

**Acceptance:**
- Round-trip: Rust enum → generator → 5 language types → each lang program decodes the same JSON without divergence.
- `#[flux::event]` on the bridge's `Event` enum compiles + is the source of truth.
- `0.16.0` bump.

---

## v0.17.0 — CI publish + docs site + semver-on-schema-diff

**Why:** Generated code that doesn't auto-publish is a manual-labour treadmill. v0.17 wires the package-registry side.

### Sub-tasks

| ID | Files | What | Est QUG |
|---|---|---|---|
| `v0.17-A` | `crates/flux-api/src/publish.rs` + `fluxc-mcp/src/handlers/api.rs` | `flux_api_publish` MCP tool: bundles per-lang SDK + uploads to npm / PyPI / crates.io / Go module proxy / Maven Central via tokens in `FLUX_PUBLISH_TOKENS_*` env. Dry-run by default. | 1.0 |
| `v0.17-B` | `crates/flux-api/src/docsite.rs` | Static HTML generator: combines OpenAPI + event-type union into `quillon.xyz/api/<crate>/index.html`. Embeds the JSON; renders the same theme as fluxc dashboard. | 0.8 |
| `v0.17-C` | `crates/flux-api/src/diff.rs` | Schema-diff: compare two `OpenAPI` values, classify changes as `Patch | Minor | Major` (breaking field removals → major, additive → minor, doc-only → patch). Drives the workspace version bump. | 0.6 |
| `v0.17-D` | `tests/publish_dry.rs`, `tests/docsite_render.rs`, `tests/diff_categorize.rs` | Dry-run publish lands the tarball in a tempdir; docsite renders a non-empty HTML; diff catches a removed field as Major. | 0.5 |

**Acceptance:**
- `flux_api_publish dry_run=true` produces the right tarballs without touching real registries.
- Docsite for `flux-ue-bridge` renders + lists 3 endpoints + 5 event variants.
- `diff(old, new)` returns `Major` when a required field is dropped.
- `0.17.0` bump.

---

## Stretch targets (not part of the 6-version plan, but worth noting)

- **JSON-Schema input mode** — accept a JSON Schema as well as a `WorkspaceGraph`, for non-Rust services.
- **GraphQL subgraph export** — emit a graphql-typescript schema from the same source.
- **Federated API gateway** — `fluxc serve` mounts every registered crate's OpenAPI under `/api/<crate>` automatically.
- **Stripe-style versioned APIs** — `Api-Version` header pins a snapshot; codegen produces N clients, one per version.

---

## Where to find / track this

- **Live state:** `cat /tmp/flux-swarm.json | python3 -c "import json,sys; ..."` (see roadmap memory for parser).
- **Memory entry:** `project-flux-api-roadmap-v0_12-v0_17` (this doc's pointer).
- **Partner-session start prompt:** `start-prompt-flux-api-partner` (paste into the second terminal).
