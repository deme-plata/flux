//! Auto-provenance for green combos — the crate tree stamps ITSELF.
//!
//! On a green `flux_combo`, the builder (not some downstream relay) computes
//! the flux-rev content-address of `crates/<pkg>` and ships it inside the
//! `combo_complete` webhook as `data.rev`. Receivers (flux-buzz #builds, CI,
//! dashboards) then carry a receipt whose verdict and content-address come
//! from the same process that ran the tests.
//!
//! Uses the flux-rev LIBRARY (no subprocess): auto-genesis on the first green
//! combo of a crate, `snapshot_if_changed` afterwards — an unchanged tree
//! reuses HEAD instead of minting a timestamp-churned duplicate revision.

use std::path::Path;

/// Stamp `crates/<pkg>` and return the 64-hex full revision id.
/// Returns None (never errors the combo) when the dir is missing or any
/// flux-rev step fails — a build receipt without a stamp beats no receipt.
pub fn stamp_crate(pkg: &str) -> Option<String> {
    let dir = super::ws().join("crates").join(pkg);
    stamp_dir(&dir)
}

pub fn stamp_dir(dir: &Path) -> Option<String> {
    if !dir.is_dir() {
        return None;
    }
    let store = flux_rev::Store::open(dir).ok()?;
    let author = "fluxc-combo";
    match store.read_head() {
        Some(head) => {
            let r = store.get_revision(&head).ok()?;
            match flux_rev::snapshot_if_changed(
                dir,
                &store,
                &r.genesis,
                &r.workspace_version,
                author,
                "green combo auto-stamp",
            )
            .ok()?
            {
                Some(rev) => Some(rev.id),
                // Tree byte-identical to HEAD — HEAD *is* the stamp.
                None => Some(head),
            }
        }
        None => {
            let version = fluxc_core::version::VersionInfo::load(&super::ws())
                .map(|v: fluxc_core::version::VersionInfo| v.display())
                .unwrap_or_else(|_| "0.0.0".into());
            let g = flux_rev::stamp_genesis(
                &store,
                "fluxc-combo",
                &version,
                author,
                "auto-genesis on first green combo",
            )
            .ok()?;
            flux_rev::snapshot(dir, &store, None, &g.id(), &version, author, "genesis import")
                .ok()
                .map(|rev| rev.id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::stamp_dir;

    #[test]
    fn stamps_fresh_tree_then_reuses_head_when_unchanged() {
        let dir = std::env::temp_dir().join(format!("revstamp-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/lib.rs"), "pub fn x() -> u8 { 1 }\n").unwrap();

        let first = stamp_dir(&dir).expect("auto-genesis must stamp a fresh tree");
        assert_eq!(first.len(), 64);
        let again = stamp_dir(&dir).expect("second stamp");
        assert_eq!(first, again, "unchanged tree must reuse HEAD, not mint a new id");

        std::fs::write(dir.join("src/lib.rs"), "pub fn x() -> u8 { 2 }\n").unwrap();
        let changed = stamp_dir(&dir).expect("changed tree");
        assert_ne!(first, changed, "changed tree must get a new id");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_dir_is_none_not_error() {
        assert!(stamp_dir(std::path::Path::new("/nonexistent/xyz")).is_none());
    }
}
