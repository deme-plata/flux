//! `flux_sigil_wallet_*` — the SIGIL WALLET MCP surface, built on the flux MCP
//! framework ([`ToolRegistry`]) and inspired by the Quillon wallet MCP
//! (create_wallet / import_wallet / get_balance / network_status / mining_status
//! / send / faucet).
//!
//! Talks to the live `sigil-rpcd` JSON API over HTTP **or HTTPS** via `ureq`
//! (rustls), so it works ON-box (`http://127.0.0.1:8099`) or OFF-box against the
//! public endpoint. Set `FLUX_SIGIL_RPC` to the base URL (default
//! `http://127.0.0.1:8099`; public: `https://quillon.xyz:8843`).
//!
//! Reads are live + safe. Key generation is real ed25519 (rpcd's convention:
//! `wallet_id == ed25519 pubkey`). Sends are SIGNED locally with the
//! caller-supplied secret and, by default, merely PROPOSED (returned
//! ready-to-POST) — a real transfer only happens on explicit `"broadcast": true`.
//! The MCP never stores keys.
//!
//! The "flux way" (zerox idiom): the rpcd surface these tools drive is ALSO
//! modeled as flux-api [`ApiEndpoint`]s ([`sigil_wallet_spec`]), so
//! `flux_sigil_wallet_openapi` emits the OpenAPI 3.1 document and
//! `flux_sigil_wallet_sdk` generates typed clients (TS/Python/Rust/Go/Kotlin)
//! from the same definitions. Response schemas mirror the live sigil-rpcd
//! handlers (sigil/crates/sigil-rpc/src/bin/sigil-rpcd.rs), probed 2026-07-05.

use flux_api::{ApiEndpoint, ApiParameter, ApiResponse, ApiSchema, HttpMethod, ParamLocation};
use serde_json::{json, Value};

use crate::handlers::{ToolDef, ToolRegistry};

const AUTH_DOMAIN: &str = "sigil-rpc/v1";

fn rpc_base() -> String {
    std::env::var("FLUX_SIGIL_RPC").unwrap_or_else(|_| "http://127.0.0.1:8099".to_string())
}
fn arg_str(a: &Value, k: &str, d: &str) -> String {
    a.get(k).and_then(|v| v.as_str()).unwrap_or(d).to_string()
}
fn arg_u64(a: &Value, k: &str, d: u64) -> u64 {
    a.get(k)
        .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .unwrap_or(d)
}
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Read the body of a ureq response, treating an HTTP error status as data (rpcd
/// returns useful JSON in its 400s).
fn body_of(resp: Result<ureq::Response, ureq::Error>) -> Result<String, String> {
    match resp {
        Ok(r) => r.into_string().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(_c, r)) => r.into_string().map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    }
}
fn get(path: &str) -> String {
    let url = format!("{}{}", rpc_base(), path);
    match body_of(ureq::get(&url).call()) {
        Ok(b) => b,
        Err(e) => json!({"ok": false, "error": e, "endpoint": url, "hint": "set FLUX_SIGIL_RPC to the rpcd base URL"}).to_string(),
    }
}
fn post(path: &str, body: &str) -> Result<String, String> {
    let url = format!("{}{}", rpc_base(), path);
    body_of(ureq::post(&url).set("Content-Type", "application/json").send_string(body))
}

pub fn register(registry: &mut ToolRegistry) {
    registry.register(ToolDef {
        name: "flux_sigil_wallet_create",
        description: "Generate a fresh SIGIL wallet (ed25519). Returns wallet_id (= public key hex) and the SECRET key hex. SAFEGUARD THE SECRET; the MCP does not store it.",
        input_schema: json!({"type":"object","properties":{}}),
    }, wallet_create);

    registry.register(ToolDef {
        name: "flux_sigil_wallet_import",
        description: "Derive a wallet_id (= pubkey) from an existing 32-byte hex secret, to use with balance/send. Args: secret (hex). The secret is not stored.",
        input_schema: json!({"type":"object","properties":{"secret":{"type":"string"}},"required":["secret"]}),
    }, wallet_import);

    registry.register(ToolDef {
        name: "flux_sigil_wallet_balance",
        description: "Live balance of a SIGIL wallet for a token. Args: wallet (hex), token (default 'SIGIL').",
        input_schema: json!({"type":"object","properties":{"wallet":{"type":"string"},"token":{"type":"string","default":"SIGIL"}},"required":["wallet"]}),
    }, wallet_balance);

    registry.register(ToolDef {
        name: "flux_sigil_wallet_network_status",
        description: "Live SIGIL network status: service, network_id, height, peers, tip. (rpcd /status + /tip)",
        input_schema: json!({"type":"object","properties":{}}),
    }, wallet_network_status);

    registry.register(ToolDef {
        name: "flux_sigil_wallet_economy",
        description: "Live SIGIL economy: tokens, pools, traders, citizens, treasury. (rpcd /economy)",
        input_schema: json!({"type":"object","properties":{}}),
    }, |_a| get("/api/v1/economy"));

    registry.register(ToolDef {
        name: "flux_sigil_wallet_pools",
        description: "Live DEX pools on SIGIL. (rpcd /pools)",
        input_schema: json!({"type":"object","properties":{}}),
    }, |_a| get("/api/v1/pools"));

    registry.register(ToolDef {
        name: "flux_sigil_wallet_tokens",
        description: "Live token registry on SIGIL. (rpcd /tokens)",
        input_schema: json!({"type":"object","properties":{}}),
    }, |_a| get("/api/v1/tokens"));

    registry.register(ToolDef {
        name: "flux_sigil_wallet_mining_status",
        description: "Live mining challenge/status for a wallet. Args: wallet (hex). (rpcd /mining/challenge)",
        input_schema: json!({"type":"object","properties":{"wallet":{"type":"string"}},"required":["wallet"]}),
    }, wallet_mining_status);

    registry.register(ToolDef {
        name: "flux_sigil_wallet_recent",
        description: "Recent on-chain activity. Args: kind (optional filter). (rpcd /recent)",
        input_schema: json!({"type":"object","properties":{"kind":{"type":"string"}}}),
    }, |a| {
        let kind = arg_str(a, "kind", "");
        if kind.is_empty() { get("/api/v1/recent") } else { get(&format!("/api/v1/recent?kind={kind}")) }
    });

    registry.register(ToolDef {
        name: "flux_sigil_wallet_faucet",
        description: "Onboard + fund a new wallet from the rpcd faucet (verified-user starter funding, per-IP rate-limited). Args: seed (optional 32-byte hex; rpcd derives one if omitted). Returns the funded wallet. (rpcd POST /onboard)",
        input_schema: json!({"type":"object","properties":{"seed":{"type":"string"}}}),
    }, wallet_faucet);

    registry.register(ToolDef {
        name: "flux_sigil_wallet_send",
        description: "Send SIGIL/tokens. SIGNS locally with the caller's secret (rpcd ed25519 auth). By DEFAULT only PROPOSES (returns the signed request ready to POST). Pass \"broadcast\": true to submit to rpcd /send. Args: from (hex), to (hex), amount (int), secret (hex), token (default 'SIGIL'), nonce (default now ms), broadcast (default false).",
        input_schema: json!({"type":"object","properties":{
            "from":{"type":"string"},"to":{"type":"string"},"amount":{"type":"integer"},
            "secret":{"type":"string"},"token":{"type":"string","default":"SIGIL"},
            "nonce":{"type":"integer"},"broadcast":{"type":"boolean","default":false}
        },"required":["from","to","amount","secret"]}),
    }, wallet_send);

    registry.register(ToolDef {
        name: "flux_sigil_wallet_openapi",
        description: "Emit the OpenAPI 3.1 spec of the SIGIL wallet surface — the 10 sigil-rpcd endpoints these tools drive, generated the flux way from flux-api ApiEndpoint definitions. Feed to any SDK generator or docs pipeline.",
        input_schema: json!({"type":"object","properties":{}}),
    }, |_a| flux_api::generate_openapi("SIGIL Wallet API (sigil-rpcd)", "1.0.0", &sigil_wallet_spec()).to_string());

    registry.register(ToolDef {
        name: "flux_sigil_wallet_sdk",
        description: "Generate a typed sigil-rpcd client SDK from the same flux-api spec as flux_sigil_wallet_openapi. Args: language — typescript (default) | python | rust | go | kotlin. Base URL = FLUX_SIGIL_RPC.",
        input_schema: json!({"type":"object","properties":{"language":{"type":"string","enum":["typescript","python","rust","go","kotlin"],"default":"typescript"}}}),
    }, wallet_sdk);

    registry.register(ToolDef {
        name: "flux_sigil_wallet_setup_script",
        description: "Emit a ready-to-paste MCP client config for this SIGIL wallet server (mirrors Quillon's generate_mcp_setup_script).",
        input_schema: json!({"type":"object","properties":{}}),
    }, wallet_setup_script);
}

fn wallet_create(_a: &Value) -> String {
    use ed25519_dalek::SigningKey;
    use rand::RngCore;
    let mut sk_bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut sk_bytes);
    let sk = SigningKey::from_bytes(&sk_bytes);
    let pk_hex = hex::encode(sk.verifying_key().as_bytes());
    json!({
        "ok": true, "wallet_id": pk_hex, "pubkey": pk_hex, "secret": hex::encode(sk_bytes),
        "warning": "SECRET grants full control — store it securely; the MCP does not keep it.",
        "next": "fund via flux_sigil_wallet_faucet, then flux_sigil_wallet_balance"
    }).to_string()
}

fn wallet_import(a: &Value) -> String {
    use ed25519_dalek::SigningKey;
    let secret = arg_str(a, "secret", "");
    let sk_bytes = match hex::decode(&secret).ok().and_then(|b| <[u8; 32]>::try_from(b).ok()) {
        Some(b) => b,
        None => return json!({"ok": false, "error": "secret must be 32-byte hex"}).to_string(),
    };
    let sk = SigningKey::from_bytes(&sk_bytes);
    let wid = hex::encode(sk.verifying_key().as_bytes());
    json!({"ok": true, "wallet_id": wid, "pubkey": wid, "note": "derived from secret; ready for balance/send"}).to_string()
}

fn wallet_balance(a: &Value) -> String {
    let wallet = arg_str(a, "wallet", "");
    let token = arg_str(a, "token", "SIGIL");
    if wallet.is_empty() { return json!({"ok": false, "error": "wallet required"}).to_string(); }
    get(&format!("/api/v1/balance?wallet={wallet}&token={token}"))
}

fn wallet_network_status(_a: &Value) -> String {
    let sj: Value = serde_json::from_str(&get("/api/v1/status")).unwrap_or(json!(null));
    let tj: Value = serde_json::from_str(&get("/api/v1/tip")).unwrap_or(json!(null));
    json!({"ok": true, "status": sj, "tip": tj}).to_string()
}

fn wallet_mining_status(a: &Value) -> String {
    let wallet = arg_str(a, "wallet", "");
    if wallet.is_empty() { return json!({"ok": false, "error": "wallet required"}).to_string(); }
    get(&format!("/api/v1/mining/challenge?wallet={wallet}"))
}

fn wallet_faucet(a: &Value) -> String {
    let seed = arg_str(a, "seed", "");
    let body = if seed.is_empty() { json!({}) } else { json!({"seed": seed}) };
    match post("/api/v1/onboard", &body.to_string()) {
        Ok(b) => json!({"ok": true, "onboard": serde_json::from_str::<Value>(&b).unwrap_or(json!({"raw": b}))}).to_string(),
        Err(e) => json!({"ok": false, "error": e}).to_string(),
    }
}

fn wallet_send(a: &Value) -> String {
    use ed25519_dalek::{Signer, SigningKey};
    let from = arg_str(a, "from", "");
    let to = arg_str(a, "to", "");
    let amount = arg_u64(a, "amount", 0);
    let token = arg_str(a, "token", "SIGIL");
    let secret = arg_str(a, "secret", "");
    let nonce = arg_u64(a, "nonce", now_ms());
    let broadcast = a.get("broadcast").and_then(|v| v.as_bool()).unwrap_or(false);
    if from.is_empty() || to.is_empty() || amount == 0 || secret.is_empty() {
        return json!({"ok": false, "error": "from, to, amount, secret required"}).to_string();
    }
    let sk_bytes = match hex::decode(&secret).ok().and_then(|b| <[u8; 32]>::try_from(b).ok()) {
        Some(b) => b,
        None => return json!({"ok": false, "error": "secret must be 32-byte hex"}).to_string(),
    };
    let sk = SigningKey::from_bytes(&sk_bytes);
    if hex::encode(sk.verifying_key().as_bytes()) != from.to_lowercase() {
        return json!({"ok": false, "error": "secret does not match `from` wallet (rpcd wallet_id == pubkey)"}).to_string();
    }
    let msg = format!("{AUTH_DOMAIN}|send|{from}|{to}|{token}|{amount}|nonce={nonce}");
    let sig = hex::encode(sk.sign(msg.as_bytes()).to_bytes());
    let request = json!({"from": from, "to": to, "amount": amount, "token": token, "sig": sig, "req_nonce": nonce});
    if !broadcast {
        return json!({
            "ok": true, "proposed": true,
            "note": "SIGNED but NOT sent. Re-call with \"broadcast\": true to submit.",
            "endpoint": format!("POST {}/api/v1/send", rpc_base()), "request": request
        }).to_string();
    }
    match post("/api/v1/send", &request.to_string()) {
        Ok(b) => json!({"ok": true, "broadcast": true, "rpcd_response": serde_json::from_str::<Value>(&b).unwrap_or(json!({"raw": b}))}).to_string(),
        Err(e) => json!({"ok": false, "broadcast": true, "error": e}).to_string(),
    }
}

fn wallet_setup_script(_a: &Value) -> String {
    json!({
        "ok": true,
        "mcp_client_config": {"mcpServers": {"sigil-wallet": {
            "command": "fluxc", "args": ["mcp"],
            "env": {"FLUX_SIGIL_RPC": rpc_base()}
        }}},
        "note": "off-box: set FLUX_SIGIL_RPC=https://quillon.xyz:8843 ; on-box: http://127.0.0.1:8099",
        "tools": [
            "flux_sigil_wallet_create","flux_sigil_wallet_import","flux_sigil_wallet_balance",
            "flux_sigil_wallet_network_status","flux_sigil_wallet_economy","flux_sigil_wallet_pools",
            "flux_sigil_wallet_tokens","flux_sigil_wallet_mining_status","flux_sigil_wallet_recent",
            "flux_sigil_wallet_faucet","flux_sigil_wallet_send",
            "flux_sigil_wallet_openapi","flux_sigil_wallet_sdk"
        ]
    }).to_string()
}

// ===== flux-api surface — "the spec is the API" =============================

fn ep(
    method: HttpMethod,
    path: &str,
    op: &str,
    summary: &str,
    parameters: Vec<ApiParameter>,
    request_body: Option<ApiSchema>,
    ok: ApiSchema,
) -> ApiEndpoint {
    ApiEndpoint {
        crate_name: "sigil-rpcd".to_string(),
        method,
        path: path.to_string(),
        operation_id: op.to_string(),
        summary: summary.to_string(),
        parameters,
        request_body,
        responses: vec![
            ApiResponse { status: 200, description: "OK".to_string(), schema: Some(ok) },
            ApiResponse {
                status: 400,
                description: "rpcd error (JSON body, still useful data)".to_string(),
                schema: Some(
                    ApiSchema::object()
                        .req_prop("ok", ApiSchema::boolean())
                        .req_prop("error", ApiSchema::string())
                        .build(),
                ),
            },
        ],
        tags: vec!["sigil-wallet".to_string()],
        middleware: None,
    }
}

fn qp(name: &str, required: bool, schema: ApiSchema, desc: &str) -> ApiParameter {
    ApiParameter {
        name: name.to_string(),
        location: ParamLocation::Query,
        required,
        schema,
        description: desc.to_string(),
    }
}

/// 32-byte hex (64 chars) — wallet ids, token ids, block hashes.
fn hex32() -> ApiSchema {
    ApiSchema::string_with_format("hex32")
}

/// The sigil-rpcd endpoints the `flux_sigil_wallet_*` tools drive, as flux-api
/// endpoint definitions. Response shapes mirror the rpcd handlers 1:1.
pub fn sigil_wallet_spec() -> Vec<ApiEndpoint> {
    vec![
        ep(
            HttpMethod::GET, "/api/v1/status", "network_status",
            "Service liveness: network id, height, peer count, index readiness.",
            vec![], None,
            ApiSchema::object()
                .req_prop("ok", ApiSchema::boolean())
                .req_prop("service", ApiSchema::string())
                .req_prop("network", ApiSchema::string())
                .req_prop("height", ApiSchema::integer())
                .prop("version", ApiSchema::string())
                .prop("peers", ApiSchema::integer())
                .prop("index_ready", ApiSchema::boolean())
                .build(),
        ),
        ep(
            HttpMethod::GET, "/api/v1/tip", "tip",
            "Chain tip: height, tip hash, difficulty bits, genesis timestamp.",
            vec![], None,
            ApiSchema::object()
                .req_prop("ok", ApiSchema::boolean())
                .req_prop("height", ApiSchema::integer())
                .req_prop("tip", hex32())
                .prop("block_height", ApiSchema::integer())
                .prop("bits", ApiSchema::integer())
                .prop("genesis_ts_us", ApiSchema::string())
                .build(),
        ),
        ep(
            HttpMethod::GET, "/api/v1/balance", "balance",
            "Live balance of a wallet for a token.",
            vec![
                qp("wallet", true, hex32(), "wallet id (= ed25519 pubkey, 64-hex)"),
                qp("token", false, ApiSchema::string(), "token symbol or 64-hex id (default SIGIL)"),
            ],
            None,
            ApiSchema::object()
                .req_prop("ok", ApiSchema::boolean())
                .req_prop("balance", ApiSchema::integer())
                .build(),
        ),
        ep(
            HttpMethod::GET, "/api/v1/economy", "economy",
            "Economy overview: token/pool/trader/citizen counts + treasury.",
            vec![], None,
            ApiSchema::object()
                .req_prop("ok", ApiSchema::boolean())
                .req_prop("network", ApiSchema::string())
                .req_prop("height", ApiSchema::integer())
                .prop("tokens", ApiSchema::integer())
                .prop("pools", ApiSchema::integer())
                .prop("traders", ApiSchema::integer())
                .prop("students", ApiSchema::integer())
                .prop("citizens", ApiSchema::integer())
                .prop("treasury_usds", ApiSchema::integer())
                .build(),
        ),
        ep(
            HttpMethod::GET, "/api/v1/tokens", "tokens",
            "Token registry (symbol → 64-hex token id).",
            vec![], None,
            ApiSchema::object()
                .req_prop("ok", ApiSchema::boolean())
                .req_prop("tokens", ApiSchema::array_of(
                    ApiSchema::object()
                        .req_prop("symbol", ApiSchema::string())
                        .req_prop("id", hex32())
                        .build(),
                ))
                .build(),
        ),
        ep(
            HttpMethod::GET, "/api/v1/pools", "pools",
            "DEX pools: pair labels, reserves, LP shares, fees.",
            vec![], None,
            ApiSchema::object()
                .req_prop("ok", ApiSchema::boolean())
                .req_prop("pools", ApiSchema::array_of(
                    ApiSchema::object()
                        .req_prop("id", hex32())
                        .req_prop("label", ApiSchema::string())
                        .prop("token_a", hex32())
                        .prop("token_b", hex32())
                        .prop("sym_a", ApiSchema::string())
                        .prop("sym_b", ApiSchema::string())
                        .prop("reserve_a", ApiSchema::integer())
                        .prop("reserve_b", ApiSchema::integer())
                        .prop("lp_shares", ApiSchema::integer())
                        .prop("fee_bps", ApiSchema::integer())
                        .prop("accrued_fees", ApiSchema::integer())
                        .build(),
                ))
                .build(),
        ),
        ep(
            HttpMethod::GET, "/api/v1/mining/challenge", "mining_challenge",
            "Mining challenge for a wallet: VDF input bytes, blake4 target, VDF steps. NOTE: no `ok` field — raw challenge object.",
            vec![qp("wallet", true, hex32(), "miner wallet id (64-hex)")],
            None,
            ApiSchema::object()
                .req_prop("height", ApiSchema::integer())
                .req_prop("vdf_input", ApiSchema::array_of(ApiSchema::integer()))
                .req_prop("blake4_target", ApiSchema::integer())
                .req_prop("vdf_t", ApiSchema::integer())
                .build(),
        ),
        ep(
            HttpMethod::GET, "/api/v1/recent", "recent",
            "Recent on-chain activity feed, optionally filtered by kind (mine/send/swap/…).",
            vec![qp("kind", false, ApiSchema::string(), "filter: mine | send | swap | …")],
            None,
            ApiSchema::object()
                .req_prop("ok", ApiSchema::boolean())
                .req_prop("count", ApiSchema::integer())
                .prop("kind", ApiSchema::string())
                .req_prop("results", ApiSchema::array_of(
                    ApiSchema::object()
                        .req_prop("title", ApiSchema::string())
                        .req_prop("kind", ApiSchema::string())
                        .req_prop("ts", ApiSchema::integer())
                        .prop("content", ApiSchema::string())
                        .prop("h", ApiSchema::integer())
                        .prop("hash", ApiSchema::string())
                        .prop("prod", ApiSchema::string())
                        .prop("cid", ApiSchema::string())
                        .build(),
                ))
                .build(),
        ),
        ep(
            HttpMethod::POST, "/api/v1/onboard", "onboard",
            "Faucet onboarding: derive/accept a seed, register student + citizen, grant starter NATIVE+USDS from the finite OPERATOR faucet. Per-IP rate-limited.",
            vec![],
            Some(
                ApiSchema::object()
                    .prop("seed", hex32())
                    .build(),
            ),
            ApiSchema::object()
                .req_prop("ok", ApiSchema::boolean())
                .req_prop("wallet", hex32())
                .req_prop("seed", hex32())
                .prop("student", ApiSchema::boolean())
                .prop("citizen", ApiSchema::boolean())
                .prop("native", ApiSchema::integer())
                .prop("usds", ApiSchema::integer())
                .build(),
        ),
        ep(
            HttpMethod::POST, "/api/v1/send", "send",
            "Transfer tokens. Requires ed25519 auth: sig over the sigil-rpc/v1 send domain (see flux_sigil_wallet_send, which signs this for you) + req_nonce.",
            vec![],
            Some(
                ApiSchema::object()
                    .req_prop("from", hex32())
                    .req_prop("to", hex32())
                    .req_prop("amount", ApiSchema::integer())
                    .req_prop("sig", ApiSchema::string_with_format("hex-ed25519-sig"))
                    .req_prop("req_nonce", ApiSchema::integer())
                    .prop("token", ApiSchema::string())
                    .build(),
            ),
            ApiSchema::object()
                .req_prop("ok", ApiSchema::boolean())
                .req_prop("from", hex32())
                .req_prop("to", hex32())
                .req_prop("amount", ApiSchema::integer())
                .req_prop("token", hex32())
                .req_prop("height", ApiSchema::integer())
                .build(),
        ),
    ]
}

fn wallet_sdk(a: &Value) -> String {
    let lang = arg_str(a, "language", "typescript");
    let spec = sigil_wallet_spec();
    let base = rpc_base();
    match lang.as_str() {
        "typescript" | "ts" => flux_api::generate_typescript_sdk(&spec, &base),
        "python" | "py" => flux_api::generate_python_sdk(&spec, &base),
        "rust" | "rs" => flux_api::generate_rust_client_sdk(&spec, &base),
        "go" => flux_api::generate_go_sdk(&spec, &base, "sigilwallet"),
        "kotlin" | "kt" => flux_api::generate_kotlin_sdk(&spec, &base, "xyz.fluxapp.sigilwallet"),
        other => json!({
            "ok": false, "error": format!("unknown language '{other}'"),
            "supported": ["typescript", "python", "rust", "go", "kotlin"]
        })
        .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_wallet_is_valid_ed25519() {
        let v: Value = serde_json::from_str(&wallet_create(&json!({}))).unwrap();
        assert_eq!(v["ok"], true);
        let (wid, sec) = (v["wallet_id"].as_str().unwrap(), v["secret"].as_str().unwrap());
        assert_eq!(hex::decode(wid).unwrap().len(), 32);
        use ed25519_dalek::SigningKey;
        let sk = SigningKey::from_bytes(&<[u8; 32]>::try_from(hex::decode(sec).unwrap()).unwrap());
        assert_eq!(hex::encode(sk.verifying_key().as_bytes()), wid);
    }

    #[test]
    fn import_matches_create() {
        let c: Value = serde_json::from_str(&wallet_create(&json!({}))).unwrap();
        let i: Value = serde_json::from_str(&wallet_import(&json!({"secret": c["secret"]}))).unwrap();
        assert_eq!(c["wallet_id"], i["wallet_id"]);
    }

    #[test]
    fn send_defaults_to_proposal_and_signs_deterministically() {
        use ed25519_dalek::SigningKey;
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let from = hex::encode(sk.verifying_key().as_bytes());
        let v: Value = serde_json::from_str(&wallet_send(&json!({"from": from, "to": "ab".repeat(32), "amount": 100, "secret": hex::encode([7u8;32]), "nonce": 42}))).unwrap();
        assert_eq!(v["proposed"], true);
        assert_eq!(v["request"]["sig"].as_str().unwrap().len(), 128);
        assert_eq!(v["request"]["req_nonce"], 42);
    }

    #[test]
    fn send_rejects_secret_not_owning_from() {
        let v: Value = serde_json::from_str(&wallet_send(&json!({"from": "00".repeat(32), "to": "ab".repeat(32), "amount": 1, "secret": hex::encode([7u8;32])}))).unwrap();
        assert_eq!(v["ok"], false);
    }

    #[test]
    fn spec_models_the_full_rpcd_surface() {
        let spec = sigil_wallet_spec();
        assert_eq!(spec.len(), 10);
        assert!(spec.iter().all(|e| e.path.starts_with("/api/v1/")));
        let ops: std::collections::BTreeSet<&str> =
            spec.iter().map(|e| e.operation_id.as_str()).collect();
        assert_eq!(ops.len(), spec.len(), "operation_ids must be unique");
        for e in &spec {
            if e.method == HttpMethod::POST {
                assert!(e.request_body.is_some(), "{} must carry a request body", e.path);
            } else {
                assert!(e.request_body.is_none(), "{} is a GET — no body", e.path);
            }
        }
    }

    #[test]
    fn openapi_covers_send_and_balance() {
        let doc = flux_api::generate_openapi("SIGIL Wallet API", "1.0.0", &sigil_wallet_spec());
        assert_eq!(doc["openapi"], "3.1.0");
        assert!(doc["paths"]["/api/v1/send"].get("post").is_some(), "POST /send missing: {doc}");
        assert!(doc["paths"]["/api/v1/balance"].get("get").is_some(), "GET /balance missing");
        assert!(doc["paths"]["/api/v1/onboard"].get("post").is_some(), "POST /onboard missing");
    }

    #[test]
    fn sdk_generates_for_every_language() {
        for lang in ["typescript", "python", "rust", "go", "kotlin"] {
            let out = wallet_sdk(&json!({"language": lang}));
            assert!(out.len() > 200, "{lang} SDK suspiciously small ({} bytes)", out.len());
            assert!(!out.contains("unknown language"), "{lang} rejected: {out}");
        }
        let bad = wallet_sdk(&json!({"language": "cobol"}));
        assert!(bad.contains("unknown language"));
    }
}
