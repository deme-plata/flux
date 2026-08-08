//! flux-rev bridge — snapshot a tree and return its content-address.
//!
//! Shared by the CLI (`flux-buzz rev-post`) and the relay's fluxc build-hook
//! (auto-provenance on green combos). The binary is resolved via FLUX_REV_BIN,
//! then PATH, then the flux-tree debug path.

use anyhow::{bail, Result};
use std::path::Path;

pub fn flux_rev_snapshot(dir: &Path) -> Result<String> {
    let candidates = [
        std::env::var("FLUX_REV_BIN").unwrap_or_default(),
        "flux-rev".to_string(),
        "/home/storage/deepseek-codewhale/flux/target/debug/flux-rev".to_string(),
    ];
    for bin in candidates.iter().filter(|b| !b.is_empty()) {
        let out = match std::process::Command::new(bin).arg("snapshot").arg(dir).output() {
            Ok(o) => o,
            Err(_) => continue, // binary not found under this name — try next
        };
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        if !out.status.success() {
            bail!("flux-rev snapshot failed for {}: {}", dir.display(), text.trim());
        }
        if let Some(stamp) = parse_rev_stamp(&text) {
            return Ok(stamp);
        }
        // Snapshot printed no stamp (shouldn't happen) — fall back to `head`.
        let head = std::process::Command::new(bin).arg("head").arg(dir).output()?;
        if let Some(stamp) = parse_rev_stamp(&String::from_utf8_lossy(&head.stdout)) {
            return Ok(stamp);
        }
        bail!("could not parse a full: stamp from flux-rev output: {}", text.trim());
    }
    bail!("flux-rev binary not found (set FLUX_REV_BIN)")
}

/// Extract a 64-hex flux-rev full id from CLI output — either a `full: <hex>`
/// line (snapshot/genesis) or a bare hex line (`head`).
pub fn parse_rev_stamp(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        let cand = line.strip_prefix("full:").map(str::trim).unwrap_or(line);
        if cand.len() == 64 && cand.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Some(cand.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::parse_rev_stamp;

    #[test]
    fn parses_snapshot_and_head_output() {
        // Real observed formats (2026-08-08):
        let snapshot = "📦 revision f82dc51b2146283a  ·  parent 4807c97d45fec4a8\n   +0 ~0 -0\n   full: f82dc51b2146283a43aa8ef305a71e0a1ed4f4b2ebc66d5daa266d7b2311c25a\n";
        assert_eq!(
            parse_rev_stamp(snapshot).as_deref(),
            Some("f82dc51b2146283a43aa8ef305a71e0a1ed4f4b2ebc66d5daa266d7b2311c25a")
        );
        let head = "f82dc51b2146283a43aa8ef305a71e0a1ed4f4b2ebc66d5daa266d7b2311c25a\n";
        assert_eq!(parse_rev_stamp(head).as_deref(), parse_rev_stamp(snapshot).as_deref());
        assert_eq!(parse_rev_stamp("✗ no HEAD — run `flux-rev genesis .` first"), None);
        assert_eq!(parse_rev_stamp("📦 revision f82dc51b2146283a"), None);
    }
}
