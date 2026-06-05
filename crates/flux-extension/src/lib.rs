//! flux-extension — the **scaffold-chain** engine.
//!
//! Turns a chain name + a few params into a complete, FLUXFOOD-conformant
//! Flux-native sibling-chain workspace (net identity, content-addressed header,
//! deterministic chronos harness, lightweight tip-verify node). This is the code
//! behind `fluxc scaffold-chain --name <X>` / the `flux_chain_template` MCP tool —
//! the automation the `SIGIL_TEMPLATES.md` doc was the manual path for.
//!
//! Pure std: generation is string substitution over embedded templates modeled on
//! the live `sigil-header` / `sigil-net` / `sigil-chronos` / `sigil-node` crates.
//! The generated workspace is self-contained and compiles standalone.

use std::path::Path;

/// Parameters that fully determine a generated chain. Everything else is derived.
#[derive(Debug, Clone)]
pub struct ChainParams {
    /// Lowercase chain name, e.g. "aurum". ASCII `[a-z0-9-]`.
    pub name: String,
    /// Genesis tag, e.g. "g0".
    pub tag: String,
    /// libp2p port — pick distinct from Quillon :9001 / SIGIL :9501.
    pub p2p_port: u16,
    /// API port — distinct from :8080 / :8181.
    pub api_port: u16,
}

impl ChainParams {
    pub fn new(
        name: impl Into<String>,
        tag: impl Into<String>,
        p2p_port: u16,
        api_port: u16,
    ) -> Result<Self, String> {
        let name = name.into();
        let tag = tag.into();
        if name.is_empty() || tag.is_empty() {
            return Err("name and tag must be non-empty".into());
        }
        if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Err(format!("name '{name}' must be ascii [a-z0-9-]"));
        }
        if !tag.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) {
            return Err(format!("tag '{tag}' must be ascii [a-z0-9]"));
        }
        let p = ChainParams { name, tag, p2p_port, api_port };
        // network_id is a fixed [u8; N] in the header → serde derive needs N ≤ 32.
        if p.network_id().len() > 32 {
            return Err(format!(
                "network_id '{}' is {} bytes (max 32 — shorten name or tag)",
                p.network_id(),
                p.network_id().len()
            ));
        }
        Ok(p)
    }

    pub fn network_id(&self) -> String {
        format!("{}-{}", self.name, self.tag)
    }
    fn name_uscore(&self) -> String {
        self.name.replace('-', "_")
    }
    fn name_upper(&self) -> String {
        self.name_uscore().to_ascii_uppercase()
    }
    fn nid_len(&self) -> usize {
        self.network_id().len()
    }

    /// Apply every placeholder substitution to a template string.
    fn fill(&self, template: &str) -> String {
        template
            .replace("{{NAME_USCORE}}", &self.name_uscore())
            .replace("{{NAME_UPPER}}", &self.name_upper())
            .replace("{{NAME}}", &self.name)
            .replace("{{TAG}}", &self.tag)
            .replace("{{P2P_PORT}}", &self.p2p_port.to_string())
            .replace("{{API_PORT}}", &self.api_port.to_string())
            .replace("{{NID_LEN}}", &self.nid_len().to_string())
    }
}

/// One generated file: workspace-relative path + full content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedFile {
    pub path: String,
    pub content: String,
}

/// The full generated chain workspace.
#[derive(Debug, Clone)]
pub struct GeneratedChain {
    pub name: String,
    pub network_id: String,
    pub files: Vec<GeneratedFile>,
}

impl GeneratedChain {
    /// Write every file under `root`, creating parent dirs. Returns paths written.
    pub fn write_to(&self, root: &Path) -> std::io::Result<Vec<String>> {
        let mut written = Vec::new();
        for f in &self.files {
            let full = root.join(&f.path);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&full, &f.content)?;
            written.push(full.display().to_string());
        }
        Ok(written)
    }

    /// Human summary (paths + verify hints).
    pub fn manifest(&self) -> String {
        let mut out = format!(
            "⬡ scaffolded chain '{}' (network_id={}) — {} files:\n",
            self.name,
            self.network_id,
            self.files.len()
        );
        for f in &self.files {
            out.push_str(&format!("  {}\n", f.path));
        }
        out.push_str(&format!(
            "\nverify:\n  flux_combo {n}-header\n  flux_combo {n}-chronos\n  fluxc build --package {n}-node\n",
            n = self.name
        ));
        out
    }
}

/// Generate the complete chain workspace from params.
pub fn scaffold_chain(p: &ChainParams) -> GeneratedChain {
    let n = &p.name;
    let files = vec![
        GeneratedFile { path: "Cargo.toml".into(), content: p.fill(WORKSPACE_TOML) },
        GeneratedFile { path: "README.md".into(), content: p.fill(README) },
        GeneratedFile { path: format!("crates/{n}-net/Cargo.toml"), content: p.fill(&crate_toml("net", &["serde", "serde_json"])) },
        GeneratedFile { path: format!("crates/{n}-net/src/lib.rs"), content: p.fill(NET_LIB) },
        GeneratedFile { path: format!("crates/{n}-header/Cargo.toml"), content: p.fill(&crate_toml("header", &["serde", "serde_json", "blake3", "thiserror"])) },
        GeneratedFile { path: format!("crates/{n}-header/src/lib.rs"), content: p.fill(HEADER_LIB) },
        GeneratedFile { path: format!("crates/{n}-chronos/Cargo.toml"), content: p.fill(&chronos_toml(n)) },
        GeneratedFile { path: format!("crates/{n}-chronos/src/lib.rs"), content: p.fill(CHRONOS_LIB) },
        GeneratedFile { path: format!("crates/{n}-chronos/tests/chronos.rs"), content: p.fill(CHRONOS_TESTS) },
        GeneratedFile { path: format!("crates/{n}-node/Cargo.toml"), content: p.fill(&node_toml(n)) },
        GeneratedFile { path: format!("crates/{n}-node/src/main.rs"), content: p.fill(NODE_MAIN) },
    ];
    GeneratedChain { name: p.name.clone(), network_id: p.network_id(), files }
}

// ── per-crate Cargo.toml builders ──

fn crate_toml(suffix: &str, deps: &[&str]) -> String {
    let mut s = format!(
        "[package]\nname = \"{{{{NAME}}}}-{suffix}\"\nversion.workspace = true\nedition.workspace = true\nlicense.workspace = true\ndescription = \"{{{{NAME}}}}-{suffix} — generated by flux-extension scaffold-chain.\"\n\n[dependencies]\n"
    );
    for d in deps {
        s.push_str(&format!("{d} = {{ workspace = true }}\n"));
    }
    s
}

fn chronos_toml(_n: &str) -> String {
    let mut s = crate_toml("chronos", &["blake3"]);
    s.push_str("{{NAME}}-header = { workspace = true }\n");
    s
}

fn node_toml(_n: &str) -> String {
    let mut s = crate_toml("node", &["serde_json", "hex"]);
    s.push_str("{{NAME}}-header = { workspace = true }\n");
    s
}

// ── embedded templates (modeled on the live sigil-* crates) ──

const WORKSPACE_TOML: &str = r#"[workspace]
members = ["crates/*"]
resolver = "2"

[workspace.package]
version = "0.0.1"
edition = "2021"
license = "MIT OR Apache-2.0"

# Declare shared deps ONCE; crates pull them via { workspace = true } so cargo
# dedups across the workspace (one build, shared target). FLUXFOOD lever 2.
[workspace.dependencies]
serde      = { version = "1", features = ["derive"] }
serde_json = { version = "1", features = ["arbitrary_precision"] }
blake3     = "1"
anyhow     = "1"
thiserror  = "1"
hex        = "0.4"
{{NAME}}-header  = { path = "crates/{{NAME}}-header" }
{{NAME}}-chronos = { path = "crates/{{NAME}}-chronos" }
"#;

const README: &str = r#"# {{NAME}}

A Flux-native chain, scaffolded by `flux-extension` (the `fluxc scaffold-chain` engine).

| | |
|---|---|
| network_id | `{{NAME}}-{{TAG}}` |
| p2p port | `{{P2P_PORT}}` |
| api port | `{{API_PORT}}` |

## Crates
- `{{NAME}}-net` — on-the-wire identifiers + bootstrap parsing (no hardcoded peers).
- `{{NAME}}-header` — content-addressed block header v0 (state committed in roots).
- `{{NAME}}-chronos` — deterministic chain sim: happy path `blocks_applied=N, divergence=0`, adversarial reject.
- `{{NAME}}-node` — lightweight tip-verify node.

## Verify
```
flux_combo {{NAME}}-header
flux_combo {{NAME}}-chronos
fluxc build --package {{NAME}}-node
```

Build with `fluxc` / `flux_combo`, never raw `cargo` (FLUXFOOD no-cargo rule).
"#;

const NET_LIB: &str = r#"//! {{NAME}}-net — on-the-wire identifiers + bootstrap parsing.
pub const NETWORK_ID: &[u8] = b"{{NAME}}-{{TAG}}";
pub const NETWORK_ID_STR: &str = "{{NAME}}-{{TAG}}";
pub const PROTOCOL_PREFIX: &str = "/{{NAME}}/{{TAG}}/";

pub const TOPIC_BLOCKS: &str = "/{{NAME}}/{{TAG}}/blocks";
pub const TOPIC_PEER_HEIGHTS: &str = "/{{NAME}}/{{TAG}}/peer-heights";
pub const TOPIC_TIP_PROOFS: &str = "/{{NAME}}/{{TAG}}/tip-proofs";
pub const TOPIC_TXS: &str = "/{{NAME}}/{{TAG}}/txs";
pub const TOPIC_RELEASE: &str = "/{{NAME}}/{{TAG}}/release";

/// Subscribe order: tip-proofs FIRST so verify-before-sync holds.
pub const ALL_TOPICS: &[&str] =
    &[TOPIC_TIP_PROOFS, TOPIC_PEER_HEIGHTS, TOPIC_RELEASE, TOPIC_BLOCKS, TOPIC_TXS];

pub const DEFAULT_P2P_PORT: u16 = {{P2P_PORT}};
pub const DEFAULT_API_PORT: u16 = {{API_PORT}};

pub const BOOTSTRAP_ENV: &str = "{{NAME_UPPER}}_BOOTSTRAP_PEERS";

pub fn read_bootstrap_peers() -> Vec<String> {
    parse_bootstrap_list(&std::env::var(BOOTSTRAP_ENV).unwrap_or_default())
}
pub fn parse_bootstrap_list(s: &str) -> Vec<String> {
    s.split(',').map(str::trim).filter(|p| !p.is_empty()).map(String::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn topics_carry_prefix() {
        for t in ALL_TOPICS { assert!(t.starts_with(PROTOCOL_PREFIX), "{t}"); }
    }
    #[test]
    fn id_str_matches_bytes() { assert_eq!(NETWORK_ID_STR.as_bytes(), NETWORK_ID); }
    #[test]
    fn bootstrap_parses() {
        assert_eq!(parse_bootstrap_list(" a, ,b ").len(), 2);
        assert!(parse_bootstrap_list("").is_empty());
    }
}
"#;

const HEADER_LIB: &str = r#"//! {{NAME}}-header — block header v0 (content-addressed, deterministic).
use serde::{Deserialize, Serialize};

pub const NETWORK_ID: [u8; {{NID_LEN}}] = *b"{{NAME}}-{{TAG}}";
pub const HEADER_VERSION: u16 = 0;
pub type BlockHash = [u8; 32];
pub type Root = [u8; 32];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockHeader {
    pub version: u16,
    pub network_id: [u8; {{NID_LEN}}],
    pub height: u64,
    pub parent_hash: BlockHash,
    pub timestamp_ms: u64,
    pub state_root: Root, // commit-in-roots: the postmortem fix by construction
}

impl BlockHeader {
    pub fn genesis() -> Self {
        Self { version: HEADER_VERSION, network_id: NETWORK_ID, height: 0,
               parent_hash: [0u8; 32], timestamp_ms: 0, state_root: [0u8; 32] }
    }
    pub fn child(&self, timestamp_ms: u64, state_root: Root) -> Self {
        Self { version: HEADER_VERSION, network_id: NETWORK_ID, height: self.height + 1,
               parent_hash: self.hash(), timestamp_ms, state_root }
    }
    pub fn hash(&self) -> BlockHash {
        let mut h = blake3::Hasher::new();
        if let Ok(b) = serde_json::to_vec(self) { h.update(&b); }
        *h.finalize().as_bytes()
    }
    pub fn precheck(&self) -> Result<(), HeaderError> {
        if self.version != HEADER_VERSION {
            return Err(HeaderError::WrongVersion { expected: HEADER_VERSION, got: self.version });
        }
        if self.network_id != NETWORK_ID { return Err(HeaderError::WrongNetwork); }
        Ok(())
    }
    pub fn verify_child_of(&self, parent: &BlockHeader) -> Result<(), HeaderError> {
        self.precheck()?;
        if self.height != parent.height + 1 {
            return Err(HeaderError::HeightGap { expected: parent.height + 1, got: self.height });
        }
        if self.parent_hash != parent.hash() { return Err(HeaderError::ParentMismatch); }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum HeaderError {
    #[error("wrong header version: expected {expected}, got {got}")]
    WrongVersion { expected: u16, got: u16 },
    #[error("wrong network id")]
    WrongNetwork,
    #[error("height gap: expected {expected}, got {got}")]
    HeightGap { expected: u64, got: u64 },
    #[error("parent hash mismatch")]
    ParentMismatch,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn genesis_ok() { assert!(BlockHeader::genesis().precheck().is_ok()); }
    #[test]
    fn child_links() {
        let g = BlockHeader::genesis();
        let c = g.child(1000, [1u8; 32]);
        assert!(c.verify_child_of(&g).is_ok());
    }
    #[test]
    fn forged_parent_rejected() {
        let g = BlockHeader::genesis();
        let mut forged = g.child(1000, [1u8; 32]);
        forged.parent_hash = [7u8; 32];
        assert!(forged.verify_child_of(&g).is_err());
    }
    #[test]
    fn hash_deterministic() { let g = BlockHeader::genesis(); assert_eq!(g.hash(), g.hash()); }
}
"#;

const CHRONOS_LIB: &str = r#"//! {{NAME}}-chronos — deterministic chain sim + adversarial scenarios.
use {{NAME_USCORE}}_header::{BlockHeader, HeaderError, Root};

/// Build a deterministic linear chain of `n` blocks past genesis.
pub fn build_chain(n: u64) -> Vec<BlockHeader> {
    let mut chain = vec![BlockHeader::genesis()];
    for i in 1..=n {
        let parent = chain.last().unwrap().clone();
        chain.push(parent.child(i * 1000, det_root(i)));
    }
    chain
}
fn det_root(seed: u64) -> Root {
    let mut h = blake3::Hasher::new();
    h.update(&seed.to_le_bytes());
    *h.finalize().as_bytes()
}
/// Verify every block links to its parent. Returns blocks_applied.
pub fn verify_chain(chain: &[BlockHeader]) -> Result<usize, HeaderError> {
    if let Some(g) = chain.first() { g.precheck()?; }
    for w in chain.windows(2) { w[1].verify_child_of(&w[0])?; }
    Ok(chain.len())
}
/// Adversarial: tamper a block in the middle -> chain MUST reject downstream.
pub fn tamper_at(chain: &[BlockHeader], idx: usize) -> Vec<BlockHeader> {
    let mut c = chain.to_vec();
    if let Some(b) = c.get_mut(idx) { b.state_root = [0xFF; 32]; }
    c
}
"#;

const CHRONOS_TESTS: &str = r#"// tests/chronos.rs
use {{NAME_USCORE}}_chronos::*;

#[test]
fn happy_path_blocks_applied() {
    let c = build_chain(20);
    assert_eq!(verify_chain(&c).unwrap(), 21); // genesis + 20
}
#[test]
fn tamper_is_rejected() {
    let t = tamper_at(&build_chain(20), 10);
    assert!(verify_chain(&t).is_err()); // divergence detected
}
#[test]
fn deterministic_reproducible() {
    let a: Vec<_> = build_chain(8).iter().map(|b| b.hash()).collect();
    let b: Vec<_> = build_chain(8).iter().map(|b| b.hash()).collect();
    assert_eq!(a, b);
}
"#;

const NODE_MAIN: &str = r#"//! {{NAME}}-node — lightweight tip-verify node.
use {{NAME_USCORE}}_header::BlockHeader;

const NAME: &str = "{{NAME}}";
const NETWORK_ID_STR: &str = "{{NAME}}-{{TAG}}";
const DEFAULT_P2P_PORT: u16 = {{P2P_PORT}};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("genesis") => {
            let g = BlockHeader::genesis();
            println!("genesis hash: {}", hex::encode(g.hash()));
        }
        Some("verify") => {
            let path = args.get(2).ok_or("usage: {{NAME}}-node verify <chain.json>")?;
            let chain: Vec<BlockHeader> = serde_json::from_slice(&std::fs::read(path)?)?;
            for w in chain.windows(2) { w[1].verify_child_of(&w[0])?; }
            println!("ok VALID — {} blocks, tip height {}",
                     chain.len(), chain.last().map(|b| b.height).unwrap_or(0));
        }
        _ => println!("{NAME} node | net={NETWORK_ID_STR} p2p=:{DEFAULT_P2P_PORT}\nusage: {NAME}-node [genesis|verify <chain.json>]"),
    }
    Ok(())
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn aurum() -> ChainParams {
        ChainParams::new("aurum", "g0", 9601, 8281).unwrap()
    }

    #[test]
    fn rejects_bad_name() {
        assert!(ChainParams::new("Aurum", "g0", 9601, 8281).is_err()); // uppercase
        assert!(ChainParams::new("au rum", "g0", 9601, 8281).is_err()); // space
        assert!(ChainParams::new("", "g0", 9601, 8281).is_err());
    }

    #[test]
    fn rejects_oversize_network_id() {
        // 31-char name + "-g0" = 34 bytes > 32
        let long = "a".repeat(31);
        assert!(ChainParams::new(long, "g0", 9601, 8281).is_err());
    }

    #[test]
    fn derives_params() {
        let p = ChainParams::new("flux-coin", "g1", 9601, 8281).unwrap();
        assert_eq!(p.network_id(), "flux-coin-g1");
        assert_eq!(p.name_uscore(), "flux_coin");
        assert_eq!(p.name_upper(), "FLUX_COIN");
        assert_eq!(p.nid_len(), "flux-coin-g1".len());
    }

    #[test]
    fn generates_full_workspace() {
        let g = scaffold_chain(&aurum());
        // 11 files: workspace toml, readme, + 4 crates (net/header/chronos/node)
        assert_eq!(g.files.len(), 11);
        let paths: Vec<&str> = g.files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"Cargo.toml"));
        assert!(paths.contains(&"crates/aurum-header/src/lib.rs"));
        assert!(paths.contains(&"crates/aurum-chronos/tests/chronos.rs"));
        assert!(paths.contains(&"crates/aurum-node/src/main.rs"));
    }

    #[test]
    fn no_placeholders_leak() {
        let g = scaffold_chain(&aurum());
        for f in &g.files {
            assert!(!f.content.contains("{{"), "unfilled placeholder in {}", f.path);
            assert!(!f.content.contains("}}"), "unfilled placeholder in {}", f.path);
        }
    }

    #[test]
    fn substitution_is_correct() {
        let g = scaffold_chain(&aurum());
        let header = g.files.iter().find(|f| f.path.ends_with("aurum-header/src/lib.rs")).unwrap();
        assert!(header.content.contains(r#"*b"aurum-g0""#));
        // NID_LEN must equal the byte length of "aurum-g0" (8)
        assert!(header.content.contains("[u8; 8]"));
        let net = g.files.iter().find(|f| f.path.ends_with("aurum-net/src/lib.rs")).unwrap();
        assert!(net.content.contains("DEFAULT_P2P_PORT: u16 = 9601"));
        assert!(net.content.contains("AURUM_BOOTSTRAP_PEERS"));
    }

    #[test]
    fn chronos_uses_uscore_use_path() {
        let g = scaffold_chain(&ChainParams::new("flux-coin", "g0", 9601, 8281).unwrap());
        let chronos = g.files.iter().find(|f| f.path.ends_with("chronos/src/lib.rs")).unwrap();
        assert!(chronos.content.contains("flux_coin_header::"));
    }

    #[test]
    fn writes_to_disk() {
        let g = scaffold_chain(&aurum());
        let dir = std::env::temp_dir().join(format!("flux-ext-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let written = g.write_to(&dir).unwrap();
        assert_eq!(written.len(), 11);
        assert!(dir.join("crates/aurum-header/src/lib.rs").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manifest_lists_files_and_verify() {
        let m = scaffold_chain(&aurum()).manifest();
        assert!(m.contains("aurum"));
        assert!(m.contains("flux_combo aurum-header"));
    }
}
