// fluxc-core/provenance.rs — Cryptographic artifact-provenance proofs.
//
// `flux_provenance` ties a compiled artifact (.o file) to the agent who emitted it,
// the source it was compiled from, the fluxc version used, and (optionally) the swarm
// settlement transaction on Quillon Graph. The proof is a 700-byte bundle signed with
// the agent's SQIsign Level 5 key. Anyone with the agent's public key can verify the
// binary is from a paid agent doing paid work — without trusting any centralized server.
//
// See FLUX_AI_CODING_V2_1M_CONTEXT.md (Section 3) for the design rationale.

use blake3::Hasher;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// The signed bundle. Compact (700 bytes) so it fits in 12 DNS TXT records or a QR code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceProof {
    pub version: u8,
    pub artifact_hash: [u8; 32],
    pub source_hash: [u8; 32],
    pub fluxc_version: u32,
    pub fluxc_git: [u8; 20],
    pub agent_wallet: [u8; 32],
    pub swarm_task_id: [u8; 16],
    pub settle_tx: [u8; 32],
    pub timestamp_us: u64,
    pub sqisign_pubkey: Vec<u8>,
    pub sqisign_sig: Vec<u8>,
    /// Classical hybrid leg (defense-in-depth). Empty on legacy single-SQIsign
    /// proofs; `#[serde(default)]` keeps those proofs deserializable. When both
    /// the SQIsign and Ed25519 legs are present, the proof is a require-BOTH
    /// hybrid and `verify` demands both validate.
    #[serde(default)]
    pub ed25519_pubkey: Vec<u8>,
    #[serde(default)]
    pub ed25519_sig: Vec<u8>,
}

/// Inputs the agent supplies when emitting a proof.
#[derive(Debug, Clone)]
pub struct ProvenanceContext {
    pub artifact_bytes: Vec<u8>,
    pub source_bytes: Vec<u8>,
    pub agent_wallet: [u8; 32],
    pub swarm_task_id: [u8; 16],
    pub settle_tx: Option<[u8; 32]>,
    pub fluxc_git: [u8; 20],
    pub fluxc_version: u32,
}

/// Result of a successful verification.
#[derive(Debug, Clone)]
pub struct VerifiedProof {
    pub agent_wallet: [u8; 32],
    pub source_hash: [u8; 32],
    pub artifact_hash: [u8; 32],
    pub timestamp_us: u64,
    pub settle_tx: Option<[u8; 32]>,
    pub on_chain_backed: bool,
}

#[derive(Debug)]
pub enum ProvenanceError {
    ArtifactHashMismatch { expected: String, got: String },
    InvalidSignature,
    SigningNotImplemented,
    Serde(String),
}

impl std::fmt::Display for ProvenanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProvenanceError::ArtifactHashMismatch { expected, got } =>
                write!(f, "artifact hash mismatch (expected {}, got {})", expected, got),
            ProvenanceError::InvalidSignature => write!(f, "SQIsign signature invalid"),
            ProvenanceError::SigningNotImplemented =>
                write!(f, "SQIsign signing not yet wired (agent key management TBD)"),
            ProvenanceError::Serde(e) => write!(f, "serialization: {}", e),
        }
    }
}

impl std::error::Error for ProvenanceError {}

/// BLAKE3 the artifact, BLAKE3 the source, build the bundle. Optional SQIsign signing
/// if both `agent_sk` and `agent_pk` are provided.
///
/// Returns the unsigned bundle (with empty sqisign fields) if no keys are supplied —
/// useful for testing the hash-bundling logic without needing an agent identity.
pub fn emit(ctx: &ProvenanceContext) -> Result<ProvenanceProof, ProvenanceError> {
    emit_with_keys(ctx, None, None)
}

pub fn emit_signed(
    ctx: &ProvenanceContext,
    agent_sk: &[u8],
    agent_pk: &[u8],
) -> Result<ProvenanceProof, ProvenanceError> {
    emit_with_keys(ctx, Some(agent_sk), Some(agent_pk))
}

fn emit_with_keys(
    ctx: &ProvenanceContext,
    agent_sk: Option<&[u8]>,
    agent_pk: Option<&[u8]>,
) -> Result<ProvenanceProof, ProvenanceError> {
    let artifact_hash = blake3_bytes(&ctx.artifact_bytes);
    let source_hash = blake3_bytes(&ctx.source_bytes);
    let timestamp_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0);
    let settle_tx = ctx.settle_tx.unwrap_or([0u8; 32]);

    let mut proof = ProvenanceProof {
        version: 1,
        artifact_hash,
        source_hash,
        fluxc_version: ctx.fluxc_version,
        fluxc_git: ctx.fluxc_git,
        agent_wallet: ctx.agent_wallet,
        swarm_task_id: ctx.swarm_task_id,
        settle_tx,
        timestamp_us,
        sqisign_pubkey: Vec::new(),
        sqisign_sig: Vec::new(),
        ed25519_pubkey: Vec::new(),
        ed25519_sig: Vec::new(),
    };

    if let (Some(sk), Some(pk)) = (agent_sk, agent_pk) {
        let msg = canonical_bundle_bytes(&proof);
        let sig = flux_sqisign::sign(&msg, sk, pk)
            .map_err(|_| ProvenanceError::InvalidSignature)?;
        proof.sqisign_pubkey = pk.to_vec();
        proof.sqisign_sig = sig;
    }

    Ok(proof)
}

/// Emit a require-BOTH hybrid proof: the canonical bundle is signed by SQIsign
/// AND Ed25519 through `flux_sqisign::hybrid`, which binds the (scheme, pubkey)
/// set into each leg's signing input. A break in either family cannot forge the
/// proof, and a verifier rejects any single-leg-stripped bundle.
pub fn emit_signed_hybrid(
    ctx: &ProvenanceContext,
    sqi_sk: &[u8],
    sqi_pk: &[u8],
    ed_sk: &[u8],
    ed_pk: &[u8],
) -> Result<ProvenanceProof, ProvenanceError> {
    use flux_sqisign::hybrid::SchemeId;
    let mut proof = emit(ctx)?;
    let record = canonical_bundle_bytes(&proof);

    let mut keys = std::collections::HashMap::new();
    keys.insert(SchemeId::SQIsign, (sqi_sk.to_vec(), sqi_pk.to_vec()));
    keys.insert(SchemeId::Ed25519, (ed_sk.to_vec(), ed_pk.to_vec()));

    let bundle = flux_sqisign::hybrid::hybrid_sign(
        &record,
        &keys,
        &[SchemeId::SQIsign, SchemeId::Ed25519],
    )
    .map_err(|_| ProvenanceError::InvalidSignature)?;

    for leg in bundle.signatures {
        match leg.scheme {
            SchemeId::SQIsign => {
                proof.sqisign_pubkey = leg.public_key;
                proof.sqisign_sig = leg.signature;
            }
            SchemeId::Ed25519 => {
                proof.ed25519_pubkey = leg.public_key;
                proof.ed25519_sig = leg.signature;
            }
            SchemeId::Dilithium5 => {}
        }
    }
    Ok(proof)
}

/// Canonical byte serialization of the bundle (used as the signing input).
/// Excludes the signature fields themselves.
fn canonical_bundle_bytes(p: &ProvenanceProof) -> Vec<u8> {
    let mut out = Vec::with_capacity(256);
    out.push(p.version);
    out.extend_from_slice(&p.artifact_hash);
    out.extend_from_slice(&p.source_hash);
    out.extend_from_slice(&p.fluxc_version.to_le_bytes());
    out.extend_from_slice(&p.fluxc_git);
    out.extend_from_slice(&p.agent_wallet);
    out.extend_from_slice(&p.swarm_task_id);
    out.extend_from_slice(&p.settle_tx);
    out.extend_from_slice(&p.timestamp_us.to_le_bytes());
    out
}

/// Verify a proof against the artifact bytes. The signature check is skipped if the
/// proof has empty signature fields (unsigned scaffold mode).
pub fn verify(
    artifact_bytes: &[u8],
    proof: &ProvenanceProof,
) -> Result<VerifiedProof, ProvenanceError> {
    let recomputed = blake3_bytes(artifact_bytes);
    if recomputed != proof.artifact_hash {
        return Err(ProvenanceError::ArtifactHashMismatch {
            expected: hex(&proof.artifact_hash),
            got: hex(&recomputed),
        });
    }
    // Signature check. Three modes, decided by which legs are present:
    //   - Ed25519 leg present  -> REQUIRE-BOTH hybrid: SQIsign AND Ed25519 must
    //     validate over the bound record (a stripped/extended bundle is rejected).
    //   - only SQIsign present -> legacy single-leg verify (backward compatible).
    //   - neither present      -> unsigned scaffold, hash-only.
    if !proof.ed25519_sig.is_empty() {
        use flux_sqisign::hybrid::{HybridSignature, SchemeId, SchemeSignature};
        let record = canonical_bundle_bytes(proof);
        let bundle = HybridSignature {
            version: 1,
            signatures: vec![
                SchemeSignature {
                    scheme: SchemeId::SQIsign,
                    public_key: proof.sqisign_pubkey.clone(),
                    signature: proof.sqisign_sig.clone(),
                    verified: false,
                },
                SchemeSignature {
                    scheme: SchemeId::Ed25519,
                    public_key: proof.ed25519_pubkey.clone(),
                    signature: proof.ed25519_sig.clone(),
                    verified: false,
                },
            ],
        };
        let r = flux_sqisign::hybrid::hybrid_verify(
            &record,
            &bundle,
            &[SchemeId::SQIsign, SchemeId::Ed25519],
        );
        if !r.all_valid {
            return Err(ProvenanceError::InvalidSignature);
        }
    } else if !proof.sqisign_sig.is_empty() && !proof.sqisign_pubkey.is_empty() {
        let msg = canonical_bundle_bytes(proof);
        match flux_sqisign::verify(&msg, &proof.sqisign_sig, &proof.sqisign_pubkey) {
            Ok(true) => {}
            _ => return Err(ProvenanceError::InvalidSignature),
        }
    }
    let on_chain_backed = proof.settle_tx != [0u8; 32];
    Ok(VerifiedProof {
        agent_wallet: proof.agent_wallet,
        source_hash: proof.source_hash,
        artifact_hash: proof.artifact_hash,
        timestamp_us: proof.timestamp_us,
        settle_tx: if on_chain_backed { Some(proof.settle_tx) } else { None },
        on_chain_backed,
    })
}

/// Load the agent's SQIsign keypair from the FLUX_AGENT_KEY_PATH env var (a JSON file
/// with {"sk": [..], "pk": [..]}), or from explicit FLUX_AGENT_SK_HEX + FLUX_AGENT_PK_HEX
/// envs. Returns None if no keys are configured.
pub fn load_agent_keys() -> Option<(Vec<u8>, Vec<u8>)> {
    if let (Ok(sk_hex), Ok(pk_hex)) = (
        std::env::var("FLUX_AGENT_SK_HEX"),
        std::env::var("FLUX_AGENT_PK_HEX"),
    ) {
        if let (Some(sk), Some(pk)) = (parse_hex(&sk_hex), parse_hex(&pk_hex)) {
            return Some((sk, pk));
        }
    }
    let path = std::env::var("FLUX_AGENT_KEY_PATH").ok()
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
            format!("{}/.flux-agent-key.json", home)
        });
    let raw = std::fs::read_to_string(&path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let sk = v.get("sk_hex").and_then(|s| s.as_str()).and_then(parse_hex)?;
    let pk = v.get("pk_hex").and_then(|s| s.as_str()).and_then(parse_hex)?;
    Some((sk, pk))
}

/// Generate a fresh SQIsign Level 5 keypair and persist it to disk. Idempotent: if a
/// keypair already exists at the path, returns the existing one without overwriting.
pub fn ensure_agent_key(path: Option<&str>) -> Result<(Vec<u8>, Vec<u8>), ProvenanceError> {
    let path_owned = path
        .map(|s| s.to_string())
        .or_else(|| std::env::var("FLUX_AGENT_KEY_PATH").ok())
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
            format!("{}/.flux-agent-key.json", home)
        });
    if let Ok(raw) = std::fs::read_to_string(&path_owned) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
            if let (Some(sk_hex), Some(pk_hex)) = (
                v.get("sk_hex").and_then(|s| s.as_str()),
                v.get("pk_hex").and_then(|s| s.as_str()),
            ) {
                if let (Some(sk), Some(pk)) = (parse_hex(sk_hex), parse_hex(pk_hex)) {
                    return Ok((sk, pk));
                }
            }
        }
    }
    let (sk, pk) = flux_sqisign::keygen();
    let body = serde_json::json!({
        "level": "Level5",
        "sk_hex": hex(&sk),
        "pk_hex": hex(&pk),
        "created_at_us": SystemTime::now().duration_since(UNIX_EPOCH)
            .map(|d| d.as_micros() as u64).unwrap_or(0),
    });
    let serialized = serde_json::to_vec_pretty(&body)
        .map_err(|e| ProvenanceError::Serde(e.to_string()))?;
    write_key_file(&path_owned, &serialized)
        .map_err(|e| ProvenanceError::Serde(format!("write {}: {}", path_owned, e)))?;
    Ok((sk, pk))
}

/// Load hybrid agent keys `(sqi_sk, sqi_pk, ed_sk, ed_pk)`, or None if either
/// leg is absent. SQIsign leg via [`load_agent_keys`]; Ed25519 leg via
/// `ed_sk_hex`/`ed_pk_hex` in the key file, or `FLUX_AGENT_ED_SK_HEX` /
/// `FLUX_AGENT_ED_PK_HEX` envs.
pub fn load_agent_keys_hybrid() -> Option<(Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>)> {
    let (sqi_sk, sqi_pk) = load_agent_keys()?;
    if let (Ok(esk), Ok(epk)) = (
        std::env::var("FLUX_AGENT_ED_SK_HEX"),
        std::env::var("FLUX_AGENT_ED_PK_HEX"),
    ) {
        if let (Some(esk), Some(epk)) = (parse_hex(&esk), parse_hex(&epk)) {
            return Some((sqi_sk, sqi_pk, esk, epk));
        }
    }
    let path = std::env::var("FLUX_AGENT_KEY_PATH").ok().unwrap_or_else(|| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        format!("{}/.flux-agent-key.json", home)
    });
    let raw = std::fs::read_to_string(&path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let esk = v.get("ed_sk_hex").and_then(|s| s.as_str()).and_then(parse_hex)?;
    let epk = v.get("ed_pk_hex").and_then(|s| s.as_str()).and_then(parse_hex)?;
    Some((sqi_sk, sqi_pk, esk, epk))
}

/// Ensure the agent has BOTH a SQIsign and an Ed25519 key, persisted in the key
/// file. Idempotent and additive: an agent created before the hybrid upgrade
/// keeps its SQIsign identity and gains an Ed25519 leg merged into the same file.
pub fn ensure_agent_keys_hybrid(
    path: Option<&str>,
) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>), ProvenanceError> {
    // SQIsign leg first (creates the file if absent, preserves an existing one).
    let (sqi_sk, sqi_pk) = ensure_agent_key(path)?;
    let path_owned = path
        .map(|s| s.to_string())
        .or_else(|| std::env::var("FLUX_AGENT_KEY_PATH").ok())
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
            format!("{}/.flux-agent-key.json", home)
        });
    let mut v: serde_json::Value = std::fs::read_to_string(&path_owned)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if let (Some(esk), Some(epk)) = (
        v.get("ed_sk_hex").and_then(|s| s.as_str()).and_then(parse_hex),
        v.get("ed_pk_hex").and_then(|s| s.as_str()).and_then(parse_hex),
    ) {
        return Ok((sqi_sk, sqi_pk, esk, epk));
    }
    let (esk, epk) = flux_sqisign::hybrid::ed25519_keygen();
    v["ed_sk_hex"] = serde_json::Value::String(hex(&esk));
    v["ed_pk_hex"] = serde_json::Value::String(hex(&epk));
    v["ed_scheme"] = serde_json::Value::String("Ed25519".to_string());
    let serialized =
        serde_json::to_vec_pretty(&v).map_err(|e| ProvenanceError::Serde(e.to_string()))?;
    write_key_file(&path_owned, &serialized)
        .map_err(|e| ProvenanceError::Serde(format!("write {}: {}", path_owned, e)))?;
    Ok((sqi_sk, sqi_pk, esk, epk))
}

/// SEC-003: the key file holds `sk_hex` (the agent's SQIsign signing identity).
/// Create it 0600 from the first byte — `std::fs::write` would leave a 0644
/// world-readable window under the default umask — and re-tighten the mode if
/// the file pre-existed from an older fluxc.
fn write_key_file(path: &str, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    #[cfg(unix)]
    {
        // mode() only applies on CREATE — an existing 0644 file keeps its mode,
        // so tighten it explicitly.
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(bytes)
}

fn parse_hex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim().trim_start_matches("0x");
    if s.len() % 2 != 0 { return None; }
    let mut out = Vec::with_capacity(s.len() / 2);
    for i in (0..s.len()).step_by(2) {
        let byte = u8::from_str_radix(&s[i..i+2], 16).ok()?;
        out.push(byte);
    }
    Some(out)
}

/// Serialize a proof to JSON bytes suitable for `<file>.o.proof` storage.
pub fn to_json_bytes(proof: &ProvenanceProof) -> Result<Vec<u8>, ProvenanceError> {
    serde_json::to_vec_pretty(proof).map_err(|e| ProvenanceError::Serde(e.to_string()))
}

/// Deserialize a proof from JSON bytes.
pub fn from_json_bytes(bytes: &[u8]) -> Result<ProvenanceProof, ProvenanceError> {
    serde_json::from_slice(bytes).map_err(|e| ProvenanceError::Serde(e.to_string()))
}

fn blake3_bytes(input: &[u8]) -> [u8; 32] {
    let mut h = Hasher::new();
    h.update(input);
    *h.finalize().as_bytes()
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_then_verify_roundtrip() {
        let ctx = ProvenanceContext {
            artifact_bytes: b"fake-elf-bytes".to_vec(),
            source_bytes: b"pub fn add(a: i64, b: i64) -> i64 { a + b }".to_vec(),
            agent_wallet: [0x42; 32],
            swarm_task_id: [0xaa; 16],
            settle_tx: Some([0xbb; 32]),
            fluxc_git: [0xcc; 20],
            fluxc_version: 0x0a01_0000,
        };
        let proof = emit(&ctx).unwrap();
        let verified = verify(&ctx.artifact_bytes, &proof).unwrap();
        assert_eq!(verified.agent_wallet, [0x42; 32]);
        assert!(verified.on_chain_backed);
    }

    #[test]
    fn canonical_bundle_excludes_sig_and_is_field_sensitive() {
        // SEC-018: the SIGNING INPUT is canonical_bundle_bytes(), NOT the JSON
        // serialization — so JSON field reordering can never produce a different
        // message that verifies against the same signature. Pin two properties:
        //   (1) the canonical bytes do NOT contain the signature fields, and
        //   (2) flipping ANY committed field changes the canonical bytes (so a
        //       forged bundle can't collide with a captured signature).
        let ctx = ProvenanceContext {
            artifact_bytes: b"artifact".to_vec(),
            source_bytes: b"source".to_vec(),
            agent_wallet: [0x11; 32],
            swarm_task_id: [0x22; 16],
            settle_tx: Some([0x33; 32]),
            fluxc_git: [0x44; 20],
            fluxc_version: 7,
        };
        let signed = emit_with_keys(&ctx, Some(b"x"), Some(b"y")); // see helper below
        let proof = signed;
        let canon = canonical_bundle_bytes(&proof);
        // (1) signature bytes must not leak into the signing input
        if !proof.sqisign_sig.is_empty() {
            assert!(!canon.windows(proof.sqisign_sig.len().max(1)).any(|w| w == proof.sqisign_sig.as_slice()));
        }
        // (2) any committed-field change perturbs the canonical bytes
        let mut p2 = proof.clone();
        p2.agent_wallet[0] ^= 0xff;
        assert_ne!(canon, canonical_bundle_bytes(&p2), "wallet change must alter canonical bytes");
        let mut p3 = proof.clone();
        p3.timestamp_us = p3.timestamp_us.wrapping_add(1);
        assert_ne!(canon, canonical_bundle_bytes(&p3), "timestamp change must alter canonical bytes");
        let mut p4 = proof.clone();
        p4.artifact_hash[31] ^= 0x01;
        assert_ne!(canon, canonical_bundle_bytes(&p4), "artifact_hash change must alter canonical bytes");
    }

    // Build a proof without keys, then fill placeholder sig/pubkey for the
    // canonicalization test (we only assert about canonical_bundle_bytes, which
    // excludes those fields, so placeholder values are fine).
    fn emit_with_keys(ctx: &ProvenanceContext, sig: Option<&[u8]>, pk: Option<&[u8]>) -> ProvenanceProof {
        let mut p = emit(ctx).unwrap();
        if let Some(s) = sig { p.sqisign_sig = s.to_vec(); }
        if let Some(k) = pk { p.sqisign_pubkey = k.to_vec(); }
        p
    }

    #[test]
    fn verify_detects_artifact_tampering() {
        let ctx = ProvenanceContext {
            artifact_bytes: b"original".to_vec(),
            source_bytes: b"src".to_vec(),
            agent_wallet: [0; 32],
            swarm_task_id: [0; 16],
            settle_tx: None,
            fluxc_git: [0; 20],
            fluxc_version: 0,
        };
        let proof = emit(&ctx).unwrap();
        let result = verify(b"tampered", &proof);
        assert!(matches!(result, Err(ProvenanceError::ArtifactHashMismatch { .. })));
    }

    #[test]
    fn signed_emit_then_verify_roundtrip() {
        let (sk, pk) = flux_sqisign::keygen();
        let ctx = ProvenanceContext {
            artifact_bytes: b"signed-elf-bytes".to_vec(),
            source_bytes: b"pub fn add(a: i64, b: i64) -> i64 { a + b }".to_vec(),
            agent_wallet: [0x42; 32],
            swarm_task_id: [0xaa; 16],
            settle_tx: Some([0xbb; 32]),
            fluxc_git: [0xcc; 20],
            fluxc_version: 0x0a01_0000,
        };
        let proof = emit_signed(&ctx, &sk, &pk).expect("sign");
        assert!(!proof.sqisign_sig.is_empty(), "signature should be populated");
        assert!(!proof.sqisign_pubkey.is_empty(), "pubkey should be populated");
        let verified = verify(&ctx.artifact_bytes, &proof).expect("verify");
        assert_eq!(verified.agent_wallet, [0x42; 32]);
        assert!(verified.on_chain_backed);
    }

    #[test]
    fn signed_verify_detects_tampered_bundle() {
        let (sk, pk) = flux_sqisign::keygen();
        let ctx = ProvenanceContext {
            artifact_bytes: b"original".to_vec(), source_bytes: b"src".to_vec(),
            agent_wallet: [1; 32], swarm_task_id: [2; 16],
            settle_tx: None, fluxc_git: [3; 20], fluxc_version: 1,
        };
        let mut proof = emit_signed(&ctx, &sk, &pk).unwrap();
        // Tamper with a metadata field — signature should no longer verify.
        proof.timestamp_us = proof.timestamp_us.wrapping_add(1);
        let r = verify(&ctx.artifact_bytes, &proof);
        assert!(matches!(r, Err(ProvenanceError::InvalidSignature)));
    }

    #[test]
    fn hybrid_emit_then_verify_roundtrip() {
        let (ssk, spk) = flux_sqisign::keygen();
        let (esk, epk) = flux_sqisign::hybrid::ed25519_keygen();
        let ctx = ProvenanceContext {
            artifact_bytes: b"hybrid-elf-bytes".to_vec(),
            source_bytes: b"pub fn add(a:i64,b:i64)->i64{a+b}".to_vec(),
            agent_wallet: [0x42; 32],
            swarm_task_id: [0xaa; 16],
            settle_tx: Some([0xbb; 32]),
            fluxc_git: [0xcc; 20],
            fluxc_version: 0x0a01_0000,
        };
        let proof = emit_signed_hybrid(&ctx, &ssk, &spk, &esk, &epk).expect("hybrid sign");
        assert!(!proof.sqisign_sig.is_empty(), "sqisign leg present");
        assert!(!proof.ed25519_sig.is_empty(), "ed25519 leg present");
        let verified = verify(&ctx.artifact_bytes, &proof).expect("hybrid verify");
        assert_eq!(verified.agent_wallet, [0x42; 32]);
    }

    #[test]
    fn hybrid_verify_rejects_stripped_ed25519_leg() {
        // Forge attempt: drop the Ed25519 leg, hoping the verifier accepts the
        // surviving SQIsign signature. A require-both proof MUST reject it — and
        // it must NOT silently fall through to the legacy single-SQIsign path.
        let (ssk, spk) = flux_sqisign::keygen();
        let (esk, epk) = flux_sqisign::hybrid::ed25519_keygen();
        let ctx = ProvenanceContext {
            artifact_bytes: b"strip-me".to_vec(), source_bytes: b"s".to_vec(),
            agent_wallet: [1; 32], swarm_task_id: [2; 16],
            settle_tx: None, fluxc_git: [3; 20], fluxc_version: 1,
        };
        let mut proof = emit_signed_hybrid(&ctx, &ssk, &spk, &esk, &epk).unwrap();
        proof.ed25519_pubkey.clear();
        proof.ed25519_sig.clear();
        // Now it looks like a legacy single-SQIsign proof, but the SQIsign leg
        // was signed over the HYBRID binding, not the legacy canonical bundle,
        // so the legacy verify path must also fail.
        let r = verify(&ctx.artifact_bytes, &proof);
        assert!(matches!(r, Err(ProvenanceError::InvalidSignature)),
            "stripped hybrid proof must not verify, got {r:?}");
    }

    #[test]
    fn hybrid_verify_detects_tampered_bundle() {
        let (ssk, spk) = flux_sqisign::keygen();
        let (esk, epk) = flux_sqisign::hybrid::ed25519_keygen();
        let ctx = ProvenanceContext {
            artifact_bytes: b"orig".to_vec(), source_bytes: b"s".to_vec(),
            agent_wallet: [1; 32], swarm_task_id: [2; 16],
            settle_tx: None, fluxc_git: [3; 20], fluxc_version: 1,
        };
        let mut proof = emit_signed_hybrid(&ctx, &ssk, &spk, &esk, &epk).unwrap();
        proof.timestamp_us = proof.timestamp_us.wrapping_add(1);
        assert!(matches!(verify(&ctx.artifact_bytes, &proof), Err(ProvenanceError::InvalidSignature)));
    }

    #[test]
    fn legacy_single_sqisign_proof_still_verifies() {
        // Backward compatibility: a proof with only the SQIsign leg (the pre-
        // hybrid format) must still verify through the legacy branch.
        let (sk, pk) = flux_sqisign::keygen();
        let ctx = ProvenanceContext {
            artifact_bytes: b"legacy".to_vec(), source_bytes: b"s".to_vec(),
            agent_wallet: [9; 32], swarm_task_id: [8; 16],
            settle_tx: None, fluxc_git: [7; 20], fluxc_version: 2,
        };
        let proof = emit_signed(&ctx, &sk, &pk).unwrap();
        assert!(proof.ed25519_sig.is_empty(), "legacy proof has no ed leg");
        assert!(verify(&ctx.artifact_bytes, &proof).is_ok());
    }

    #[test]
    fn json_roundtrip() {
        let ctx = ProvenanceContext {
            artifact_bytes: b"x".to_vec(), source_bytes: b"y".to_vec(),
            agent_wallet: [1; 32], swarm_task_id: [2; 16],
            settle_tx: None, fluxc_git: [3; 20], fluxc_version: 1,
        };
        let proof = emit(&ctx).unwrap();
        let bytes = to_json_bytes(&proof).unwrap();
        let restored = from_json_bytes(&bytes).unwrap();
        assert_eq!(restored.agent_wallet, proof.agent_wallet);
        assert_eq!(restored.artifact_hash, proof.artifact_hash);
    }
}
