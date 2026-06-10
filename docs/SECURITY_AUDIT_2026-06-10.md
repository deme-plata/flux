# Flux Security Audit — 2026-06-10

Workspace: v0.27.0, 138 crates. Auditor: rocky (Claude, claude-fable-5) — 4 parallel code-level sweeps
(secrets/keys, HTTP/network surface, command execution, crypto+unsafe) + `flux_ai_audit` (96%) +
`flux_version_status` (135/138 consistent).

Issue IDs: `SEC-001`-style, tracked here per the docs/ issue convention.
Status: ⚪ Open → 🔵 In Progress → ✅ Closed

## CRITICAL

### SEC-001 ✅ Shell injection → RCE in flux_sigil_node_deploy
- `crates/fluxc-mcp/src/handlers/sigil_ops.rs:336-342`
- `launch_cmd` MCP arg interpolated raw into the SSH remote shell string.
  `launch_cmd: "...; rm -rf /; #"` = arbitrary root command on Delta.
- FIXED 2026-06-10: `launch_cmd` gated on `safe_cmd_charset()` (rejects all shell
  metacharacters), `host` gated on `safe_host()` in both node_restart and node_deploy.
  Shared helpers in `handlers/mod.rs` with unit tests. Verified flux_combo 63/63.

### SEC-002 ✅ Shell injection → fleet RCE in flux_swarm_cortex
- `crates/fluxc-mcp/src/handlers/swarm_compile.rs:340-345`
- `preset` arg interpolated into inline JSON inside an `ssh_exec` shell string; `"` breaks out.
- FIXED 2026-06-10: preset restricted to identifier chars `[A-Za-z0-9_-]` before use.

### SEC-003 ✅ SQIsign agent private key written world-readable
- `crates/fluxc-core/src/provenance.rs:236`
- `std::fs::write(~/.flux-agent-key.json)` → mode 0644 under default umask. The sk_hex inside is the
  agent's provenance signing identity; any local user can forge all .proof signatures.
- FIXED 2026-06-10: new `write_key_file()` opens with `OpenOptionsExt::mode(0o600)` AND re-tightens
  via `set_permissions(0o600)` (covers pre-existing 0644 files). Live `/root/.flux-agent-key.json`
  chmod'd 0600 out-of-band. provenance tests 5/5.

### SEC-004 ✅ DeepSeek API key visible in /proc/*/cmdline
- `crates/fluxc-core/src/phase3.rs:466`, `crates/flux-cortex/src/ai_cortex.rs:680`
- Key passed to subprocess curl as `-H "Authorization: Bearer <key>"` → visible to every process via
  /proc cmdline / ps.
- FIXED 2026-06-10: both call sites now feed the Authorization header to curl via a stdin curl-config
  (`-K -`, `header = "..."`), so the key never appears in any argv. stderr→null. Builds green.

### SEC-005 ✅ thread_rng() used for long-term cryptographic keys
- `crates/flux-wireguard/src/lib.rs:174,305` (X25519 StaticSecret)
- `crates/flux-zk-p2p/src/lib.rs:102` (proof-system secret key), `anonymous_identity.rs:214` (nonce)
- (`crates/flux-kyberkem/src/lib.rs:74-94` — test code, fixed for hygiene)
- FIXED 2026-06-10: all sites now use `rand::rngs::OsRng`. flux-wireguard 8/8, flux-kyberkem 3/3.

### SEC-006 ✅ SSRF in webhook registration
- `crates/fluxc-core/src/webhook.rs:431-451` (dispatch), `:132-165` (register — no URL validation)
- Registered webhook URLs are POSTed to unconditionally: can target 127.0.0.1:8080 (q-api-server),
  169.254.169.254, internal services.
- FIXED 2026-06-10: `ssrf_check`/`ssrf_check_with(url, allow_private)` enforced at the dispatch
  chokepoint `send_http_post` (covers all delivery paths + DNS rebinding + pre-existing entries):
  requires http(s); resolves host and inspects EVERY resolved IP; ALWAYS blocks 169.254.169.254;
  blocks loopback/RFC1918/link-local/unspecified/ULA unless `FLUX_WEBHOOK_ALLOW_PRIVATE=1` (the
  documented same-host fluxc-serve webhook pattern). Strips userinfo so `pub@127.0.0.1` can't smuggle.
  Pure inner fn keeps tests env-free. ssrf_guard test green. Enforced at dispatch (not register) so
  registration stays offline-safe and the quarantine/health tests keep their loopback semantics.

## HIGH

### SEC-007 ✅ Path traversal in /api/proof/<crate>
- `crates/fluxc-core/src/serve.rs:804-827`
- `crate_name` not sanitized; `..` escapes the proof dir.
- FIXED 2026-06-10: `crate_name` restricted to `[A-Za-z0-9_-]` (Cargo's own crate-name charset)
  before any path join — rejects `..`, `/`, `\`.

### SEC-008 ✅ Unbounded Content-Length on fluxc serve
- `crates/fluxc-core/src/serve.rs:1141-1175`
- Re-assessed: the server reads ONE ≤8 KiB buffer, so a body is already structurally capped — there
  is no read-loop, so no OOM/slowloris-via-Content-Length. The real defect was SILENT truncation of a
  body whose declared Content-Length exceeds 8 KiB.
- FIXED 2026-06-10: declared Content-Length > 8 KiB now returns 413 (fail loud) instead of truncating.

### SEC-009 ✅ (injection part) molt handler: shell interpolation of method/path
- `crates/fluxc-mcp/src/handlers/molt.rs:69-92` (DELTA const at :23)
- `method`/`path` were interpolated unescaped into the remote shell string.
- FIXED 2026-06-10: `method` allowlisted (GET/POST/PUT/PATCH/DELETE), `path` gated on
  `safe_url_path()` (no quotes/spaces/metacharacters). `body` was already correctly
  single-quote-escaped for the single remote shell layer (local side passes the command
  as a discrete ssh arg — there is only ONE shell layer, on Delta).
- KEY-HYGIENE HALF FIXED 2026-06-10: the `K=$(jq ...)` shell-variable form (key in Delta's process
  env / history) replaced — jq now emits a curl-config `header` line piped to `curl -K -`, so the
  Moltbook key never lands in any argv or shell variable on Delta. fluxc-mcp 63/63.

### SEC-010 ⚪ SQIsign verify does not enforce Level-5 signature length
- `crates/flux-sqisign/src/lib.rs:29-38`
- `Signature::<Level5>::from_bytes(sig_bytes)` never checks `sig_bytes.len() == 292`; a Level-1
  (148B) sig may deserialize → downgrade risk. `signature_size()` exists but is never consulted.
- Fix: explicit length check before `from_bytes` in both sig and pk paths.

## MEDIUM

### SEC-011 ✅ CRLF header injection through flux_proxy_to
- `crates/fluxc-core/src/serve.rs:1223-1230` — forwarded header values not checked for `\r\n`.
- FIXED 2026-06-10: header parse loop drops any name/value containing CR/LF (reachable via a mid-line
  `\r` that `str::lines()` doesn't strip), AND the proxy forward loop skips them at the sink
  (defense-in-depth). `content-length` match made case-insensitive while there.

### SEC-012 ✅ run_ssh() trusts callers to pre-quote
- `crates/fluxc-mcp/src/handlers/sigil_ops.rs:345-360`
- FIXED 2026-06-10 (contract form): shared `shell_quote()` / `safe_host()` /
  `safe_cmd_charset()` / `safe_url_path()` helpers added to `handlers/mod.rs` (with
  tests), `run_ssh()` doc-comment now binds callers to validate every interpolated
  value. Remote ssh commands are inherently one shell string, so the helper-based
  contract is the structural fix; new SSH-touching handlers must use the helpers.

### SEC-013 ⚪ bincode deserialization of peer blobs without size limit
- `crates/flux-sync/src/lib.rs:144` — malicious peer blob can pre-allocate huge Vec before error.
- Fix: `bincode` options `.with_limit(1_000_000)`.

### SEC-014 ⚪ WASM miner FFI bounds
- `crates/quillonos-q-miner/src/lib.rs:87-88,152-155` — `from_raw_parts(ptr, 32)` with no host length
  param; `:41-44` scratch allocator uses unchecked `+` (use `checked_add`).

### SEC-015 ⚪ Unauthenticated internal-state read endpoints
- `crates/fluxc-core/src/serve.rs:620-633` — /api/stats, /api/xray, /api/goal/current leak workspace/
  peer/build topology when exposed publicly. Decide: public-by-design (dashboard) or gate.

### SEC-016 ⚪ Predictable /tmp/flux-events/webhook_<ts>.json writes
- `crates/fluxc-core/src/serve.rs:582-585` — race/pre-creation on shared /tmp; also no body size cap
  on /api/build_event payloads written verbatim.

### SEC-017 ⚪ Non-constant-time proof comparison (mock code — must not ship)
- `crates/flux-zk-p2p/src/anonymous_identity.rs:246`, `network_membership.rs:256` — `==` on proof
  values. Fine while mock; use `subtle::ConstantTimeEq` when production-ized.

### SEC-018 ⚪ Provenance canonicalization pinning
- `crates/fluxc-core/src/provenance.rs:134-146` — canonical_bundle_bytes() is the (correct) signing
  input; add a test asserting JSON round-trip is NEVER signed, so a future consumer can't regress it.

## LOW

- SEC-019 ⚪ Hardcoded Delta IP in molt.rs:23 (topology disclosure) → env/config.
- SEC-020 ⚪ Stale crate versions: flux-agora-stargate, flux-agora-stargate-mcp (0.2.0), flux-sync
  (0.1.0) → `flux_version_sync`.
- SEC-021 ⚪ flux-cache mmap without size sanity check (`crates/flux-cache/src/lib.rs:317,368`).
- SEC-022 ⚪ HTTP redirect listener not rate-limited (`serve.rs:921-950`).

## Healthy patterns confirmed ✅

- flux-0x / flux-cmc: reqwest `.header()` / `.bearer_auth()` — keys never in argv.
- flux-sigil keyed.rs: proper BLAKE3 domain separation via versioned `derive_key` context.
- serve.rs static files: canonicalize-after-join correctly defends symlink escapes.
- flux-hotswap unsafe: sound (Acquire loads, correct Box::from_raw ownership).
- distributed.rs `shell_quote()` + fleet.rs base64-env: the good patterns to propagate.
- flux-search secret_scrape.rs redaction layer exists.

## Remediation order

1. SEC-001, SEC-002, SEC-009 — MCP→SSH injection class. Highest exploitability: every agent with MCP
   access (incl. non-Claude siblings) can root the fleet with one tool call. Structural fix = SEC-012.
2. SEC-003, SEC-004, SEC-005 — key hygiene, all one-to-few-line fixes.
3. SEC-006, SEC-007, SEC-008 — fluxc serve / webhook surface (matters wherever serve is exposed
   beyond localhost).
4. SEC-010 + SEC-013 + SEC-014 — protocol-level hardening.
5. Mediums/lows opportunistically.

Per CLAUDE.md risk categorization: SEC-001..010 are 🟠 HIGH+ — fixes must go through flux_combo
verification, never raw cargo, and any consensus-adjacent change stays height-gated.
