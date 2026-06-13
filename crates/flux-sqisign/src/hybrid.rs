// flux-sqisign/hybrid.rs — Hybrid multi-scheme provenance signatures
//
// Defense-in-depth: sign with multiple INDEPENDENT signature families. A
// break in one family (an isogeny cryptanalysis advance, a quantum attack on
// the curve, or an implementation bug) does not forge a binary, because the
// verifier requires EVERY leg in a caller-fixed REQUIRED scheme-set to validate.
//
//   SQIsign     — isogeny-based, 292 B sig / 129 B pk, NIST PQC Level 5
//   Ed25519     — elliptic curve, 64 B sig / 32 B pk, classical
//   Dilithium5  — lattice-based, 4595 B sig, NIST PQC Level 5 (optional 3rd leg
//                 for root keys; integration pending — see hybrid_keygen)
//
// Default provenance hedge = SQIsign + Ed25519 ≈ 356-byte signature — still
// smaller than ONE Dilithium-5 signature, yet a break in either family is
// survived.
//
// SECURITY MODEL (read before changing anything here):
//   1. ACCEPTANCE IS REQUIRE-ALL. There is NO "fall back to whichever leg still
//      verifies" — that would make security the WEAKEST leg, inverting the whole
//      point. A previous draft of this file had an `analyze_fallback` helper with
//      exactly that OR-semantics footgun; it is deliberately gone. Fallback is a
//      key-ROTATION concern, never an acceptance concern.
//   2. THE SCHEME-SET IS BOUND. Every leg signs over a domain-separated digest
//      that commits to (domain_tag, version, the canonical set of (scheme_id,
//      public_key), the record). So a forger who breaks one family cannot STRIP
//      the bundle down to a single surviving leg: the verifier (a) rejects any
//      bundle whose present set != the required set, and (b) rebuilds the binding
//      from the present pubkeys, so a stripped/extended set produces a different
//      binding and every signature fails. The bundle's self-declared scheme
//      count is NEVER trusted for acceptance.
//
// This is OUT-OF-BAND release provenance. It does NOT touch the per-tx ed25519
// batch-auth path or turbo-sync, and it does NOT alter the crate's existing
// single-SQIsign keygen/sign/verify (292 B) surface that sigil-tx et al. link.

use std::collections::HashMap;

use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use rand::RngCore;

/// Domain separator for the hybrid provenance binding. Bump the trailing
/// version if the binding layout ever changes.
const DOMAIN_TAG: &[u8] = b"flux-sigil/hybrid-provenance/v1";

const ED25519_SIG_LEN: usize = 64;
const ED25519_PK_LEN: usize = 32;
const ED25519_SK_LEN: usize = 32;

// ── Scheme identity ──

/// Identity of a signature scheme in the hybrid. The numeric value is the
/// canonical ordering key and the on-wire `scheme_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SchemeId {
    SQIsign = 1,
    Dilithium5 = 2,
    Ed25519 = 3,
}

impl SchemeId {
    pub fn name(&self) -> &'static str {
        match self {
            SchemeId::SQIsign => "SQIsign (isogeny)",
            SchemeId::Dilithium5 => "Dilithium5 (lattice)",
            SchemeId::Ed25519 => "Ed25519 (edwards)",
        }
    }
}

/// A single scheme's signature within a hybrid.
#[derive(Debug, Clone)]
pub struct SchemeSignature {
    pub scheme: SchemeId,
    pub public_key: Vec<u8>,
    pub signature: Vec<u8>,
    /// Filled during verification (per-leg diagnostic only — NOT an acceptance
    /// signal; acceptance is `HybridResult::all_valid`).
    pub verified: bool,
}

/// A hybrid signature bundle: one [`SchemeSignature`] per leg.
#[derive(Debug, Clone)]
pub struct HybridSignature {
    pub version: u8,
    pub signatures: Vec<SchemeSignature>,
}

/// Result of hybrid verification against a REQUIRED scheme-set.
#[derive(Debug, Clone)]
pub struct HybridResult {
    /// The ONLY acceptance signal: the present set equalled the required set AND
    /// every required leg's signature validated over the bound digest.
    pub all_valid: bool,
    /// Per-leg results (diagnostics).
    pub results: Vec<SchemeSignature>,
    pub passed_schemes: Vec<SchemeId>,
    pub failed_schemes: Vec<SchemeId>,
    /// Non-empty when verification was refused before per-leg checks (e.g.
    /// present set != required set).
    pub reason: String,
    pub total_size: usize,
}

// ── ed25519 leg (the classical hedge) ──

fn ed25519_keygen() -> (Vec<u8>, Vec<u8>) {
    // 32 random seed bytes ARE the ed25519 secret key; from_bytes avoids the
    // optional `rand_core` feature on ed25519-dalek.
    let mut seed = [0u8; ED25519_SK_LEN];
    RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut seed);
    let sk = SigningKey::from_bytes(&seed);
    let pk = sk.verifying_key();
    (sk.to_bytes().to_vec(), pk.to_bytes().to_vec())
}

fn ed25519_sign(msg: &[u8], sk_bytes: &[u8]) -> Result<Vec<u8>, String> {
    let arr: [u8; ED25519_SK_LEN] = sk_bytes
        .try_into()
        .map_err(|_| format!("ed25519: sk must be {ED25519_SK_LEN} bytes, got {}", sk_bytes.len()))?;
    let sk = SigningKey::from_bytes(&arr);
    Ok(sk.sign(msg).to_bytes().to_vec())
}

fn ed25519_verify(msg: &[u8], sig_bytes: &[u8], pk_bytes: &[u8]) -> Result<bool, String> {
    let pk_arr: [u8; ED25519_PK_LEN] = pk_bytes
        .try_into()
        .map_err(|_| format!("ed25519: pk must be {ED25519_PK_LEN} bytes, got {}", pk_bytes.len()))?;
    let sig_arr: [u8; ED25519_SIG_LEN] = sig_bytes
        .try_into()
        .map_err(|_| format!("ed25519: sig must be {ED25519_SIG_LEN} bytes, got {}", sig_bytes.len()))?;
    let vk = VerifyingKey::from_bytes(&pk_arr).map_err(|e| format!("ed25519: bad pk: {e}"))?;
    let sig = ed25519_dalek::Signature::from_bytes(&sig_arr);
    // verify_strict rejects small-order / malleable points.
    Ok(vk.verify_strict(msg, &sig).is_ok())
}

// ── The binding: what every leg actually signs ──

/// Compute the domain-separated 32-byte digest that EVERY leg signs. It commits
/// to the domain tag, the bundle version, the canonical (scheme_id, public_key)
/// set, and the record. Canonical = pairs sorted by scheme_id, so signer and
/// verifier agree regardless of input order.
fn build_binding(version: u8, pairs: &[(SchemeId, Vec<u8>)], record: &[u8]) -> [u8; 32] {
    let mut sorted: Vec<&(SchemeId, Vec<u8>)> = pairs.iter().collect();
    sorted.sort_by_key(|(s, _)| *s as u8);

    let mut h = blake3::Hasher::new();
    h.update(DOMAIN_TAG);
    h.update(&[version]);
    h.update(&[sorted.len() as u8]);
    for (scheme, pk) in sorted {
        h.update(&[*scheme as u8]);
        h.update(&(pk.len() as u16).to_le_bytes());
        h.update(pk);
    }
    h.update(&(record.len() as u32).to_le_bytes());
    h.update(record);
    *h.finalize().as_bytes()
}

// ── Key generation ──

/// Generate keys for all requested schemes. SQIsign and Ed25519 are live;
/// Dilithium5 (the optional lattice leg for root keys) is integration-pending.
pub fn hybrid_keygen(
    schemes: &[SchemeId],
) -> Result<HashMap<SchemeId, (Vec<u8>, Vec<u8>)>, String> {
    let mut keys = HashMap::new();
    for &scheme in schemes {
        let (sk, pk) = match scheme {
            SchemeId::SQIsign => crate::keygen(),
            SchemeId::Ed25519 => ed25519_keygen(),
            SchemeId::Dilithium5 => {
                return Err("Dilithium5 keygen: lattice leg integration pending".into())
            }
        };
        keys.insert(scheme, (sk, pk));
    }
    Ok(keys)
}

// ── Signing ──

/// Sign `record` with every scheme in `schemes`, producing a require-all hybrid
/// bundle. Each leg signs the SAME domain-separated binding over the full
/// (scheme_id, pubkey) set, so no leg can later be lifted out of context.
pub fn hybrid_sign(
    record: &[u8],
    keys: &HashMap<SchemeId, (Vec<u8>, Vec<u8>)>,
    schemes: &[SchemeId],
) -> Result<HybridSignature, String> {
    if schemes.is_empty() {
        return Err("hybrid_sign: at least one scheme required".into());
    }
    let version: u8 = 1;

    // Bind to the canonical pubkey set BEFORE signing.
    let pairs: Vec<(SchemeId, Vec<u8>)> = schemes
        .iter()
        .map(|&s| {
            keys.get(&s)
                .map(|(_, pk)| (s, pk.clone()))
                .ok_or_else(|| format!("hybrid_sign: no key for {:?}", s))
        })
        .collect::<Result<_, _>>()?;
    let binding = build_binding(version, &pairs, record);

    let mut signatures = Vec::with_capacity(schemes.len());
    for &scheme in schemes {
        let (sk, pk) = keys.get(&scheme).expect("checked above");
        let signature = match scheme {
            SchemeId::SQIsign => crate::sign(&binding, sk, pk)?,
            SchemeId::Ed25519 => ed25519_sign(&binding, sk)?,
            SchemeId::Dilithium5 => {
                return Err("Dilithium5 signing: lattice leg integration pending".into())
            }
        };
        signatures.push(SchemeSignature {
            scheme,
            public_key: pk.clone(),
            signature,
            verified: false,
        });
    }

    Ok(HybridSignature { version, signatures })
}

// ── Verification (require-all, scheme-set bound) ──

/// Verify a hybrid bundle against a caller-supplied REQUIRED scheme-set.
///
/// Acceptance (`all_valid == true`) demands BOTH:
///   1. the bundle's present set equals `required` exactly (no missing leg, no
///      extra leg, no duplicate leg), and
///   2. every leg's signature validates over the binding rebuilt from the
///      present pubkeys.
///
/// There is no partial credit and no fallback. A single-leg-stripped bundle, an
/// extended bundle, or a tampered record all yield `all_valid == false`.
pub fn hybrid_verify(
    record: &[u8],
    hybrid: &HybridSignature,
    required: &[SchemeId],
) -> HybridResult {
    let mut results = hybrid.signatures.clone();
    let total_size: usize =
        1 + results.iter().map(|s| 1 + 2 + s.signature.len() + 2 + s.public_key.len()).sum::<usize>();

    // (1) Required-set check: present multiset must equal required set, no dupes.
    let mut req: Vec<SchemeId> = required.to_vec();
    req.sort();
    req.dedup();
    let mut present: Vec<SchemeId> = results.iter().map(|s| s.scheme).collect();
    present.sort();
    let present_has_dupes = present.windows(2).any(|w| w[0] == w[1]);
    let mut present_set = present.clone();
    present_set.dedup();

    if required.is_empty() {
        return refused(results, "hybrid_verify: required scheme-set is empty", total_size);
    }
    if present_has_dupes {
        return refused(results, "hybrid_verify: duplicate scheme leg in bundle", total_size);
    }
    if present_set != req {
        return refused(
            results,
            &format!("hybrid_verify: present set {present_set:?} != required {req:?}"),
            total_size,
        );
    }

    // (2) Rebuild the binding from the PRESENT pubkeys and verify each leg.
    let pairs: Vec<(SchemeId, Vec<u8>)> =
        results.iter().map(|s| (s.scheme, s.public_key.clone())).collect();
    let binding = build_binding(hybrid.version, &pairs, record);

    let mut passed = Vec::new();
    let mut failed = Vec::new();
    for sig in &mut results {
        let valid = match sig.scheme {
            SchemeId::SQIsign => crate::verify(&binding, &sig.signature, &sig.public_key)
                .unwrap_or(false),
            SchemeId::Ed25519 => {
                ed25519_verify(&binding, &sig.signature, &sig.public_key).unwrap_or(false)
            }
            SchemeId::Dilithium5 => false, // integration pending → never accepted
        };
        sig.verified = valid;
        if valid {
            passed.push(sig.scheme);
        } else {
            failed.push(sig.scheme);
        }
    }

    HybridResult {
        all_valid: failed.is_empty(),
        results,
        passed_schemes: passed,
        failed_schemes: failed,
        reason: String::new(),
        total_size,
    }
}

fn refused(results: Vec<SchemeSignature>, reason: &str, total_size: usize) -> HybridResult {
    let failed = results.iter().map(|s| s.scheme).collect();
    HybridResult {
        all_valid: false,
        results,
        passed_schemes: Vec::new(),
        failed_schemes: failed,
        reason: reason.to_string(),
        total_size,
    }
}

// ── Serialization ──
//
// Layout: [version:1][num_schemes:1] then per leg
//   [scheme_id:1][sig_len:2 LE][sig...][pk_len:2 LE][pk...]
// The wire form carries the bundle's self-declared set, but `hybrid_verify`
// never trusts it for acceptance — it checks the present set against the
// caller's required set and rebinds.

pub fn serialize_hybrid(hybrid: &HybridSignature) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.push(hybrid.version);
    bytes.push(hybrid.signatures.len() as u8);
    for sig in &hybrid.signatures {
        bytes.push(sig.scheme as u8);
        bytes.extend_from_slice(&(sig.signature.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&sig.signature);
        bytes.extend_from_slice(&(sig.public_key.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&sig.public_key);
    }
    bytes
}

pub fn deserialize_hybrid(bytes: &[u8]) -> Result<HybridSignature, String> {
    if bytes.len() < 2 {
        return Err("hybrid sig too short".into());
    }
    let version = bytes[0];
    let num_sigs = bytes[1] as usize;
    let mut pos = 2;
    let mut signatures = Vec::with_capacity(num_sigs);

    for _ in 0..num_sigs {
        if pos + 3 > bytes.len() {
            return Err("truncated hybrid sig (header)".into());
        }
        let scheme = match bytes[pos] {
            1 => SchemeId::SQIsign,
            2 => SchemeId::Dilithium5,
            3 => SchemeId::Ed25519,
            other => return Err(format!("unknown scheme id: {other}")),
        };
        pos += 1;

        let sig_len = u16::from_le_bytes([bytes[pos], bytes[pos + 1]]) as usize;
        pos += 2;
        if pos + sig_len + 2 > bytes.len() {
            return Err("truncated signature".into());
        }
        let signature = bytes[pos..pos + sig_len].to_vec();
        pos += sig_len;

        let pk_len = u16::from_le_bytes([bytes[pos], bytes[pos + 1]]) as usize;
        pos += 2;
        if pos + pk_len > bytes.len() {
            return Err("truncated public key".into());
        }
        let public_key = bytes[pos..pos + pk_len].to_vec();
        pos += pk_len;

        signatures.push(SchemeSignature { scheme, public_key, signature, verified: false });
    }

    Ok(HybridSignature { version, signatures })
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    const REQUIRED: &[SchemeId] = &[SchemeId::SQIsign, SchemeId::Ed25519];
    const RECORD: &[u8] = b"artifact_blake3=... source_blake3=... fluxc_version=1.0";

    fn keys() -> HashMap<SchemeId, (Vec<u8>, Vec<u8>)> {
        hybrid_keygen(REQUIRED).expect("keygen sqisign+ed25519")
    }

    #[test]
    fn ed25519_leg_roundtrip() {
        let (sk, pk) = ed25519_keygen();
        let sig = ed25519_sign(b"hello", &sk).unwrap();
        assert_eq!(sig.len(), ED25519_SIG_LEN);
        assert_eq!(pk.len(), ED25519_PK_LEN);
        assert!(ed25519_verify(b"hello", &sig, &pk).unwrap());
        assert!(!ed25519_verify(b"HELLO", &sig, &pk).unwrap());
    }

    #[test]
    fn require_both_roundtrip() {
        let k = keys();
        let bundle = hybrid_sign(RECORD, &k, REQUIRED).unwrap();
        assert_eq!(bundle.signatures.len(), 2);
        let r = hybrid_verify(RECORD, &bundle, REQUIRED);
        assert!(r.all_valid, "both legs must validate: {}", r.reason);
        assert!(r.failed_schemes.is_empty());
        assert!(r.passed_schemes.contains(&SchemeId::SQIsign));
        assert!(r.passed_schemes.contains(&SchemeId::Ed25519));
    }

    #[test]
    fn single_leg_stripped_is_rejected() {
        // Forge attempt: keep only the SQIsign leg, drop Ed25519, present it
        // as if SQIsign alone authorizes the binary.
        let k = keys();
        let mut bundle = hybrid_sign(RECORD, &k, REQUIRED).unwrap();
        bundle.signatures.retain(|s| s.scheme == SchemeId::SQIsign);
        let r = hybrid_verify(RECORD, &bundle, REQUIRED);
        assert!(!r.all_valid, "stripped single-leg bundle MUST be rejected");
        assert!(r.reason.contains("!= required"), "rejection reason: {}", r.reason);
    }

    #[test]
    fn extra_leg_is_rejected() {
        // Append a duplicate/extra leg the required-set didn't ask for.
        let k = keys();
        let mut bundle = hybrid_sign(RECORD, &k, REQUIRED).unwrap();
        let extra = bundle.signatures[0].clone();
        bundle.signatures.push(extra);
        let r = hybrid_verify(RECORD, &bundle, REQUIRED);
        assert!(!r.all_valid, "extra/duplicate leg MUST be rejected");
    }

    #[test]
    fn tampered_record_is_rejected() {
        let k = keys();
        let bundle = hybrid_sign(RECORD, &k, REQUIRED).unwrap();
        let r = hybrid_verify(b"a DIFFERENT record", &bundle, REQUIRED);
        assert!(!r.all_valid, "record tamper must break the binding");
        assert!(r.reason.is_empty(), "should fail at signature check, not set check");
        assert!(!r.failed_schemes.is_empty());
    }

    #[test]
    fn swapped_pubkey_is_rejected() {
        // Replace the ed25519 pubkey with a fresh one (forger doesn't hold the sk).
        let k = keys();
        let mut bundle = hybrid_sign(RECORD, &k, REQUIRED).unwrap();
        let (_, other_pk) = ed25519_keygen();
        for s in &mut bundle.signatures {
            if s.scheme == SchemeId::Ed25519 {
                s.public_key = other_pk.clone();
            }
        }
        let r = hybrid_verify(RECORD, &bundle, REQUIRED);
        assert!(!r.all_valid, "swapped pubkey must fail (binding + sig both change)");
    }

    #[test]
    fn serialization_roundtrip_then_verifies() {
        let k = keys();
        let bundle = hybrid_sign(RECORD, &k, REQUIRED).unwrap();
        let bytes = serialize_hybrid(&bundle);
        let back = deserialize_hybrid(&bytes).unwrap();
        assert_eq!(back.version, bundle.version);
        assert_eq!(back.signatures.len(), 2);
        let r = hybrid_verify(RECORD, &back, REQUIRED);
        assert!(r.all_valid, "deserialized bundle must still verify: {}", r.reason);
    }

    #[test]
    fn dilithium_leg_is_pending_not_silently_ok() {
        let e = hybrid_keygen(&[SchemeId::Dilithium5]).unwrap_err();
        assert!(e.contains("pending"), "dilithium must be explicitly pending, got: {e}");
    }

    #[test]
    fn empty_required_set_is_refused() {
        let k = keys();
        let bundle = hybrid_sign(RECORD, &k, REQUIRED).unwrap();
        let r = hybrid_verify(RECORD, &bundle, &[]);
        assert!(!r.all_valid);
        assert!(r.reason.contains("empty"));
    }
}
