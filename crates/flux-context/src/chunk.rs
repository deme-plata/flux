//! chunk — semantic chunking + dependency-ripple scoring over the Flux workspace.
//!
//! Task 1 of `docs/1M_CONTEXT_WINDOW_PLAN.md`. Built ON `flux-graph` (which already
//! resolves crates + path-deps + topological batches) — no new manifest parser.
//!
//! The **ripple score** answers "if this crate changes, how much downstream is
//! impacted?" so the 1M window can be packed highest-impact-first:
//!
//! ```text
//! ripple_raw(j) = ( Σ_{downstream k} 1/dist(j,k) ) × (downstream_count / total_crates)
//! ripple_score  = ripple_raw / max(ripple_raw)        // normalized 0..1
//! ```
//!
//! where `downstream` = the reverse-dependency closure of crate `j` (crates that
//! transitively depend on it), and `dist` = hops in the dep graph.

use crate::est_tokens;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::Path;

/// Coarse semantic category, derived from the crate name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChunkCategory {
    Core,
    P2p,
    Consensus,
    Crypto,
    Mcp,
    Sigil,
    Frontend,
    Tool,
    Other,
}

/// Classify a crate into a semantic category by name (cheap, order-sensitive).
pub fn classify(name: &str) -> ChunkCategory {
    let n = name.to_lowercase();
    let has = |k: &str| n.contains(k);
    if matches!(name, "fluxc" | "fluxc-core" | "flux-frontend" | "flux-backend" | "flux-graph") {
        ChunkCategory::Core
    } else if has("mcp") {
        ChunkCategory::Mcp
    } else if has("sigil") {
        ChunkCategory::Sigil
    } else if has("sqisign") || has("blake") || has("zk") || has("lattice") || has("vrf")
        || has("vdf") || has("crypto") || has("cypher") || has("recursi")
    {
        ChunkCategory::Crypto
    } else if has("p2p") || has("swarm") || has("network") || n == "flux-net" || has("-net") {
        ChunkCategory::P2p
    } else if has("dagknight") || has("narwhal") || has("consensus") || has("sap") || has("xalgo")
        || has("x-algo")
    {
        ChunkCategory::Consensus
    } else if has("wallet") || has("vite") || has("frontend") || has("desktop") || has("ui") {
        ChunkCategory::Frontend
    } else if has("tool") || has("example") || has("bench") {
        ChunkCategory::Tool
    } else {
        ChunkCategory::Other
    }
}

/// One crate-granularity chunk of the workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticChunk {
    /// Crate dir, relative to the workspace root.
    pub path: String,
    pub crate_name: String,
    pub category: ChunkCategory,
    /// Normalized 0..1 — higher = more downstream impact.
    pub ripple_score: f64,
    /// Estimated tokens (Σ est_tokens over the crate's .rs files).
    pub estimated_tokens: u64,
    /// BLAKE3 (via flux-rev) over the crate's sorted .rs sources — content fingerprint
    /// for the context-diff (Task 2). Changes iff source content changes.
    pub blake3_hex: String,
    /// Newest .rs mtime in the crate (ns since epoch) — fast pre-filter for diffs.
    pub mtime_ns: u64,
    /// Workspace crates this one depends on (path-deps).
    pub deps: Vec<String>,
    /// Workspace crates that depend on this one.
    pub rev_deps: Vec<String>,
}

/// The full chunk manifest (sorted by ripple DESC), written to `.whale/context/chunks.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkManifest {
    pub version: u32,
    pub workspace: String,
    pub crate_count: usize,
    pub total_tokens_estimated: u64,
    pub chunks: Vec<SemanticChunk>,
}

/// Scan a crate dir (recursive; skips target/.git/node_modules): returns
/// (Σ est_tokens, BLAKE3 over sorted .rs sources, newest mtime ns). The hash is
/// computed via flux-rev so the content fingerprint matches the rest of the stack.
fn crate_scan(dir: &Path) -> (u64, String, u64) {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut tokens = 0u64;
    let mut max_mtime = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                let skip = p
                    .file_name()
                    .map(|n| n == "target" || n == ".git" || n == "node_modules")
                    .unwrap_or(false);
                if !skip {
                    stack.push(p);
                }
            } else if p.extension().map(|x| x == "rs").unwrap_or(false) {
                let Ok(bytes) = std::fs::read(&p) else { continue };
                if let Ok(s) = std::str::from_utf8(&bytes) {
                    tokens += est_tokens(s) as u64;
                }
                if let Ok(m) = entry.metadata().and_then(|md| md.modified()) {
                    if let Ok(dur) = m.duration_since(std::time::UNIX_EPOCH) {
                        max_mtime = max_mtime.max(dur.as_nanos() as u64);
                    }
                }
                let rel = p.strip_prefix(dir).unwrap_or(&p).to_string_lossy().to_string();
                files.push((rel, bytes));
            }
        }
    }
    // Stable order → stable hash regardless of fs walk order.
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut buf = Vec::new();
    for (rel, bytes) in &files {
        buf.extend_from_slice(rel.as_bytes());
        buf.push(0);
        buf.extend_from_slice(bytes);
        buf.push(0);
    }
    (tokens, flux_rev::hash_bytes(&buf), max_mtime)
}

/// Compute the chunk manifest for a workspace root, using flux-graph for the dep DAG.
pub fn compute_manifest(root: &Path) -> Result<ChunkManifest, String> {
    let rootbuf = root.to_path_buf();
    let ws = flux_graph::resolve_workspace(&rootbuf)?;
    let n = ws.crates.len();
    let idx: HashMap<&str, usize> =
        ws.crates.iter().enumerate().map(|(i, c)| (c.name.as_str(), i)).collect();

    // forward deps[i] = crates i depends on (path-deps that resolve to a ws crate);
    // rev[j] = crates that depend on j.
    let mut deps: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut rev: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, c) in ws.crates.iter().enumerate() {
        for d in &c.dependencies {
            if d.path.is_some() {
                if let Some(&j) = idx.get(d.name.as_str()) {
                    if j != i && !deps[i].contains(&j) {
                        deps[i].push(j);
                        rev[j].push(i);
                    }
                }
            }
        }
    }

    // ripple_raw via reverse-dep BFS.
    let mut raw = vec![0f64; n];
    for j in 0..n {
        let mut visited = vec![false; n];
        visited[j] = true;
        let mut q: VecDeque<(usize, usize)> = VecDeque::new();
        for &k in &rev[j] {
            if !visited[k] {
                visited[k] = true;
                q.push_back((k, 1));
            }
        }
        let (mut sum_inv, mut count) = (0f64, 0usize);
        while let Some((node, dist)) = q.pop_front() {
            sum_inv += 1.0 / dist as f64;
            count += 1;
            for &k in &rev[node] {
                if !visited[k] {
                    visited[k] = true;
                    q.push_back((k, dist + 1));
                }
            }
        }
        let impact = if n > 0 { count as f64 / n as f64 } else { 0.0 };
        raw[j] = sum_inv * impact;
    }
    let maxraw = raw.iter().cloned().fold(0f64, f64::max);

    let mut chunks: Vec<SemanticChunk> = Vec::with_capacity(n);
    let mut total_tokens = 0u64;
    for (i, c) in ws.crates.iter().enumerate() {
        let (toks, blake3_hex, mtime_ns) = crate_scan(&c.path);
        total_tokens += toks;
        let rel = c.path.strip_prefix(&rootbuf).unwrap_or(&c.path).to_string_lossy().to_string();
        chunks.push(SemanticChunk {
            path: rel,
            crate_name: c.name.clone(),
            category: classify(&c.name),
            ripple_score: if maxraw > 0.0 { raw[i] / maxraw } else { 0.0 },
            estimated_tokens: toks,
            blake3_hex,
            mtime_ns,
            deps: deps[i].iter().map(|&j| ws.crates[j].name.clone()).collect(),
            rev_deps: rev[i].iter().map(|&j| ws.crates[j].name.clone()).collect(),
        });
    }
    chunks.sort_by(|a, b| {
        b.ripple_score
            .partial_cmp(&a.ripple_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(ChunkManifest {
        version: 1,
        workspace: rootbuf.to_string_lossy().to_string(),
        crate_count: n,
        total_tokens_estimated: total_tokens,
        chunks,
    })
}

/// Pack the highest-ripple chunks that fit within `budget_tokens` (1M-window fill).
pub fn pack_to_budget(manifest: &ChunkManifest, budget_tokens: u64) -> Vec<&SemanticChunk> {
    let mut out = Vec::new();
    let mut used = 0u64;
    for c in &manifest.chunks {
        if used + c.estimated_tokens <= budget_tokens {
            used += c.estimated_tokens;
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn classify_basics() {
        assert_eq!(classify("fluxc-core"), ChunkCategory::Core);
        assert_eq!(classify("flux-p2p"), ChunkCategory::P2p);
        assert_eq!(classify("fluxc-mcp"), ChunkCategory::Mcp);
        assert_eq!(classify("sigil-top"), ChunkCategory::Sigil);
        assert_eq!(classify("flux-sqisign"), ChunkCategory::Crypto);
    }

    fn workspace_root() -> PathBuf {
        let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        loop {
            let is_ws = std::fs::read_to_string(dir.join("Cargo.toml"))
                .map(|t| t.contains("[workspace]"))
                .unwrap_or(false);
            if is_ws {
                return dir;
            }
            dir = dir.parent().expect("no workspace root above flux-context").to_path_buf();
        }
    }

    #[test]
    fn manifest_on_self_workspace() {
        let root = workspace_root();
        let m = compute_manifest(&root).expect("manifest");
        assert!(m.crate_count > 10, "expected many crates, got {}", m.crate_count);
        assert!(m.total_tokens_estimated > 0);
        // sorted DESC → the top chunk carries the normalized max ripple (1.0).
        assert!(
            m.chunks[0].ripple_score >= 0.999,
            "top ripple should normalize to 1.0, got {}",
            m.chunks[0].ripple_score
        );
        // budget packing never exceeds the budget.
        let packed = pack_to_budget(&m, 50_000);
        let used: u64 = packed.iter().map(|c| c.estimated_tokens).sum();
        assert!(used <= 50_000);
    }
}
