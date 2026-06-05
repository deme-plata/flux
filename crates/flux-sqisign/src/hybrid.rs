// flux-sqisign/hybrid.rs — Hybrid multi-scheme signatures
//
// Defense-in-depth: sign with multiple independent PQ + classical schemes.
// If one is cryptanalyzed, the others still protect.
//
// Tiers:
//   Tier 1: SQIsign     (isogeny-based, 177B, NIST PQC Level 5)
//   Tier 2: Dilithium5   (lattice-based, 4595B, NIST PQC Level 5)
//   Tier 3: ed25519      (elliptic curve, 64B, classical)
//
// Hybrid signature format:
//   [version: 1B] [num_schemes: 1B] [scheme_id: 1B] [sig_len: 2B LE] [sig_bytes...] ...
//
// Combined sig size: ~4,837B (SQIsign 177 + Dilithium5 4595 + ed25519 64 + 1B overhead)
// Verifier checks ALL schemes — all must pass for acceptance.

use std::collections::HashMap;

// ── Hybrid Signer ──

/// Identity of a signature scheme in the hybrid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SchemeId {
    SQIsign = 1,
    Dilithium5 = 2,
    Ed25519 = 3,
}

impl SchemeId {
    fn name(&self) -> &'static str {
        match self {
            SchemeId::SQIsign => "SQIsign (isogeny)",
            SchemeId::Dilithium5 => "Dilithium5 (lattice)",
            SchemeId::Ed25519 => "ed25519 (ECDSA)",
        }
    }
}

/// A single scheme's signature within a hybrid.
#[derive(Debug, Clone)]
pub struct SchemeSignature {
    pub scheme: SchemeId,
    pub public_key: Vec<u8>,
    pub signature: Vec<u8>,
    pub verified: bool,
}

/// A hybrid signature containing multiple scheme signatures.
#[derive(Debug, Clone)]
pub struct HybridSignature {
    pub version: u8,
    pub signatures: Vec<SchemeSignature>,
}

/// Result of hybrid verification.
#[derive(Debug, Clone)]
pub struct HybridResult {
    /// All signatures present and verified.
    pub all_valid: bool,
    /// Per-scheme results.
    pub results: Vec<SchemeSignature>,
    /// Which schemes were verified successfully.
    pub passed_schemes: Vec<SchemeId>,
    /// Which schemes failed verification.
    pub failed_schemes: Vec<SchemeId>,
    /// Total combined signature size in bytes.
    pub total_size: usize,
}

// ── Hybrid Key Generation ──

/// Generate keys for all requested schemes.
pub fn hybrid_keygen(schemes: &[SchemeId]) -> Result<HashMap<SchemeId, (Vec<u8>, Vec<u8>)>, String> {
    let mut keys = HashMap::new();
    for &scheme in schemes {
        let (sk, pk) = match scheme {
            SchemeId::SQIsign => crate::keygen(),
            SchemeId::Dilithium5 => {
                // Use Dilithium5 via the flux-zk crate if available.
                // For now, generate a placeholder — real implementation
                // would call pqcrypto_dilithium::dilithium5::keypair().
                return Err("Dilithium5 keygen: flux-zk integration pending".into());
            }
            SchemeId::Ed25519 => {
                // ed25519 keygen would use ed25519-dalek or ring.
                // For now, generate a placeholder.
                return Err("ed25519 keygen: ring/ed25519-dalek integration pending".into());
            }
        };
        keys.insert(scheme, (sk, pk));
    }
    Ok(keys)
}

// ── Hybrid Signing ──

/// Sign a message with multiple schemes, producing a hybrid signature.
pub fn hybrid_sign(
    msg: &[u8],
    keys: &HashMap<SchemeId, (Vec<u8>, Vec<u8>)>,
    schemes: &[SchemeId],
) -> Result<HybridSignature, String> {
    let mut signatures = Vec::new();

    for &scheme in schemes {
        let (sk, pk) = keys.get(&scheme)
            .ok_or_else(|| format!("No key for scheme {:?}", scheme))?;

        let sig = match scheme {
            SchemeId::SQIsign => crate::sign(msg, sk, pk)?,
            SchemeId::Dilithium5 => {
                return Err("Dilithium5 signing: flux-zk integration pending".into());
            }
            SchemeId::Ed25519 => {
                return Err("ed25519 signing: ring/ed25519-dalek integration pending".into());
            }
        };

        signatures.push(SchemeSignature {
            scheme,
            public_key: pk.clone(),
            signature: sig,
            verified: false, // filled during verification
        });
    }

    Ok(HybridSignature {
        version: 1,
        signatures,
    })
}

// ── Hybrid Verification ──

/// Verify a hybrid signature. ALL schemes must pass for `all_valid = true`.
/// Individual scheme results are reported for diagnostics.
pub fn hybrid_verify(
    msg: &[u8],
    hybrid: &HybridSignature,
    keys: &HashMap<SchemeId, (Vec<u8>, Vec<u8>)>,
) -> Result<HybridResult, String> {
    let mut results = hybrid.signatures.clone();
    let mut passed = Vec::new();
    let mut failed = Vec::new();
    let mut total_size = 1; // version byte

    for sig in &mut results {
        let (_, pk) = keys.get(&sig.scheme)
            .ok_or_else(|| format!("No key for scheme {:?}", sig.scheme))?;

        let valid = match sig.scheme {
            SchemeId::SQIsign => crate::verify(msg, &sig.signature, pk)?,
            SchemeId::Dilithium5 => {
                return Err("Dilithium5 verification: flux-zk integration pending".into());
            }
            SchemeId::Ed25519 => {
                return Err("ed25519 verification: ring/ed25519-dalek integration pending".into());
            }
        };

        sig.verified = valid;
        total_size += 1 + 2 + sig.signature.len(); // scheme_id + len + sig

        if valid {
            passed.push(sig.scheme);
        } else {
            failed.push(sig.scheme);
        }
    }

    Ok(HybridResult {
        all_valid: failed.is_empty(),
        results,
        passed_schemes: passed,
        failed_schemes: failed,
        total_size,
    })
}

// ── Fallback Logic ──

/// Determine the fallback strategy based on what failed.
#[derive(Debug, Clone)]
pub enum FallbackAction {
    /// All schemes passed — use the primary (SQIsign) alone.
    UsePrimary,
    /// SQIsign failed, fall back to Dilithium5.
    FallbackToDilithium5,
    /// SQIsign + Dilithium5 both failed, fall back to ed25519.
    FallbackToEd25519,
    /// All three failed — security completely compromised.
    AllFailed,
}

/// Analyze a hybrid result and recommend a fallback action.
pub fn analyze_fallback(result: &HybridResult) -> FallbackAction {
    if result.all_valid {
        return FallbackAction::UsePrimary;
    }

    let sqi_ok = result.passed_schemes.contains(&SchemeId::SQIsign);
    let dil_ok = result.passed_schemes.contains(&SchemeId::Dilithium5);
    let ed_ok = result.passed_schemes.contains(&SchemeId::Ed25519);

    if sqi_ok {
        FallbackAction::UsePrimary // SQIsign still works even if others failed
    } else if dil_ok {
        FallbackAction::FallbackToDilithium5
    } else if ed_ok {
        FallbackAction::FallbackToEd25519
    } else {
        FallbackAction::AllFailed
    }
}

// ── Serialization ──

/// Serialize a hybrid signature to bytes for storage/transmission.
pub fn serialize_hybrid(hybrid: &HybridSignature) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.push(hybrid.version);
    bytes.push(hybrid.signatures.len() as u8);

    for sig in &hybrid.signatures {
        bytes.push(sig.scheme as u8);
        let len = sig.signature.len() as u16;
        bytes.extend_from_slice(&len.to_le_bytes());
        bytes.extend_from_slice(&sig.signature);
        // Public key length + pk
        let pk_len = sig.public_key.len() as u16;
        bytes.extend_from_slice(&pk_len.to_le_bytes());
        bytes.extend_from_slice(&sig.public_key);
    }

    bytes
}

/// Deserialize a hybrid signature from bytes.
pub fn deserialize_hybrid(bytes: &[u8]) -> Result<HybridSignature, String> {
    if bytes.len() < 2 {
        return Err("Hybrid sig too short".into());
    }

    let version = bytes[0];
    let num_sigs = bytes[1] as usize;
    let mut pos = 2;
    let mut signatures = Vec::new();

    for _ in 0..num_sigs {
        if pos + 4 > bytes.len() {
            return Err("Truncated hybrid sig".into());
        }

        let scheme = match bytes[pos] {
            1 => SchemeId::SQIsign,
            2 => SchemeId::Dilithium5,
            3 => SchemeId::Ed25519,
            _ => return Err(format!("Unknown scheme id: {}", bytes[pos])),
        };
        pos += 1;

        let sig_len = u16::from_le_bytes([bytes[pos], bytes[pos + 1]]) as usize;
        pos += 2;

        if pos + sig_len + 2 > bytes.len() {
            return Err("Truncated signature".into());
        }
        let signature = bytes[pos..pos + sig_len].to_vec();
        pos += sig_len;

        let pk_len = u16::from_le_bytes([bytes[pos], bytes[pos + 1]]) as usize;
        pos += 2;

        if pos + pk_len > bytes.len() {
            return Err("Truncated public key".into());
        }
        let public_key = bytes[pos..pos + pk_len].to_vec();
        pos += pk_len;

        signatures.push(SchemeSignature {
            scheme,
            public_key,
            signature,
            verified: false,
        });
    }

    Ok(HybridSignature { version, signatures })
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hybrid_sqisign_keygen_sign_verify() {
        let schemes = vec![SchemeId::SQIsign];
        let keys = hybrid_keygen(&schemes).unwrap();
        let hybrid = hybrid_sign(b"test message", &keys, &schemes).unwrap();

        assert_eq!(hybrid.version, 1);
        assert_eq!(hybrid.signatures.len(), 1);
        assert_eq!(hybrid.signatures[0].scheme, SchemeId::SQIsign);

        let result = hybrid_verify(b"test message", &hybrid, &keys).unwrap();
        assert!(result.all_valid);
        assert!(result.failed_schemes.is_empty());
    }

    #[test]
    fn test_hybrid_tamper_detection() {
        let schemes = vec![SchemeId::SQIsign];
        let keys = hybrid_keygen(&schemes).unwrap();
        let hybrid = hybrid_sign(b"original", &keys, &schemes).unwrap();

        let result = hybrid_verify(b"TAMPERED", &hybrid, &keys).unwrap();
        assert!(!result.all_valid);
        assert!(result.failed_schemes.contains(&SchemeId::SQIsign));
    }

    #[test]
    fn test_serialization_roundtrip() {
        let schemes = vec![SchemeId::SQIsign];
        let keys = hybrid_keygen(&schemes).unwrap();
        let hybrid = hybrid_sign(b"test", &keys, &schemes).unwrap();

        let bytes = serialize_hybrid(&hybrid);
        let deserialized = deserialize_hybrid(&bytes).unwrap();

        assert_eq!(deserialized.version, hybrid.version);
        assert_eq!(deserialized.signatures.len(), hybrid.signatures.len());
        assert_eq!(deserialized.signatures[0].scheme, hybrid.signatures[0].scheme);
        assert_eq!(deserialized.signatures[0].signature, hybrid.signatures[0].signature);
    }

    #[test]
    fn test_fallback_analysis_sqi_only() {
        let result = HybridResult {
            all_valid: true,
            results: vec![],
            passed_schemes: vec![SchemeId::SQIsign],
            failed_schemes: vec![],
            total_size: 0,
        };
        assert!(matches!(analyze_fallback(&result), FallbackAction::UsePrimary));
    }

    #[test]
    fn test_fallback_analysis_sqi_failed_dil_ok() {
        let result = HybridResult {
            all_valid: false,
            results: vec![],
            passed_schemes: vec![SchemeId::Dilithium5],
            failed_schemes: vec![SchemeId::SQIsign],
            total_size: 0,
        };
        assert!(matches!(analyze_fallback(&result), FallbackAction::FallbackToDilithium5));
    }

    #[test]
    fn test_fallback_analysis_all_failed() {
        let result = HybridResult {
            all_valid: false,
            results: vec![],
            passed_schemes: vec![],
            failed_schemes: vec![SchemeId::SQIsign, SchemeId::Dilithium5, SchemeId::Ed25519],
            total_size: 0,
        };
        assert!(matches!(analyze_fallback(&result), FallbackAction::AllFailed));
    }
}
