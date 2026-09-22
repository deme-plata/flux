//! Crypto-agile, purpose-bound provenance.
//!
//! Cryptographic validity and order authorization are separate signals.
//! Ephemeral keys may attest drafts/quotes, but only a persistent pinned key
//! may sign an `order_authorization` intent.

use std::collections::HashMap;
use std::sync::OnceLock;

use flux_sqisign::hybrid::{
    deserialize_hybrid, hybrid_keygen, hybrid_sign, hybrid_verify, serialize_hybrid,
    HybridSignature, SchemeId,
};
use serde_json::{json, Value};

use crate::{hex_decode, hex_encode};

const SCHEMA: &str = "slagteren.order-provenance/v2";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    SqisignL5,
    HybridSqisignEd25519,
}

impl Profile {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "sqisign" | "sqisign-l5" | "sqisign-l5-v1" | "legacy" => {
                Ok(Self::SqisignL5)
            }
            "" | "hybrid" | "hybrid-v1" | "hybrid-sqisign-ed25519"
            | "hybrid-sqisign-ed25519-v1" => Ok(Self::HybridSqisignEd25519),
            other => Err(format!("unsupported provenance profile '{other}'")),
        }
    }

    fn configured() -> Self {
        let value = std::env::var("FLUX_MCP_PROVENANCE_PROFILE")
            .or_else(|_| std::env::var("ORDER_PROVENANCE_PROFILE"))
            .unwrap_or_else(|_| "hybrid-sqisign-ed25519-v1".into());
        Self::parse(&value).unwrap_or_else(|e| {
            eprintln!("[slagteren-mcp] WARNING: {e}; using hybrid profile");
            Self::HybridSqisignEd25519
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::SqisignL5 => "sqisign-l5-v1",
            Self::HybridSqisignEd25519 => "hybrid-sqisign-ed25519-v1",
        }
    }

    fn schemes(self) -> &'static [SchemeId] {
        const SQI: &[SchemeId] = &[SchemeId::SQIsign];
        const HYBRID: &[SchemeId] = &[SchemeId::SQIsign, SchemeId::Ed25519];
        match self {
            Self::SqisignL5 => SQI,
            Self::HybridSqisignEd25519 => HYBRID,
        }
    }
}

fn scheme_name(scheme: SchemeId) -> &'static str {
    match scheme {
        SchemeId::SQIsign => "sqisign-l5",
        SchemeId::Ed25519 => "ed25519",
        SchemeId::Dilithium5 => "dilithium5",
    }
}

struct KeyRing {
    profile: Profile,
    keys: HashMap<SchemeId, (Vec<u8>, Vec<u8>)>,
    persistent: bool,
    key_id: String,
}

impl KeyRing {
    fn checked(
        profile: Profile,
        keys: HashMap<SchemeId, (Vec<u8>, Vec<u8>)>,
        persistent: bool,
    ) -> Result<Self, String> {
        let probe = b"slagteren-mcp/keyring-self-test/v2";
        let bundle = hybrid_sign(probe, &keys, profile.schemes())?;
        let checked = hybrid_verify(probe, &bundle, profile.schemes());
        if !checked.all_valid {
            return Err(format!("provenance keyring self-test failed: {}", checked.reason));
        }
        let key_id = key_id_from_keys(profile, &keys)?;
        Ok(Self { profile, keys, persistent, key_id })
    }

    fn generated(profile: Profile) -> Self {
        let keys = hybrid_keygen(profile.schemes())
            .expect("supported provenance profile key generation");
        Self::checked(profile, keys, false)
            .expect("generated provenance keyring must self-verify")
    }

    fn from_env(profile: Profile) -> Result<Option<Self>, String> {
        let sqi_sk = env_value("FLUX_MCP_SQISIGN_SK_HEX", "SQISIGN_SK_HEX");
        let sqi_pk = env_value("FLUX_MCP_SQISIGN_PK_HEX", "SQISIGN_PK_HEX");
        let ed_sk = env_value("FLUX_MCP_ED25519_SK_HEX", "ED25519_SK_HEX");
        let ed_pk = env_value("FLUX_MCP_ED25519_PK_HEX", "ED25519_PK_HEX");
        if sqi_sk.is_none() && sqi_pk.is_none() && ed_sk.is_none() && ed_pk.is_none() {
            return Ok(None);
        }

        let sqi_sk = decode_required("SQIsign secret key", sqi_sk)?;
        let sqi_pk = decode_required("SQIsign public key", sqi_pk)?;
        if sqi_pk.len() != flux_sqisign::public_key_size() {
            return Err(format!(
                "SQIsign public key must be {} bytes, got {}",
                flux_sqisign::public_key_size(),
                sqi_pk.len()
            ));
        }
        let mut keys = HashMap::new();
        keys.insert(SchemeId::SQIsign, (sqi_sk, sqi_pk));

        if profile == Profile::HybridSqisignEd25519 {
            let ed_sk = decode_required("Ed25519 secret key", ed_sk)?;
            let ed_pk = decode_required("Ed25519 public key", ed_pk)?;
            if ed_sk.len() != 32 || ed_pk.len() != 32 {
                return Err(format!(
                    "Ed25519 keys must each be 32 bytes, got sk={} pk={}",
                    ed_sk.len(),
                    ed_pk.len()
                ));
            }
            keys.insert(SchemeId::Ed25519, (ed_sk, ed_pk));
        }
        Self::checked(profile, keys, true).map(Some)
    }
}

fn env_value(primary: &str, legacy: &str) -> Option<String> {
    std::env::var(primary)
        .ok()
        .or_else(|| std::env::var(legacy).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn decode_required(label: &str, value: Option<String>) -> Result<Vec<u8>, String> {
    let value = value.ok_or_else(|| format!("{label} is missing"))?;
    hex_decode(&value).map_err(|e| format!("{label}: {e}"))
}

fn configured_trusted_key_id() -> Option<String> {
    std::env::var("FLUX_MCP_TRUSTED_KEY_ID")
        .ok()
        .or_else(|| std::env::var("ORDER_PROVENANCE_TRUSTED_KEY_ID").ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

static KEYRING: OnceLock<KeyRing> = OnceLock::new();

fn current() -> &'static KeyRing {
    KEYRING.get_or_init(|| {
        let profile = Profile::configured();
        match KeyRing::from_env(profile) {
            Ok(Some(ring)) => ring,
            Ok(None) => {
                eprintln!(
                    "[slagteren-mcp] INFO: ephemeral test provenance; order authorization disabled"
                );
                KeyRing::generated(profile)
            }
            Err(e) => {
                eprintln!(
                    "[slagteren-mcp] WARNING: invalid persistent provenance ({e}); \
                     using ephemeral non-authorizing keys"
                );
                KeyRing::generated(profile)
            }
        }
    })
}

fn key_id_from_keys(
    profile: Profile,
    keys: &HashMap<SchemeId, (Vec<u8>, Vec<u8>)>,
) -> Result<String, String> {
    let pairs = profile
        .schemes()
        .iter()
        .map(|scheme| {
            keys.get(scheme)
                .map(|(_, pk)| (*scheme, pk.as_slice()))
                .ok_or_else(|| format!("missing {} public key", scheme_name(*scheme)))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(key_id_from_pairs(profile, &pairs))
}

fn key_id_from_bundle(profile: Profile, bundle: &HybridSignature) -> String {
    let pairs = bundle
        .signatures
        .iter()
        .map(|leg| (leg.scheme, leg.public_key.as_slice()))
        .collect::<Vec<_>>();
    key_id_from_pairs(profile, &pairs)
}

fn key_id_from_pairs(profile: Profile, pairs: &[(SchemeId, &[u8])]) -> String {
    let mut sorted = pairs.to_vec();
    sorted.sort_by_key(|(scheme, _)| *scheme as u8);
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"slagteren-mcp/provenance-key-id/v2");
    hasher.update(profile.name().as_bytes());
    for (scheme, pk) in sorted {
        hasher.update(&[scheme as u8]);
        hasher.update(&(pk.len() as u16).to_le_bytes());
        hasher.update(pk);
    }
    let digest = hasher.finalize().to_hex().to_string();
    format!("{}:{}", profile.name(), &digest[..32])
}

pub fn can_authorize_orders() -> bool {
    let ring = current();
    ring.persistent
        && configured_trusted_key_id().as_deref() == Some(ring.key_id.as_str())
}

pub fn sqisign_pubkey_hex() -> Option<String> {
    current()
        .keys
        .get(&SchemeId::SQIsign)
        .map(|(_, pk)| hex_encode(pk))
}

pub fn public_metadata() -> Value {
    let ring = current();
    let legs = ring
        .profile
        .schemes()
        .iter()
        .filter_map(|scheme| {
            ring.keys.get(scheme).map(|(_, pk)| {
                json!({
                    "scheme": scheme_name(*scheme),
                    "public_key_hex": hex_encode(pk),
                    "public_key_bytes": pk.len()
                })
            })
        })
        .collect::<Vec<_>>();
    json!({
        "schema": SCHEMA,
        "profile": ring.profile.name(),
        "required_schemes": ring.profile.schemes().iter().map(|s| scheme_name(*s)).collect::<Vec<_>>(),
        "key_id": ring.key_id,
        "key_pinned": configured_trusted_key_id().as_deref() == Some(ring.key_id.as_str()),
        "key_origin": if ring.persistent { "persistent" } else { "ephemeral-test" },
        "can_authorize_orders": can_authorize_orders(),
        "public_keys": legs
    })
}

pub fn sign_record(intent: &str, authorizes_order: bool, body: Value) -> Result<Value, String> {
    let ring = current();
    if authorizes_order && !can_authorize_orders() {
        return Err(
            "order authorization requires a persistent keyring matching FLUX_MCP_TRUSTED_KEY_ID"
                .into(),
        );
    }
    let signed = json!({
        "schema": SCHEMA,
        "profile": ring.profile.name(),
        "key_id": ring.key_id,
        "intent": intent,
        "authorizes_order": authorizes_order,
        "body": body
    });
    let signed_payload = signed.to_string();
    let bundle = hybrid_sign(
        signed_payload.as_bytes(),
        &ring.keys,
        ring.profile.schemes(),
    )?;
    Ok(json!({
        "version": 2,
        "profile": ring.profile.name(),
        "required_schemes": ring.profile.schemes().iter().map(|s| scheme_name(*s)).collect::<Vec<_>>(),
        "key_id": ring.key_id,
        "key_origin": if ring.persistent { "persistent" } else { "ephemeral-test" },
        "bundle_hex": hex_encode(&serialize_hybrid(&bundle)),
        "signed_payload": signed_payload,
        "note": if authorizes_order {
            "Crypto-agile order authorization; verify against the shop-pinned key id."
        } else {
            "Integrity attestation only; this does not authorize an order or payment."
        }
    }))
}

pub fn verify_envelope(
    signed_payload: &str,
    bundle_hex: &str,
    profile_name: &str,
    expected_key_id: Option<&str>,
) -> Value {
    let profile = match Profile::parse(profile_name) {
        Ok(p) => p,
        Err(e) => return json!({"valid": false, "error": e, "authorizes_order": false}),
    };
    let bundle = match hex_decode(bundle_hex).and_then(|b| deserialize_hybrid(&b)) {
        Ok(b) => b,
        Err(e) => return json!({"valid": false, "error": e, "authorizes_order": false}),
    };
    if bundle.version != 1 {
        return json!({
            "valid": false,
            "error": format!("unsupported hybrid bundle version {}", bundle.version),
            "authorizes_order": false
        });
    }
    let checked = hybrid_verify(signed_payload.as_bytes(), &bundle, profile.schemes());
    let payload = serde_json::from_str::<Value>(signed_payload).unwrap_or(Value::Null);
    let actual_key_id = key_id_from_bundle(profile, &bundle);
    let metadata_bound = payload.get("schema").and_then(Value::as_str) == Some(SCHEMA)
        && payload.get("profile").and_then(Value::as_str) == Some(profile.name())
        && payload.get("key_id").and_then(Value::as_str) == Some(actual_key_id.as_str());
    let matches_server_key = actual_key_id.as_str() == current().key_id.as_str();
    let matches_expected_key =
        expected_key_id.map(|expected| expected == actual_key_id.as_str());
    let matches_configured_pin =
        configured_trusted_key_id().as_deref() == Some(actual_key_id.as_str());
    // A caller-provided expected id is diagnostic only. Trust must come from the
    // server-side pin; otherwise a signer could declare its own key as trusted.
    let trusted_key = matches_configured_pin;
    let cryptographically_valid = checked.all_valid && metadata_bound;
    let intent = payload.get("intent").and_then(Value::as_str).unwrap_or("");
    let signed_authorization = payload
        .get("authorizes_order")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let authorizes_order = cryptographically_valid
        && trusted_key
        && signed_authorization
        && intent == "order_authorization";
    json!({
        "valid": cryptographically_valid,
        "cryptographically_valid": cryptographically_valid,
        "metadata_bound": metadata_bound,
        "profile": profile.name(),
        "key_id": actual_key_id,
        "matches_server_key": matches_server_key,
        "matches_expected_key": matches_expected_key,
        "matches_configured_pin": matches_configured_pin,
        "trusted_key": trusted_key,
        "intent": intent,
        "authorizes_order": authorizes_order,
        "passed_schemes": checked.passed_schemes.iter().map(|s| scheme_name(*s)).collect::<Vec<_>>(),
        "failed_schemes": checked.failed_schemes.iter().map(|s| scheme_name(*s)).collect::<Vec<_>>(),
        "reason": checked.reason,
        "note": if authorizes_order {
            "Valid order authorization from a trusted/pinned key."
        } else if cryptographically_valid {
            "Valid integrity attestation, but it does not authorize an order."
        } else {
            "Invalid provenance envelope."
        }
    })
}

pub fn verify_legacy(signed_payload: &str, signature_hex: &str, pubkey_hex: &str) -> Value {
    let decoded = hex_decode(signature_hex).and_then(|sig| {
        hex_decode(pubkey_hex).map(|pk| (sig, pk))
    });
    let (sig, pk) = match decoded {
        Ok(v) => v,
        Err(e) => return json!({"valid": false, "error": e, "authorizes_order": false}),
    };
    let valid = flux_sqisign::verify(signed_payload.as_bytes(), &sig, &pk).unwrap_or(false);
    json!({
        "valid": valid,
        "cryptographically_valid": valid,
        "profile": "legacy-sqisign-l5",
        "authorizes_order": false,
        "trusted_key": false,
        "note": if valid {
            "Legacy signature is valid, but its payload has no bound authorization intent."
        } else {
            "Invalid legacy SQIsign signature."
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_verifies_but_never_authorizes() {
        let envelope = sign_record(
            "quote_request",
            false,
            json!({"product_id": 361, "budget_ore": 10_000, "price_known": false}),
        )
        .unwrap();
        let checked = verify_envelope(
            envelope["signed_payload"].as_str().unwrap(),
            envelope["bundle_hex"].as_str().unwrap(),
            envelope["profile"].as_str().unwrap(),
            envelope["key_id"].as_str(),
        );
        assert_eq!(checked["cryptographically_valid"], true);
        assert_eq!(checked["authorizes_order"], false);
    }

    #[test]
    fn tampered_payload_fails() {
        let envelope = sign_record("quote_request", false, json!({"budget_ore": 10_000})).unwrap();
        let mut payload: Value =
            serde_json::from_str(envelope["signed_payload"].as_str().unwrap()).unwrap();
        payload["body"]["budget_ore"] = json!(1);
        let checked = verify_envelope(
            &payload.to_string(),
            envelope["bundle_hex"].as_str().unwrap(),
            envelope["profile"].as_str().unwrap(),
            envelope["key_id"].as_str(),
        );
        assert_eq!(checked["cryptographically_valid"], false);
        assert_eq!(checked["authorizes_order"], false);
    }

    #[test]
    fn caller_supplied_expected_key_cannot_create_trust() {
        let ring = current();
        let forged = json!({
            "schema": SCHEMA,
            "profile": ring.profile.name(),
            "key_id": ring.key_id,
            "intent": "order_authorization",
            "authorizes_order": true,
            "body": {"product_id": 361, "budget_ore": 10_000}
        });
        let signed_payload = forged.to_string();
        let bundle = hybrid_sign(
            signed_payload.as_bytes(),
            &ring.keys,
            ring.profile.schemes(),
        )
        .unwrap();
        let checked = verify_envelope(
            &signed_payload,
            &hex_encode(&serialize_hybrid(&bundle)),
            ring.profile.name(),
            Some(&ring.key_id),
        );
        assert_eq!(checked["cryptographically_valid"], true);
        assert_eq!(checked["matches_expected_key"], true);
        assert_eq!(checked["trusted_key"], false);
        assert_eq!(checked["authorizes_order"], false);
    }
}
