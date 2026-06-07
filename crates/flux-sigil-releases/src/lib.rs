//! flux-sigil-releases — append-only ledger for the 100-iteration SIGIL wallet
//! plan. Each release is one line of JSON in a `.jsonl` file served as a static
//! asset; the public site (`sigil-releases.html`) polls it every 5 s and fires
//! Web Notifications when new entries land. No backend process required —
//! the ledger IS the substrate.
//!
//! Layout (FLUXFOOD-compliant):
//!   • Workspace deps only (serde, serde_json, anyhow)
//!   • Build via `fluxc build --package flux-sigil-releases`
//!   • Shared target (no per-crate `target/` override)
//!
//! Phases are A..J, versions 0.1.0 .. 1.0.9 — 100 slots total.

pub mod plan;

use serde::{Deserialize, Serialize};
use std::path::Path;

pub const PHASES: [(char, &str); 10] = [
    ('A', "Bootstrap & Inventory"),
    ('B', "Visual Identity"),
    ('C', "Network Substitution"),
    ('D', "SIGIL Primitives"),
    ('E', "Dev Surface"),
    ('F', "Multi-Agent / agentic-money"),
    ('G', "Privacy + ZK"),
    ('H', "DEX + DeFi"),
    ('I', "Polish + Perf"),
    ('J', "Launch + Post-Launch"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Shipped + verified + (optionally) settled on-chain.
    Shipped,
    /// Being worked on right now (claimed in swarm).
    InFlight,
    /// Not yet started.
    Pending,
    /// Started but abandoned; superseded by a later release in same phase.
    Aborted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Release {
    /// Phase letter — A..J.
    pub phase: char,
    /// `0.X.Y` version. Within Phase A this is 0.1.0–0.1.9, etc.
    pub version: String,
    /// One-line title (`Fork import`, `API URL swap`, …).
    pub title: String,
    pub status: Status,
    /// QUG settled when status==Shipped. None for in-flight/pending.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_qug: Option<f64>,
    /// Unix-ms timestamp of the most recent status change.
    pub ts_ms: u64,
    /// Free-text notes (markdown allowed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Optional verify URL (deployed artefact).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Which agent shipped it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

impl Release {
    /// Stable id: `A-0.1.0` → unique per slot.
    pub fn slot_id(&self) -> String {
        format!("{}-{}", self.phase, self.version)
    }

    /// True if this release belongs to a phase + slot within the 100-grid.
    pub fn is_canonical(&self) -> bool {
        if !PHASES.iter().any(|(p, _)| *p == self.phase) {
            return false;
        }
        // version must parse as 0.X.Y where X = phase index+1, Y in 0..=9
        let parts: Vec<&str> = self.version.split('.').collect();
        if parts.len() != 3 {
            return false;
        }
        let major: u32 = match parts[0].parse() {
            Ok(n) => n,
            Err(_) => return false,
        };
        let phase_idx = PHASES.iter().position(|(p, _)| *p == self.phase).unwrap();
        // phase A = 0.1.x, B = 0.2.x, ..., J = 1.0.x
        let expected_major = if phase_idx < 9 { 0 } else { 1 };
        let expected_minor = if phase_idx < 9 { (phase_idx + 1) as u32 } else { 0 };
        let minor: u32 = parts[1].parse().unwrap_or(99);
        let patch: u32 = parts[2].parse().unwrap_or(99);
        major == expected_major && minor == expected_minor && patch <= 9
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Read all releases from a `.jsonl` file (one Release per line, ignoring blank lines).
pub fn read_jsonl(path: &Path) -> anyhow::Result<Vec<Release>> {
    if !path.exists() {
        return Ok(vec![]);
    }
    let txt = std::fs::read_to_string(path)?;
    let mut out = Vec::new();
    for (i, line) in txt.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match serde_json::from_str::<Release>(line) {
            Ok(r) => out.push(r),
            Err(e) => {
                anyhow::bail!("line {}: {}", i + 1, e);
            }
        }
    }
    Ok(out)
}

/// Append one release as a JSON line.
pub fn append_jsonl(path: &Path, r: &Release) -> anyhow::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let line = serde_json::to_string(r)?;
    writeln!(f, "{}", line)?;
    Ok(())
}

/// The 100-slot grid. Slots are returned in phase × version order. Status is
/// derived from the most-recent matching Release; if none, `Pending`.
#[derive(Debug, Clone, Serialize)]
pub struct Slot {
    pub phase: char,
    pub phase_name: String,
    pub version: String,
    pub status: Status,
    pub latest: Option<Release>,
}

pub fn build_grid(history: &[Release]) -> Vec<Slot> {
    let mut grid = Vec::with_capacity(100);
    for (i, (phase, name)) in PHASES.iter().enumerate() {
        for patch in 0..10u32 {
            let (maj, min) = if i < 9 { (0u32, (i + 1) as u32) } else { (1u32, 0u32) };
            let version = format!("{}.{}.{}", maj, min, patch);
            let slot_id = format!("{}-{}", phase, version);

            // Find latest matching Release for this slot.
            let mut latest: Option<&Release> = None;
            for r in history {
                if r.slot_id() == slot_id {
                    if latest.map(|l| r.ts_ms > l.ts_ms).unwrap_or(true) {
                        latest = Some(r);
                    }
                }
            }
            let status = latest.map(|r| r.status).unwrap_or(Status::Pending);
            grid.push(Slot {
                phase: *phase,
                phase_name: name.to_string(),
                version,
                status,
                latest: latest.cloned(),
            });
        }
    }
    grid
}

/// Compact stats for a one-line summary in the page header.
#[derive(Debug, Clone, Serialize)]
pub struct GridStats {
    pub shipped: u32,
    pub in_flight: u32,
    pub pending: u32,
    pub aborted: u32,
    pub total_qug_settled: f64,
}

pub fn grid_stats(grid: &[Slot]) -> GridStats {
    let mut g = GridStats {
        shipped: 0,
        in_flight: 0,
        pending: 0,
        aborted: 0,
        total_qug_settled: 0.0,
    };
    for s in grid {
        match s.status {
            Status::Shipped => g.shipped += 1,
            Status::InFlight => g.in_flight += 1,
            Status::Pending => g.pending += 1,
            Status::Aborted => g.aborted += 1,
        }
        if let Some(r) = &s.latest {
            if r.status == Status::Shipped {
                if let Some(q) = r.settled_qug {
                    g.total_qug_settled += q;
                }
            }
        }
    }
    g
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn release(phase: char, version: &str, status: Status, ts: u64) -> Release {
        Release {
            phase,
            version: version.into(),
            title: "test".into(),
            status,
            settled_qug: Some(0.5),
            ts_ms: ts,
            notes: None,
            url: None,
            agent: Some("rocky-sigil".into()),
        }
    }

    #[test]
    fn is_canonical_phase_a() {
        assert!(release('A', "0.1.0", Status::Shipped, 0).is_canonical());
        assert!(release('A', "0.1.9", Status::Shipped, 0).is_canonical());
        assert!(!release('A', "0.2.0", Status::Shipped, 0).is_canonical());
        assert!(!release('A', "0.1.10", Status::Shipped, 0).is_canonical());
    }

    #[test]
    fn is_canonical_phase_j_is_1_0_x() {
        assert!(release('J', "1.0.0", Status::Shipped, 0).is_canonical());
        assert!(release('J', "1.0.9", Status::Shipped, 0).is_canonical());
        assert!(!release('J', "0.10.0", Status::Shipped, 0).is_canonical());
    }

    #[test]
    fn grid_is_100_slots() {
        let g = build_grid(&[]);
        assert_eq!(g.len(), 100);
        // First slot = A 0.1.0
        assert_eq!(g[0].phase, 'A');
        assert_eq!(g[0].version, "0.1.0");
        // Last slot = J 1.0.9
        assert_eq!(g[99].phase, 'J');
        assert_eq!(g[99].version, "1.0.9");
    }

    #[test]
    fn build_grid_picks_latest_release_per_slot() {
        let hist = vec![
            release('A', "0.1.0", Status::InFlight, 1000),
            release('A', "0.1.0", Status::Shipped, 2000),
        ];
        let g = build_grid(&hist);
        assert_eq!(g[0].status, Status::Shipped);
        assert_eq!(g[0].latest.as_ref().unwrap().ts_ms, 2000);
    }

    #[test]
    fn grid_stats_counts_correctly() {
        let hist = vec![
            release('A', "0.1.0", Status::Shipped, 1),
            release('A', "0.1.1", Status::Shipped, 1),
            release('B', "0.2.0", Status::InFlight, 1),
        ];
        let g = build_grid(&hist);
        let stats = grid_stats(&g);
        assert_eq!(stats.shipped, 2);
        assert_eq!(stats.in_flight, 1);
        assert_eq!(stats.pending, 97);
        assert!((stats.total_qug_settled - 1.0).abs() < 1e-9);
    }

    #[test]
    fn jsonl_append_then_read_roundtrips() {
        let dir = tempdir_unique();
        let path = dir.join("releases.jsonl");
        append_jsonl(&path, &release('A', "0.1.0", Status::Shipped, 1)).unwrap();
        append_jsonl(&path, &release('A', "0.1.1", Status::Shipped, 2)).unwrap();
        let back = read_jsonl(&path).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].version, "0.1.0");
        assert_eq!(back[1].version, "0.1.1");
    }

    #[test]
    fn jsonl_skips_blanks_and_comments() {
        let dir = tempdir_unique();
        let path = dir.join("releases.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "# header comment").unwrap();
        writeln!(f).unwrap();
        let r = release('A', "0.1.5", Status::Shipped, 9);
        writeln!(f, "{}", serde_json::to_string(&r).unwrap()).unwrap();
        let back = read_jsonl(&path).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].version, "0.1.5");
    }

    fn tempdir_unique() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!(
            "flux-sigil-releases-test-{}-{}-{}",
            std::process::id(),
            now_ms(),
            n,
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}
