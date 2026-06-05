// resolver.rs — Feature flag resolution.
//
// Phase 2a: minimal — just unions default-features with requested features.
// Phase 2b: full cargo-style feature resolver with unification.

use crate::CrateInfo;

/// Resolve features for a crate given requested features.
/// Returns the union of default features (unless `no_default_features`)
/// and explicitly requested features.
pub fn resolve_features(
    crate_info: &CrateInfo,
    requested: &[String],
    no_default_features: bool,
) -> Vec<String> {
    let mut features: Vec<String> = Vec::new();

    // Default features
    if !no_default_features {
        // Default features are those not marked optional, or explicitly listed
        // For now, include all non-optional features as defaults
        for dep in &crate_info.dependencies {
            if !dep.optional {
                features.push(dep.name.clone());
            }
        }
    }

    // Explicitly requested features
    for req in requested {
        if !features.contains(req) {
            features.push(req.clone());
        }
    }

    features
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CrateInfo, CrateType, Dependency, DepKind};
    use std::path::PathBuf;

    #[test]
    fn test_default_features() {
        let ci = CrateInfo {
            name: "test".into(),
            path: PathBuf::from("test"),
            edition: "2021".into(),
            crate_type: CrateType::Lib,
            dependencies: vec![
                Dependency { name: "a".into(), path: Some(PathBuf::from("a")), kind: DepKind::Path, optional: false },
                Dependency { name: "b".into(), path: Some(PathBuf::from("b")), kind: DepKind::Path, optional: true },
            ],
            features: vec![],
        };
        let features = resolve_features(&ci, &[], false);
        assert!(features.contains(&"a".to_string()));
        assert!(!features.contains(&"b".to_string()));
    }
}
