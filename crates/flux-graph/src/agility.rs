// agility.rs — Crypto & dependency agility engine.
//
// Phase 2a: Audit which crates depend on which crypto primitives.
// Phase 2b: Plan migrations (sha2→sha3, ed25519→dilithium5, etc.)
// Phase 3:  Auto-apply migrations with code rewriting.
//
// Agility Score: measures how easily the workspace can absorb a
// cryptographic primitive swap without cascading breakage.
// Higher = fewer hard-coded crypto deps, more trait abstractions.

use crate::WorkspaceGraph;

/// Known cryptographic primitives we track for agility.
#[derive(Debug, Clone, PartialEq)]
pub enum CryptoPrimitive {
    HashSha2,
    HashSha3,
    HashBlake3,
    SigEd25519,
    SigDilithium5,
    SigSQIsign,
    KemX25519,
    KemKyber,
    Aes,
    ChaCha20,
    Other(String),
}

impl CryptoPrimitive {
    /// Detect which crypto primitive a dependency name refers to.
    pub fn from_dep_name(name: &str) -> Option<Self> {
        let lower = name.to_lowercase();
        match lower.as_str() {
            "sha2" => Some(CryptoPrimitive::HashSha2),
            "sha3" | "sha3-tiny" | "tiny-keccak" => Some(CryptoPrimitive::HashSha3),
            "blake3" => Some(CryptoPrimitive::HashBlake3),
            "ed25519" | "ed25519-dalek" => Some(CryptoPrimitive::SigEd25519),
            "dilithium" | "pqcrypto-dilithium" | "dilithium5" => Some(CryptoPrimitive::SigDilithium5),
            "sqisign" | "sqisign-rs" | "sqisignhd" => Some(CryptoPrimitive::SigSQIsign),
            "x25519" | "x25519-dalek" | "curve25519-dalek" => Some(CryptoPrimitive::KemX25519),
            "kyber" | "pqcrypto-kyber" => Some(CryptoPrimitive::KemKyber),
            "aes" | "aes-gcm" | "aes-gcm-siv" => Some(CryptoPrimitive::Aes),
            "chacha20" | "chacha20poly1305" => Some(CryptoPrimitive::ChaCha20),
            _ => None,
        }
    }

    /// Is this primitive quantum-resistant?
    pub fn is_post_quantum(&self) -> bool {
        matches!(self,
            CryptoPrimitive::HashSha3 |
            CryptoPrimitive::HashBlake3 |
            CryptoPrimitive::SigDilithium5 |
            CryptoPrimitive::SigSQIsign |
            CryptoPrimitive::KemKyber
        )
    }

    /// Recommended post-quantum replacement, if applicable.
    pub fn pq_replacement(&self) -> Option<&'static str> {
        match self {
            CryptoPrimitive::HashSha2 => Some("sha3 (SHA-3 / Keccak)"),
            CryptoPrimitive::SigEd25519 => Some("dilithium5 (NIST PQC Level 5)"),
            CryptoPrimitive::KemX25519 => Some("kyber (NIST PQC KEM)"),
            CryptoPrimitive::Aes => Some("aes-256 (AES-256 is already PQ at 256-bit)"),
            _ => None, // already PQ or not applicable
        }
    }
}

/// Result of an agility audit: which crates use which crypto.
#[derive(Debug, Clone)]
pub struct AgilityAudit {
    /// Map from crate index to list of crypto primitives it depends on.
    pub crate_crypto: Vec<Vec<CryptoPrimitive>>,
    /// Total agility score (0.0 = all hardcoded, 1.0 = all abstracted).
    pub agility_score: f64,
    /// Crates that need migration (non-PQ crypto detected).
    pub migration_needed: Vec<MigrationTarget>,
    /// Number of crates using post-quantum primitives.
    pub pq_crates: usize,
    /// Number of crates using classical primitives.
    pub classical_crates: usize,
}

/// A crate that needs crypto migration.
#[derive(Debug, Clone)]
pub struct MigrationTarget {
    pub crate_index: usize,
    pub crate_name: String,
    pub current: CryptoPrimitive,
    pub recommended: String,
}

/// Audit the entire workspace for crypto agility.
pub fn audit_agility(ws: &WorkspaceGraph) -> AgilityAudit {
    let n = ws.crates.len();
    let mut crate_crypto: Vec<Vec<CryptoPrimitive>> = vec![Vec::new(); n];
    let mut migration_needed = Vec::new();
    let mut pq_count = 0;
    let mut classical_count = 0;

    for (i, ci) in ws.crates.iter().enumerate() {
        for dep in &ci.dependencies {
            if let Some(primitive) = CryptoPrimitive::from_dep_name(&dep.name) {
                crate_crypto[i].push(primitive.clone());

                if !primitive.is_post_quantum() {
                    if let Some(replacement) = primitive.pq_replacement() {
                        migration_needed.push(MigrationTarget {
                            crate_index: i,
                            crate_name: ci.name.clone(),
                            current: primitive,
                            recommended: replacement.to_string(),
                        });
                    }
                }
            }
        }

        // Count PQ vs classical
        let has_classical = crate_crypto[i].iter().any(|p| !p.is_post_quantum());
        let has_pq = crate_crypto[i].iter().any(|p| p.is_post_quantum());

        if has_pq { pq_count += 1; }
        if has_classical { classical_count += 1; }
    }

    // Agility score: ratio of crates with NO classical crypto to total crates with crypto
    let crates_with_crypto: usize = crate_crypto.iter().filter(|v| !v.is_empty()).count();
    let agility_score = if crates_with_crypto > 0 {
        let clean = crates_with_crypto.saturating_sub(classical_count);
        clean as f64 / crates_with_crypto as f64
    } else {
        1.0 // no crypto = perfectly agile (nothing to migrate)
    };

    AgilityAudit {
        crate_crypto,
        agility_score,
        migration_needed,
        pq_crates: pq_count,
        classical_crates: classical_count,
    }
}

/// Transitively find all crates that depend on a given crate (directly or indirectly).
pub fn transitive_dependents(ws: &WorkspaceGraph, target_name: &str) -> Vec<usize> {
    // Find the target crate index
    let target_idx = match ws.crates.iter().position(|ci| ci.name == target_name) {
        Some(idx) => idx,
        None => return Vec::new(),
    };

    // BFS from target through depended_by edges
    let dag = match crate::graph::build_dag(&ws.crates) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };

    let mut visited = vec![false; ws.crates.len()];
    let mut queue = std::collections::VecDeque::new();
    let mut result = Vec::new();

    queue.push_back(target_idx);
    visited[target_idx] = true;

    while let Some(current) = queue.pop_front() {
        if current != target_idx {
            result.push(current);
        }
        for &dependent in &dag.depended_by[current] {
            if !visited[dependent] {
                visited[dependent] = true;
                queue.push_back(dependent);
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CrateInfo, CrateType, Dependency, DepKind};
    use std::path::PathBuf;

    fn make_test_ws() -> WorkspaceGraph {
        let crates = vec![
            CrateInfo {
                name: "flux-cache".into(),
                path: PathBuf::from("/ws/crates/flux-cache"),
                edition: "2021".into(),
                crate_type: CrateType::Lib,
                dependencies: vec![
                    Dependency { name: "sha2".into(), path: None, kind: DepKind::CratesIo, optional: false },
                    Dependency { name: "blake3".into(), path: None, kind: DepKind::CratesIo, optional: false },
                ],
                features: vec![],
            },
            CrateInfo {
                name: "flux-zk".into(),
                path: PathBuf::from("/ws/crates/flux-zk"),
                edition: "2021".into(),
                crate_type: CrateType::Lib,
                dependencies: vec![
                    Dependency { name: "pqcrypto-dilithium".into(), path: None, kind: DepKind::CratesIo, optional: false },
                ],
                features: vec![],
            },
        ];
        WorkspaceGraph {
            root: PathBuf::from("/ws"),
            crates,
            batches: vec![vec![0, 1]],
        }
    }

    #[test]
    fn test_audit_detects_classical_crypto() {
        let ws = make_test_ws();
        let audit = audit_agility(&ws);
        // flux-cache uses sha2 (classical)
        assert!(audit.classical_crates > 0);
        // flux-zk uses dilithium (PQ)
        assert!(audit.pq_crates > 0);
    }

    #[test]
    fn test_crypto_detection() {
        assert_eq!(
            CryptoPrimitive::from_dep_name("sha2"),
            Some(CryptoPrimitive::HashSha2)
        );
        assert_eq!(
            CryptoPrimitive::from_dep_name("dilithium5"),
            Some(CryptoPrimitive::SigDilithium5)
        );
        assert!(CryptoPrimitive::HashSha3.is_post_quantum());
        assert!(!CryptoPrimitive::HashSha2.is_post_quantum());
    }

    #[test]
    fn test_pq_replacement() {
        assert_eq!(
            CryptoPrimitive::HashSha2.pq_replacement(),
            Some("sha3 (SHA-3 / Keccak)")
        );
        assert_eq!(
            CryptoPrimitive::SigEd25519.pq_replacement(),
            Some("dilithium5 (NIST PQC Level 5)")
        );
        // Already PQ — no replacement needed
        assert_eq!(CryptoPrimitive::HashSha3.pq_replacement(), None);
    }
}

// ── Migration Engine ──

/// A specific code change needed to migrate a crypto primitive.
#[derive(Debug, Clone)]
#[repr(C, align(64))]
pub struct MigrationChange {
    pub crate_name: String,
    pub file_path: String,
    pub line: usize,
    pub old_text: String,
    pub new_text: String,
    pub reason: String,
}

/// Generate migration changes for a crate's dependency.
/// Maps known crypto primitives to their PQ replacements.
pub fn generate_migration(crate_name: &str, dep_name: &str, files: &[(String, String)]) -> Vec<MigrationChange> {
    let mut changes = Vec::new();

    // Cargo.toml dependency rewrite
    let toml_change = match dep_name {
        "sha2" => MigrationChange {
            crate_name: crate_name.to_string(),
            file_path: format!("crates/{}/Cargo.toml", crate_name),
            line: 0,
            old_text: format!("{} = \"{}\"", dep_name, guess_version(crate_name, dep_name)),
            new_text: "sha3 = \"0.10\"".to_string(),
            reason: "sha2 → sha3: SHA-3 is post-quantum resistant (Keccak sponge construction)".into(),
        },
        "ed25519-dalek" | "ring" => MigrationChange {
            crate_name: crate_name.to_string(),
            file_path: format!("crates/{}/Cargo.toml", crate_name),
            line: 0,
            old_text: format!("{} = \"{}\"", dep_name, guess_version(crate_name, dep_name)),
            new_text: "pqcrypto-dilithium = \"0.5\"".to_string(),
            reason: "ed25519 → dilithium5: NIST PQC Level 5, lattice-based".into(),
        },
        _ => return changes,
    };
    changes.push(toml_change);

    // Source file imports
    for (file_path, content) in files {
        let source_changes = generate_source_mutations(crate_name, file_path, content, dep_name);
        changes.extend(source_changes);
    }

    changes
}

/// Generate source-level mutations for a crypto dependency migration.
fn generate_source_mutations(crate_name: &str, file_path: &str, content: &str, dep: &str) -> Vec<MigrationChange> {
    let mut changes = Vec::new();
    let lines: Vec<&str> = content.lines().collect();

    for (i, line) in lines.iter().enumerate() {
        let line_num = i + 1;

        match dep {
            "sha2" => {
                if line.contains("use sha2::") {
                    changes.push(MigrationChange {
                        crate_name: crate_name.to_string(),
                        file_path: file_path.to_string(),
                        line: line_num,
                        old_text: line.to_string(),
                        new_text: line.replace("sha2::", "sha3::").replace("Sha256", "Sha3_256"),
                        reason: "Import rewrite: sha2 → sha3".into(),
                    });
                }
                if line.contains("Sha256::new()") {
                    changes.push(MigrationChange {
                        crate_name: crate_name.to_string(),
                        file_path: file_path.to_string(),
                        line: line_num,
                        old_text: line.to_string(),
                        new_text: line.replace("Sha256", "Sha3_256"),
                        reason: "Hash function: SHA-256 → SHA3-256".into(),
                    });
                }
            }
            "ed25519-dalek" => {
                if line.contains("ed25519_dalek") {
                    changes.push(MigrationChange {
                        crate_name: crate_name.to_string(),
                        file_path: file_path.to_string(),
                        line: line_num,
                        old_text: line.to_string(),
                        new_text: line.replace("ed25519_dalek::VerifyingKey", "pqcrypto_dilithium::dilithium5::PublicKey")
                            .replace("ed25519_dalek::SigningKey", "pqcrypto_dilithium::dilithium5::SecretKey")
                            .replace("ed25519_dalek::Keypair", "pqcrypto_dilithium::dilithium5::Keypair")
                            .replace("ed25519_dalek::Signature", "pqcrypto_dilithium::dilithium5::SignedMessage"),
                        reason: "Signature scheme: ed25519 → dilithium5 (NIST PQC Level 5)".into(),
                    });
                }
            }
            "ring" => {
                if line.contains("ring::") {
                    changes.push(MigrationChange {
                        crate_name: crate_name.to_string(),
                        file_path: file_path.to_string(),
                        line: line_num,
                        old_text: line.to_string(),
                        new_text: format!("// TODO: migrate ring:: to pqcrypto-dilithium: {}", line),
                        reason: "ring → pqcrypto-dilithium: need manual audit for this import".into(),
                    });
                }
            }
            _ => {}
        }
    }

    changes
}

fn guess_version(_crate_name: &str, dep_name: &str) -> String {
    match dep_name {
        "sha2" => "0.10".into(),
        "ed25519-dalek" => "2".into(),
        "ring" => "0.17".into(),
        _ => "\"".into(),
    }
}

/// Run a full migration audit: what changes would be needed?
pub fn audit_migration(ws: &crate::WorkspaceGraph) -> Vec<MigrationChange> {
    let audit = audit_agility(ws);
    let mut all_changes = Vec::new();

    for migration in &audit.migration_needed {
        let dep_name = match migration.current {
            CryptoPrimitive::HashSha2 => "sha2",
            CryptoPrimitive::SigEd25519 => "ed25519-dalek",
            _ => continue,
        };

        // Read source files for this crate
        if let Some(ci) = ws.crates.iter().find(|c| c.name == migration.crate_name) {
            let mut files = Vec::new();
            let src_dir = ci.path.join("src");
            if let Ok(entries) = std::fs::read_dir(&src_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().map_or(false, |e| e == "rs") {
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            let rel = path.strip_prefix(&ws.root).unwrap_or(&path);
                            files.push((rel.to_string_lossy().to_string(), content));
                        }
                    }
                }
            }
            let changes = generate_migration(&migration.crate_name, dep_name, &files);
            all_changes.extend(changes);
        }
    }

    all_changes
}

#[cfg(test)]
mod migration_tests {
    use super::*;

    #[test]
    fn test_generate_sha2_migration() {
        let files = vec![(
            "src/lib.rs".to_string(),
            "use sha2::Sha256;\nlet mut h = Sha256::new();".to_string(),
        )];
        let changes = generate_migration("test-crate", "sha2", &files);
        assert!(!changes.is_empty());
        assert!(changes.iter().any(|c| c.old_text.contains("sha2")));
    }

    #[test]
    fn test_migration_reason_contains_pq() {
        let files = vec![(
            "src/lib.rs".to_string(),
            "use ed25519_dalek::Keypair;".to_string(),
        )];
        let changes = generate_migration("test-crate", "ed25519-dalek", &files);
        assert!(changes.iter().any(|c| c.reason.contains("NIST PQC")));
    }
}
