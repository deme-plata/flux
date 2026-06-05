//! flux-search v2 — secret scraping + redaction.
//!
//! Captures interesting patterns from MCP tool calls (the "secret commands"
//! Viktor named) WITHOUT leaking sensitive material into the index.
//!
//! Redaction policy (binding):
//!   - keys named: api_key, password, secret_key, mnemonic, seed_phrase,
//!     private_key, signing_key, auth_token, bearer → "[REDACTED]"
//!   - qnk… wallet addresses → first 8 + last 8 chars
//!   - any string > 512 bytes → BLAKE3 hash + length only
//!   - any 64+ byte hex/base64 blob → length only

use serde_json::Value;

const SENSITIVE_KEYS: &[&str] = &[
    "api_key",
    "apikey",
    "password",
    "passwd",
    "secret_key",
    "secretkey",
    "mnemonic",
    "seed_phrase",
    "seedphrase",
    "private_key",
    "privatekey",
    "signing_key",
    "auth_token",
    "authtoken",
    "bearer",
    "session_token",
];

const LONG_STRING_THRESHOLD: usize = 512;

/// Apply the redaction policy to a JSON arg object.
pub fn redact_args(args: &Value) -> Value {
    redact_value(args)
}

fn redact_value(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, val) in map.iter() {
                if is_sensitive_key(k) {
                    out.insert(k.clone(), Value::String("[REDACTED]".into()));
                } else {
                    out.insert(k.clone(), redact_value(val));
                }
            }
            Value::Object(out)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(redact_value).collect()),
        Value::String(s) => Value::String(redact_string(s)),
        other => other.clone(),
    }
}

fn is_sensitive_key(k: &str) -> bool {
    let lower = k.to_lowercase();
    SENSITIVE_KEYS.iter().any(|s| lower.contains(s))
}

/// Truncate a string per the redaction policy:
///   - wallet addresses → first 8 + last 8
///   - hex/base64 ≥ 64 bytes → length only
///   - any string > 512 → length + BLAKE3 hash
pub fn redact_string(s: &str) -> String {
    if looks_like_wallet(s) {
        return truncate_wallet(s);
    }
    if s.len() > LONG_STRING_THRESHOLD {
        let h = blake3::hash(s.as_bytes());
        let short_hash: String = h.to_hex().as_str().chars().take(12).collect();
        return format!("[{} chars · blake3:{}]", s.len(), short_hash);
    }
    if s.len() >= 64 && (looks_like_hex(s) || looks_like_base64(s)) {
        return format!("[{} chars · opaque-blob]", s.len());
    }
    s.to_string()
}

fn looks_like_wallet(s: &str) -> bool {
    s.starts_with("qnk") && s.len() == 67 && s.chars().skip(3).all(|c| c.is_ascii_hexdigit())
}

fn truncate_wallet(s: &str) -> String {
    if s.len() < 24 {
        return s.to_string();
    }
    format!("{}…{}", &s[..8], &s[s.len() - 8..])
}

fn looks_like_hex(s: &str) -> bool {
    s.chars().all(|c| c.is_ascii_hexdigit())
}

fn looks_like_base64(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
}

/// Surface "interesting" command sequences as named patterns.
/// (Viktor's "secret commands" while working.)
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SecretPattern {
    ReleaseCut,           // cut-release.sh execution chain
    NewCrateScaffold,     // flux_combo against a never-before-seen package
    ClaimSettlementLoop,  // flux_swarm_claim → flux_swarm_complete in same agent_id
    ProofEmission,        // fluxc compile-native --provenance
    DeployArtifact,       // flux_ui_deploy or sigil-updater publish
    Other(String),
}

/// Classify a single tool name into a pattern hint (best-effort).
pub fn classify_pattern(tool: &str) -> SecretPattern {
    let t = tool.to_lowercase();
    if t.contains("cut-release") {
        SecretPattern::ReleaseCut
    } else if t == "flux_swarm_claim" || t == "flux_swarm_complete" {
        SecretPattern::ClaimSettlementLoop
    } else if t.contains("compile-native") || t.contains("provenance") || t.contains("sign") {
        SecretPattern::ProofEmission
    } else if t.contains("ui_deploy") || t.contains("sigil-updater") || t.contains("release_publish")
    {
        SecretPattern::DeployArtifact
    } else if t.contains("combo") {
        SecretPattern::NewCrateScaffold
    } else {
        SecretPattern::Other(tool.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sensitive_keys_redacted() {
        let v = json!({
            "api_key": "sk-deadbeef",
            "password": "hunter2",
            "package": "flux-search",
            "Mnemonic": "twelve words here you know the drill"
        });
        let out = redact_args(&v);
        assert_eq!(out["api_key"], "[REDACTED]");
        assert_eq!(out["password"], "[REDACTED]");
        assert_eq!(out["package"], "flux-search");
        assert_eq!(out["Mnemonic"], "[REDACTED]");
    }

    #[test]
    fn wallet_truncated() {
        let w = "qnk7154929a6aa0c118791373ea21004aca6e494e6e031c36f780cd5acedf031ccb";
        assert_eq!(w.len(), 67);
        let out = redact_string(w);
        assert!(out.starts_with("qnk71549"));
        assert!(out.ends_with("31ccb") || out.ends_with("1ccb"));
        assert!(out.contains("…"));
    }

    #[test]
    fn long_strings_get_hashed() {
        let big = "x".repeat(600);
        let out = redact_string(&big);
        assert!(out.contains("600 chars"));
        assert!(out.contains("blake3"));
    }

    #[test]
    fn long_hex_blob_redacted() {
        let blob = "ab".repeat(40); // 80 hex chars = 40 bytes
        let out = redact_string(&blob);
        assert!(out.contains("80 chars"));
        assert!(out.contains("opaque-blob"));
    }

    #[test]
    fn classify_common_tools() {
        assert_eq!(classify_pattern("flux_swarm_claim"), SecretPattern::ClaimSettlementLoop);
        assert_eq!(classify_pattern("flux_combo"), SecretPattern::NewCrateScaffold);
        assert_eq!(classify_pattern("flux_ui_deploy"), SecretPattern::DeployArtifact);
        assert_eq!(classify_pattern("fluxc compile-native"), SecretPattern::ProofEmission);
    }

    #[test]
    fn arrays_recurse() {
        let v = json!({
            "items": [{"api_key": "sk-x"}, {"package": "y"}]
        });
        let out = redact_args(&v);
        assert_eq!(out["items"][0]["api_key"], "[REDACTED]");
        assert_eq!(out["items"][1]["package"], "y");
    }
}
