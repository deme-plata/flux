//! roadmap — a signed, content-addressed ROADMAP ATTESTATION.
//!
//! `flux-rev snapshot` already proves *what a tree contained at a moment*. It cannot prove a
//! **claim about that tree**: "this code implements instant transaction ids, and that work is
//! in progress, not shipped". A roadmap in a README is a promise; this module turns it into an
//! object that can be checked by someone who does not trust the author.
//!
//! The attestation binds, for each roadmap item:
//!   `(item id, human title, status, why that status, what would make it Shipped,
//!     the exact source paths that implement it, each path's BLAKE3 + size + mode,
//!     the content-addressed tree id of that path set, timestamp, fluxc version, author)`
//!
//! and then hashes the whole body and signs it require-BOTH with SQIsign L5 + Ed25519.
//!
//! # Three independent tamper layers
//!
//! 1. **entries → tree_id.** Each item's `tree_id` is the [`crate::Manifest`] id of its entries —
//!    the SAME addressing function `snapshot` uses. Flip one byte of one entry hash and the
//!    recomputed tree id differs. Checkable OFFLINE, without the repository.
//! 2. **body → bundle_id.** Every field above is encoded length-prefixed and domain-separated
//!    into [`canonical_bytes`], hashed to `bundle_id`. Reordering or editing anything — a status,
//!    a title, a timestamp — changes it. Also offline.
//! 3. **bundle bytes → signature.** The canonical bytes are the signing input for a require-both
//!    hybrid bundle (`flux_sqisign::hybrid`). Forging layer 1 and 2 consistently still leaves an
//!    invalid signature unless the agent's key is broken in BOTH the isogeny and the elliptic-curve
//!    family.
//!
//! Layer 1 and 2 make the file self-checking. Layer 3 makes it attributable. A fourth, optional
//! check (`--recheck`) re-walks the real filesystem and reports DRIFT between the attestation and
//! the working tree — that one is about the repo having moved on, not about tampering, and is
//! reported separately so the two are never confused.
//!
//! # Honesty is enforced, not requested
//!
//! [`Status::Shipped`] is refused by [`RoadmapBody::validate`] unless the item carries a
//! `release_ref` — a concrete, checkable release pointer. An item cannot quietly promote itself by
//! editing a string. Both current items are [`Status::InProgress`] and carry no release_ref, which
//! is exactly why the invariant costs them nothing today and will bite the moment someone lies.

use crate::{hash_bytes, Entry, Manifest, SKIP_DIRS};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Schema tag. Baked into the signed bytes, so a v1 verifier cannot be handed a v2 body.
pub const SCHEMA: &str = "flux-rev/roadmap-attestation/v1";

/// Domain separator for the canonical encoding. Frozen — changing it invalidates every
/// attestation ever issued.
const DOMAIN: &[u8] = b"flux-rev/roadmap-attestation/v1\0";

/// The subject of this roadmap.
pub const PROJECT: &str = "sigil";

/// The two source roots an item path may name. A path is written `<tag>:<relpath>`, so the
/// attestation is machine-independent: it never records an absolute path.
pub const ROOT_TAGS: &[&str] = &["flux", "sigil"];

// ── status ──

/// Where an item actually is. Deliberately three-valued: the middle state is the honest one for
/// most real work, and having it removes the pressure to round "mostly done" up to Shipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    /// Built, released, and reachable by a user who did not build it themselves.
    Shipped,
    /// Real code exists and runs; the end-to-end claim is not yet proven.
    InProgress,
    /// Designed and written down. No implementing code yet.
    Specified,
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Shipped => "Shipped",
            Status::InProgress => "InProgress",
            Status::Specified => "Specified",
        }
    }
}

// ── the fixed roadmap ──

/// A roadmap item as declared in source. The declaration is the authority: the CLI cannot be
/// asked to attest an item that is not in this table, so an attestation can never contain a
/// third item smuggled in from a config file.
pub struct ItemSpec {
    pub id: &'static str,
    pub title: &'static str,
    pub status: Status,
    /// Why this status and not the next one up. Signed, so it cannot be quietly softened.
    pub status_rationale: &'static str,
    /// The concrete condition that would justify `Shipped`.
    pub ship_gate: &'static str,
    /// `(root_tag, repo-relative path)`. A path may be a file or a directory.
    pub paths: &'static [(&'static str, &'static str)],
}

/// **The roadmap. Exactly two items.** Adding a third is a source change, reviewed like any other.
pub const ROADMAP: &[ItemSpec] = &[
    ItemSpec {
        id: "instant-txid",
        title: "Instant transaction id + signed acceptance receipt at Confirm time",
        status: Status::InProgress,
        status_rationale:
            "The submit path returns a transaction id at Confirm time (sub-millisecond, no block \
             wait) and the finality arithmetic it must not be confused with is pinned by tests in \
             the same file: settlement is final_depth = 512 blocks, ~78 s at the measured 6.6 \
             blk/s. What does NOT yet exist is the second half of the claim — a SIGNED acceptance \
             receipt handed back at Confirm time, so today a caller has an id but no artifact \
             proving the node accepted it.",
        ship_gate:
            "A receipt object signed by the accepting node, returned synchronously from submit, \
             verifiable by a third party against the node's published key, and reachable from a \
             released client build.",
        paths: &[("sigil", "crates/sigil-api/src/send.rs")],
    },
    ItemSpec {
        id: "onboard-ai",
        title: "Self-installing on-device AI tab: flux-signed manifest -> verified ollama install \
                -> qwen3:8b pull, fail-closed on any hash/size mismatch",
        status: Status::InProgress,
        status_rationale:
            "The manifest path is live and signed: sigil-ai-latest.json plus its detached .sig \
             both serve over HTTPS and are verified client-side against the same pinned Ed25519 \
             key the auto-updater refuses to boot without; the installer is streamed, hashed \
             against Ollama's own published SHA-256 and byte size, and deleted rather than \
             executed on any mismatch, with no environment variable that skips the check. What is \
             NOT yet proven is the end-to-end install on a clean machine — the fail-closed paths \
             are exercised in code review, not in a from-scratch run.",
        ship_gate:
            "One recorded run on a machine with no ollama and no model cache that reaches a \
             working qwen3:8b chat, plus one recorded run where a deliberately corrupted \
             installer is refused.",
        paths: &[
            ("sigil", "crates/sigil-top/src/ai_setup.rs"),
            ("sigil", "crates/sigil-top/src/flux_moe.rs"),
            ("sigil", "scripts/publish-ai-manifests.sh"),
            ("flux", "crates/flux-moe/src/lib.rs"),
        ],
    },
];

// ── the attestation ──

/// One attested file: enough to recompute the tree id offline, and to re-check it on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathHash {
    /// Which root the path is relative to (`flux` | `sigil`).
    pub root: String,
    /// Repo-relative path, forward slashes. Never absolute — the attestation must verify on a
    /// machine that lays its checkouts out differently.
    pub path: String,
    pub hash: String,
    pub size: u64,
    pub mode: u32,
}

/// One roadmap item, resolved against a real tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttestedItem {
    pub id: String,
    pub title: String,
    pub status: Status,
    pub status_rationale: String,
    pub ship_gate: String,
    /// Required iff `status == Shipped`. Enforced by [`RoadmapBody::validate`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_ref: Option<String>,
    /// The declared path set, as written in [`ROADMAP`] (`<tag>:<relpath>`).
    pub declared_paths: Vec<String>,
    /// `full:<blake3hex>` — the [`Manifest`] id of `entries`. Same addressing function as
    /// `flux-rev snapshot`, so an item's tree id is comparable with a revision's.
    pub tree_id: String,
    pub entries: Vec<PathHash>,
    pub files: usize,
    pub bytes: u64,
}

impl AttestedItem {
    /// Recompute `tree_id` from `entries` alone — the offline integrity check for layer 1.
    pub fn recompute_tree_id(&self) -> String {
        tree_id_of(&self.entries)
    }
}

/// Everything that gets hashed and signed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoadmapBody {
    pub schema: String,
    pub project: String,
    pub fluxc_version: String,
    pub fluxc_git: String,
    pub author: String,
    pub ts_unix: u64,
    pub items: Vec<AttestedItem>,
}

/// The file `flux-rev roadmap` writes: body + its hash + the hybrid signature legs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attestation {
    pub body: RoadmapBody,
    /// `full:<blake3hex>` of [`canonical_bytes`] of `body`.
    pub bundle_id: String,
    /// SQIsign L5 (129-byte pk / 292-byte sig). Empty only in explicit `--unsigned` mode.
    #[serde(default)]
    pub sqisign_pubkey_hex: String,
    #[serde(default)]
    pub sqisign_sig_hex: String,
    /// Ed25519 classical leg. Present ⇒ require-BOTH verification.
    #[serde(default)]
    pub ed25519_pubkey_hex: String,
    #[serde(default)]
    pub ed25519_sig_hex: String,
}

impl Attestation {
    pub fn is_signed(&self) -> bool {
        !self.sqisign_sig_hex.is_empty()
    }
    pub fn is_hybrid(&self) -> bool {
        !self.sqisign_sig_hex.is_empty() && !self.ed25519_sig_hex.is_empty()
    }
}

// ── errors ──

#[derive(Debug)]
pub enum RoadmapError {
    Io(io::Error),
    Json(String),
    /// A declared path did not resolve, or a root tag was not supplied.
    Path(String),
    /// `bundle_id` did not match a re-hash of the body. TAMPER.
    BundleHashMismatch { stored: String, recomputed: String },
    /// An item's `tree_id` did not match a re-hash of its entries. TAMPER.
    TreeIdMismatch { item: String, stored: String, recomputed: String },
    /// A structural rule was broken (item count, ids, Shipped-without-release_ref, bad hex).
    Schema(String),
    /// The hybrid signature did not validate over the canonical bytes. TAMPER or wrong key.
    InvalidSignature(String),
    /// The file carries no signature and the caller did not pass `--allow-unsigned`.
    Unsigned,
    /// No agent key available to sign with.
    NoKey(String),
}

impl std::fmt::Display for RoadmapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RoadmapError::Io(e) => write!(f, "io: {e}"),
            RoadmapError::Json(e) => write!(f, "json: {e}"),
            RoadmapError::Path(p) => write!(f, "path: {p}"),
            RoadmapError::BundleHashMismatch { stored, recomputed } => write!(
                f,
                "TAMPER: bundle hash mismatch — file says {stored}, body hashes to {recomputed}"
            ),
            RoadmapError::TreeIdMismatch { item, stored, recomputed } => write!(
                f,
                "TAMPER: item '{item}' tree id mismatch — file says {stored}, entries hash to {recomputed}"
            ),
            RoadmapError::Schema(m) => write!(f, "schema: {m}"),
            RoadmapError::InvalidSignature(m) => write!(f, "TAMPER: signature invalid — {m}"),
            RoadmapError::Unsigned => {
                write!(f, "attestation carries no signature (pass --allow-unsigned to accept)")
            }
            RoadmapError::NoKey(m) => write!(f, "no signing key: {m}"),
        }
    }
}

impl std::error::Error for RoadmapError {}

impl From<io::Error> for RoadmapError {
    fn from(e: io::Error) -> Self {
        RoadmapError::Io(e)
    }
}

// ── canonical encoding (layer 2) ──

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(&(b.len() as u64).to_le_bytes());
    out.extend_from_slice(b);
}
fn put_str(out: &mut Vec<u8>, s: &str) {
    put_bytes(out, s.as_bytes());
}
fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// Length-prefixed, domain-separated encoding of the body. Hand-rolled rather than reusing the
/// crate's `serde_json` canon for one reason: this is a SIGNING input. A serde attribute added
/// three years from now (a rename, a `skip_serializing_if`) would silently change JSON bytes and
/// invalidate every past signature. Explicit framing cannot drift, and every variable-length field
/// carries its length, so no two distinct bodies can encode to the same bytes.
pub fn canonical_bytes(body: &RoadmapBody) -> Vec<u8> {
    let mut out = Vec::with_capacity(4096);
    out.extend_from_slice(DOMAIN);
    put_str(&mut out, &body.schema);
    put_str(&mut out, &body.project);
    put_str(&mut out, &body.fluxc_version);
    put_str(&mut out, &body.fluxc_git);
    put_str(&mut out, &body.author);
    put_u64(&mut out, body.ts_unix);
    put_u64(&mut out, body.items.len() as u64);
    for it in &body.items {
        put_str(&mut out, &it.id);
        put_str(&mut out, &it.title);
        put_str(&mut out, it.status.as_str());
        put_str(&mut out, &it.status_rationale);
        put_str(&mut out, &it.ship_gate);
        match &it.release_ref {
            Some(r) => {
                out.push(1);
                put_str(&mut out, r);
            }
            None => out.push(0),
        }
        put_u64(&mut out, it.declared_paths.len() as u64);
        for p in &it.declared_paths {
            put_str(&mut out, p);
        }
        put_str(&mut out, &it.tree_id);
        put_u64(&mut out, it.entries.len() as u64);
        for e in &it.entries {
            put_str(&mut out, &e.root);
            put_str(&mut out, &e.path);
            put_str(&mut out, &e.hash);
            put_u64(&mut out, e.size);
            put_u64(&mut out, e.mode as u64);
        }
        put_u64(&mut out, it.files as u64);
        put_u64(&mut out, it.bytes);
    }
    out
}

/// `full:<blake3hex>` of the canonical body bytes.
pub fn bundle_id_of(body: &RoadmapBody) -> String {
    format!("full:{}", hash_bytes(&canonical_bytes(body)))
}

/// `full:<blake3hex>` — the [`Manifest`] id of an entry set. Reuses the exact addressing function
/// `flux-rev snapshot` uses for a working tree, so an item's tree id and a revision's manifest id
/// live in the same namespace and can be compared directly.
pub fn tree_id_of(entries: &[PathHash]) -> String {
    let mut es: Vec<Entry> = entries
        .iter()
        .map(|e| Entry {
            path: format!("{}:{}", e.root, e.path),
            hash: e.hash.clone(),
            mode: e.mode,
        })
        .collect();
    es.sort_by(|a, b| a.path.cmp(&b.path));
    format!("full:{}", Manifest { entries: es }.id())
}

// ── validation (structural honesty) ──

impl RoadmapBody {
    /// Structural rules. Every one of these is a way an attestation could lie while still hashing
    /// and verifying correctly, so they are checked on BOTH build and verify.
    pub fn validate(&self) -> Result<(), RoadmapError> {
        if self.schema != SCHEMA {
            return Err(RoadmapError::Schema(format!(
                "unknown schema {:?} (this verifier speaks {SCHEMA:?})",
                self.schema
            )));
        }
        if self.items.len() != ROADMAP.len() {
            return Err(RoadmapError::Schema(format!(
                "expected exactly {} roadmap items, found {}",
                ROADMAP.len(),
                self.items.len()
            )));
        }
        for (got, want) in self.items.iter().zip(ROADMAP.iter()) {
            if got.id != want.id {
                return Err(RoadmapError::Schema(format!(
                    "item id {:?} is not a declared roadmap item (expected {:?})",
                    got.id, want.id
                )));
            }
        }
        for it in &self.items {
            if it.status == Status::Shipped
                && it.release_ref.as_deref().map(str::trim).unwrap_or("").is_empty()
            {
                return Err(RoadmapError::Schema(format!(
                    "item '{}' claims Shipped with no release_ref — a Shipped claim must point at \
                     something a stranger can fetch",
                    it.id
                )));
            }
            if it.entries.is_empty() {
                return Err(RoadmapError::Schema(format!(
                    "item '{}' attests no source at all",
                    it.id
                )));
            }
            if it.files != it.entries.len() {
                return Err(RoadmapError::Schema(format!(
                    "item '{}' file count {} disagrees with {} entries",
                    it.id,
                    it.files,
                    it.entries.len()
                )));
            }
            let sum: u64 = it.entries.iter().map(|e| e.size).sum();
            if sum != it.bytes {
                return Err(RoadmapError::Schema(format!(
                    "item '{}' byte total {} disagrees with entry sizes {}",
                    it.id, it.bytes, sum
                )));
            }
            for e in &it.entries {
                if e.hash.len() != 64 || !e.hash.bytes().all(|c| c.is_ascii_hexdigit()) {
                    return Err(RoadmapError::Schema(format!(
                        "item '{}' entry '{}' has a malformed hash",
                        it.id, e.path
                    )));
                }
                if !ROOT_TAGS.contains(&e.root.as_str()) {
                    return Err(RoadmapError::Schema(format!(
                        "item '{}' entry '{}' names unknown root '{}'",
                        it.id, e.path, e.root
                    )));
                }
                if e.path.starts_with('/') || e.path.contains("..") {
                    return Err(RoadmapError::Schema(format!(
                        "item '{}' entry path '{}' is not repo-relative",
                        it.id, e.path
                    )));
                }
            }
            let recomputed = it.recompute_tree_id();
            if recomputed != it.tree_id {
                return Err(RoadmapError::TreeIdMismatch {
                    item: it.id.clone(),
                    stored: it.tree_id.clone(),
                    recomputed,
                });
            }
        }
        Ok(())
    }
}

// ── building ──

/// Hash one declared path (file OR directory) into entries. Directories are walked with the same
/// `SKIP_DIRS` exclusions `snapshot` uses, so an attested directory never captures build output.
fn collect(root_tag: &str, root: &Path, rel: &str, out: &mut Vec<PathHash>) -> Result<(), RoadmapError> {
    let abs = root.join(rel);
    let meta = fs::metadata(&abs).map_err(|e| {
        RoadmapError::Path(format!("{}:{} → {} ({e})", root_tag, rel, abs.display()))
    })?;
    if meta.is_dir() {
        let mut files: Vec<(String, PathBuf, u32)> = Vec::new();
        walk_dir(&abs, &abs, &mut files)?;
        files.sort_by(|a, b| a.0.cmp(&b.0));
        for (sub, path, mode) in files {
            let bytes = fs::read(&path)?;
            out.push(PathHash {
                root: root_tag.to_string(),
                path: format!("{}/{}", rel.trim_end_matches('/'), sub),
                hash: hash_bytes(&bytes),
                size: bytes.len() as u64,
                mode,
            });
        }
    } else {
        let bytes = fs::read(&abs)?;
        out.push(PathHash {
            root: root_tag.to_string(),
            path: rel.to_string(),
            hash: hash_bytes(&bytes),
            size: bytes.len() as u64,
            mode: file_mode(&meta),
        });
    }
    Ok(())
}

fn walk_dir(base: &Path, dir: &Path, out: &mut Vec<(String, PathBuf, u32)>) -> io::Result<()> {
    for e in fs::read_dir(dir)? {
        let e = e?;
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) {
                walk_dir(base, &p, out)?;
            }
        } else if p.is_file() {
            let rel = p.strip_prefix(base).unwrap_or(&p).to_string_lossy().replace('\\', "/");
            let mode = e.metadata().map(|m| file_mode(&m)).unwrap_or(0o644);
            out.push((rel, p.clone(), mode));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn file_mode(m: &fs::Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt;
    m.mode() & 0o777
}
#[cfg(not(unix))]
fn file_mode(_m: &fs::Metadata) -> u32 {
    0o644
}

/// Where each root tag lives on THIS machine. Absolute, and deliberately never written into the
/// attestation — only the tags are signed.
pub type Roots = HashMap<String, PathBuf>;

/// Resolve [`ROADMAP`] against real trees and build the (still unsigned) body.
pub fn build_body(
    roots: &Roots,
    fluxc_version: &str,
    fluxc_git: &str,
    author: &str,
    ts_unix: u64,
) -> Result<RoadmapBody, RoadmapError> {
    let mut items = Vec::with_capacity(ROADMAP.len());
    for spec in ROADMAP {
        let mut entries = Vec::new();
        let mut declared = Vec::new();
        for (tag, rel) in spec.paths {
            declared.push(format!("{tag}:{rel}"));
            let root = roots.get(*tag).ok_or_else(|| {
                RoadmapError::Path(format!(
                    "item '{}' needs root '{}' — pass --{}-root <dir>",
                    spec.id, tag, tag
                ))
            })?;
            collect(tag, root, rel, &mut entries)?;
        }
        entries.sort_by(|a, b| (a.root.as_str(), a.path.as_str()).cmp(&(b.root.as_str(), b.path.as_str())));
        let files = entries.len();
        let bytes = entries.iter().map(|e| e.size).sum();
        items.push(AttestedItem {
            id: spec.id.to_string(),
            title: spec.title.to_string(),
            status: spec.status,
            status_rationale: spec.status_rationale.to_string(),
            ship_gate: spec.ship_gate.to_string(),
            release_ref: None,
            declared_paths: declared,
            tree_id: tree_id_of(&entries),
            entries,
            files,
            bytes,
        });
    }
    let body = RoadmapBody {
        schema: SCHEMA.to_string(),
        project: PROJECT.to_string(),
        fluxc_version: fluxc_version.to_string(),
        fluxc_git: fluxc_git.to_string(),
        author: author.to_string(),
        ts_unix,
        items,
    };
    body.validate()?;
    Ok(body)
}

// ── signing (layer 3) ──

/// Hybrid agent keys `(sqi_sk, sqi_pk, ed_sk, ed_pk)`.
pub type HybridKeys = (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>);

/// Read the agent identity from the same key file `fluxc`'s provenance uses
/// (`$FLUX_AGENT_KEY_PATH`, else `$HOME/.flux-agent-key.json`), so a roadmap attestation and a
/// build provenance proof are signed by ONE identity rather than two unrelated ones.
///
/// This crate cannot call `fluxc_core::provenance::load_agent_keys_hybrid` — `fluxc-core` depends
/// on `flux-rev`, so the reverse edge would be a dependency cycle. The FILE FORMAT is the contract
/// between them, not a function call.
pub fn load_agent_keys_hybrid(path: Option<&str>) -> Result<HybridKeys, RoadmapError> {
    let path = path
        .map(str::to_string)
        .or_else(|| std::env::var("FLUX_AGENT_KEY_PATH").ok())
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
            format!("{home}/.flux-agent-key.json")
        });
    let raw = fs::read_to_string(&path)
        .map_err(|e| RoadmapError::NoKey(format!("read {path}: {e}")))?;
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| RoadmapError::NoKey(format!("parse {path}: {e}")))?;
    let get = |k: &str| -> Result<Vec<u8>, RoadmapError> {
        v.get(k)
            .and_then(|s| s.as_str())
            .ok_or_else(|| RoadmapError::NoKey(format!("{path}: missing {k}")))
            .and_then(|s| {
                hex::decode(s.trim().trim_start_matches("0x"))
                    .map_err(|e| RoadmapError::NoKey(format!("{path}: {k} not hex: {e}")))
            })
    };
    Ok((get("sk_hex")?, get("pk_hex")?, get("ed_sk_hex")?, get("ed_pk_hex")?))
}

/// Hash the body and sign the canonical bytes require-BOTH (SQIsign L5 + Ed25519).
pub fn sign_body(body: RoadmapBody, keys: &HybridKeys) -> Result<Attestation, RoadmapError> {
    use flux_sqisign::hybrid::SchemeId;
    body.validate()?;
    let bundle_id = bundle_id_of(&body);
    let record = canonical_bytes(&body);

    let (sqi_sk, sqi_pk, ed_sk, ed_pk) = keys;
    let mut km = HashMap::new();
    km.insert(SchemeId::SQIsign, (sqi_sk.clone(), sqi_pk.clone()));
    km.insert(SchemeId::Ed25519, (ed_sk.clone(), ed_pk.clone()));
    let bundle =
        flux_sqisign::hybrid::hybrid_sign(&record, &km, &[SchemeId::SQIsign, SchemeId::Ed25519])
            .map_err(RoadmapError::InvalidSignature)?;

    let mut att = Attestation {
        body,
        bundle_id,
        sqisign_pubkey_hex: String::new(),
        sqisign_sig_hex: String::new(),
        ed25519_pubkey_hex: String::new(),
        ed25519_sig_hex: String::new(),
    };
    for leg in bundle.signatures {
        match leg.scheme {
            SchemeId::SQIsign => {
                att.sqisign_pubkey_hex = hex::encode(&leg.public_key);
                att.sqisign_sig_hex = hex::encode(&leg.signature);
            }
            SchemeId::Ed25519 => {
                att.ed25519_pubkey_hex = hex::encode(&leg.public_key);
                att.ed25519_sig_hex = hex::encode(&leg.signature);
            }
            SchemeId::Dilithium5 => {}
        }
    }
    Ok(att)
}

/// Build an unsigned attestation (hash layers only). Useful in CI without an agent key; a verifier
/// refuses it unless explicitly told to accept unsigned.
pub fn unsigned(body: RoadmapBody) -> Result<Attestation, RoadmapError> {
    body.validate()?;
    let bundle_id = bundle_id_of(&body);
    Ok(Attestation {
        body,
        bundle_id,
        sqisign_pubkey_hex: String::new(),
        sqisign_sig_hex: String::new(),
        ed25519_pubkey_hex: String::new(),
        ed25519_sig_hex: String::new(),
    })
}

// ── verification ──

/// What a successful verify established. Every field is something that was CHECKED, not read.
#[derive(Debug, Clone)]
pub struct Verified {
    pub bundle_id: String,
    pub signed: bool,
    pub hybrid: bool,
    pub signer_sqisign_pk_hex: String,
    pub items: Vec<(String, Status, String)>,
    /// Populated only by [`recheck`]: entries whose on-disk bytes no longer match.
    pub drift: Vec<String>,
}

/// Re-check every layer. Fails loudly and specifically: the error says WHICH layer broke.
pub fn verify(att: &Attestation, allow_unsigned: bool) -> Result<Verified, RoadmapError> {
    // Layer 0+1: structure, and entries → tree_id (validate does both).
    att.body.validate()?;

    // Layer 2: body → bundle_id.
    let recomputed = bundle_id_of(&att.body);
    if recomputed != att.bundle_id {
        return Err(RoadmapError::BundleHashMismatch {
            stored: att.bundle_id.clone(),
            recomputed,
        });
    }

    // Layer 3: signature over the canonical bytes.
    if !att.is_signed() {
        if !allow_unsigned {
            return Err(RoadmapError::Unsigned);
        }
    } else {
        use flux_sqisign::hybrid::{HybridSignature, SchemeId, SchemeSignature};
        let record = canonical_bytes(&att.body);
        let dec = |what: &str, s: &str| -> Result<Vec<u8>, RoadmapError> {
            hex::decode(s).map_err(|e| RoadmapError::InvalidSignature(format!("{what} not hex: {e}")))
        };
        if !att.is_hybrid() {
            return Err(RoadmapError::InvalidSignature(
                "attestation has a SQIsign leg but no Ed25519 leg — this schema is require-BOTH, a \
                 single-leg bundle is refused rather than downgraded"
                    .into(),
            ));
        }
        let bundle = HybridSignature {
            version: 1,
            signatures: vec![
                SchemeSignature {
                    scheme: SchemeId::SQIsign,
                    public_key: dec("sqisign pubkey", &att.sqisign_pubkey_hex)?,
                    signature: dec("sqisign sig", &att.sqisign_sig_hex)?,
                    verified: false,
                },
                SchemeSignature {
                    scheme: SchemeId::Ed25519,
                    public_key: dec("ed25519 pubkey", &att.ed25519_pubkey_hex)?,
                    signature: dec("ed25519 sig", &att.ed25519_sig_hex)?,
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
            return Err(RoadmapError::InvalidSignature(if r.reason.is_empty() {
                format!("failed legs: {:?}", r.failed_schemes)
            } else {
                r.reason
            }));
        }
    }

    Ok(Verified {
        bundle_id: att.bundle_id.clone(),
        signed: att.is_signed(),
        hybrid: att.is_hybrid(),
        signer_sqisign_pk_hex: att.sqisign_pubkey_hex.clone(),
        items: att
            .body
            .items
            .iter()
            .map(|i| (i.id.clone(), i.status, i.tree_id.clone()))
            .collect(),
        drift: Vec::new(),
    })
}

/// Optional fourth check: re-walk the real trees and report entries whose bytes have moved on.
///
/// Kept SEPARATE from [`verify`] on purpose. Drift is not tampering — it is the ordinary fact that
/// code changed after the attestation was cut. Folding the two together would make an honest
/// attestation of yesterday's tree look like a forgery.
pub fn recheck(att: &Attestation, roots: &Roots) -> Result<Vec<String>, RoadmapError> {
    let mut drift = Vec::new();
    for it in &att.body.items {
        for e in &it.entries {
            let Some(root) = roots.get(&e.root) else { continue };
            let abs = root.join(&e.path);
            match fs::read(&abs) {
                Ok(b) => {
                    let h = hash_bytes(&b);
                    if h != e.hash {
                        drift.push(format!("{}: {}:{} changed ({}… → {}…)", it.id, e.root, e.path, &e.hash[..12], &h[..12]));
                    }
                }
                Err(_) => drift.push(format!("{}: {}:{} MISSING on disk", it.id, e.root, e.path)),
            }
        }
    }
    Ok(drift)
}

// ── file io ──

pub fn to_json(att: &Attestation) -> Result<Vec<u8>, RoadmapError> {
    serde_json::to_vec_pretty(att).map_err(|e| RoadmapError::Json(e.to_string()))
}

pub fn from_json(bytes: &[u8]) -> Result<Attestation, RoadmapError> {
    serde_json::from_slice(bytes).map_err(|e| RoadmapError::Json(e.to_string()))
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway pair of trees holding the declared roadmap paths, so the tests exercise the
    /// REAL `build_body` path (walk → hash → manifest id) rather than a hand-built body.
    fn fixture(tag: &str) -> (PathBuf, Roots) {
        let base = std::env::temp_dir().join(format!("flux-rev-roadmap-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let mut roots = Roots::new();
        for spec in ROADMAP {
            for (root_tag, rel) in spec.paths {
                let abs = base.join(root_tag).join(rel);
                fs::create_dir_all(abs.parent().unwrap()).unwrap();
                fs::write(&abs, format!("// {root_tag}/{rel}\npub fn stub() {{}}\n")).unwrap();
                roots.insert((*root_tag).to_string(), base.join(root_tag));
            }
        }
        (base, roots)
    }

    fn body_of(tag: &str) -> (PathBuf, Roots, RoadmapBody) {
        let (base, roots) = fixture(tag);
        let b = build_body(&roots, "0.41.0", "3667615a", "rocky", 1_756_800_000).unwrap();
        (base, roots, b)
    }

    #[test]
    fn roadmap_has_exactly_the_two_declared_items_and_neither_claims_shipped() {
        assert_eq!(ROADMAP.len(), 2, "the roadmap is fixed at two items");
        assert_eq!(ROADMAP[0].id, "instant-txid");
        assert_eq!(ROADMAP[1].id, "onboard-ai");
        for s in ROADMAP {
            assert_ne!(
                s.status,
                Status::Shipped,
                "item '{}' claims Shipped — no roadmap item may claim Shipped without a \
                 release_ref, and the declaration table has no way to supply one",
                s.id
            );
            assert!(!s.ship_gate.trim().is_empty(), "item '{}' has no ship gate", s.id);
            assert!(!s.status_rationale.trim().is_empty(), "item '{}' has no rationale", s.id);
            assert!(!s.paths.is_empty(), "item '{}' attests no source", s.id);
            for (t, _) in s.paths {
                assert!(ROOT_TAGS.contains(t), "item '{}' names unknown root '{t}'", s.id);
            }
        }
    }

    #[test]
    fn build_then_verify_roundtrips_through_json() {
        let (_b, _r, body) = body_of("rt");
        let keys = flux_sqisign::hybrid::hybrid_keygen(&[
            flux_sqisign::hybrid::SchemeId::SQIsign,
            flux_sqisign::hybrid::SchemeId::Ed25519,
        ])
        .unwrap();
        let (sk, pk) = keys[&flux_sqisign::hybrid::SchemeId::SQIsign].clone();
        let (esk, epk) = keys[&flux_sqisign::hybrid::SchemeId::Ed25519].clone();
        let att = sign_body(body, &(sk, pk, esk, epk)).unwrap();

        let json = to_json(&att).unwrap();
        let back = from_json(&json).unwrap();
        assert_eq!(back, att, "JSON round-trip is lossless");

        let v = verify(&back, false).unwrap();
        assert!(v.signed && v.hybrid, "both legs present");
        assert_eq!(v.items.len(), 2);
        assert_eq!(v.items[0].1, Status::InProgress);
        assert_eq!(v.items[1].1, Status::InProgress);
        assert!(v.bundle_id.starts_with("full:"), "bundle id uses the flux-rev address form");
    }

    /// **The tamper test.** Flip ONE byte of one attested source hash and every layer that can
    /// notice, must. Layer 1 (entries → tree_id) fires first because it is the innermost.
    #[test]
    fn tamper_a_single_byte_of_an_entry_hash_is_caught() {
        let (_b, _r, body) = body_of("tamper-entry");
        let att = unsigned(body).unwrap();
        assert!(verify(&att, true).is_ok(), "the honest attestation verifies");

        let mut bad = att.clone();
        let h = &mut bad.body.items[0].entries[0].hash;
        // flip the low bit of the first hex nibble — a one-byte edit
        let first = h.remove(0);
        let flipped = char::from_digit((first.to_digit(16).unwrap() ^ 1) as u32, 16).unwrap();
        h.insert(0, flipped);

        match verify(&bad, true) {
            Err(RoadmapError::TreeIdMismatch { item, .. }) => assert_eq!(item, "instant-txid"),
            other => panic!("a flipped entry hash must fail verification, got {other:?}"),
        }
    }

    /// Tamper that survives layer 1 (recompute the tree id too) must still die at layer 2.
    #[test]
    fn tamper_that_repairs_the_tree_id_still_fails_the_bundle_hash() {
        let (_b, _r, body) = body_of("tamper-tree");
        let att = unsigned(body).unwrap();

        let mut bad = att.clone();
        let h = &mut bad.body.items[0].entries[0].hash;
        let first = h.remove(0);
        h.insert(0, char::from_digit((first.to_digit(16).unwrap() ^ 1) as u32, 16).unwrap());
        // forger repairs the inner layer
        bad.body.items[0].tree_id = bad.body.items[0].recompute_tree_id();

        match verify(&bad, true) {
            Err(RoadmapError::BundleHashMismatch { stored, recomputed }) => {
                assert_ne!(stored, recomputed)
            }
            other => panic!("a repaired-inner-layer forgery must fail the bundle hash, got {other:?}"),
        }
    }

    /// And a forger who repairs BOTH hash layers still cannot produce a valid signature.
    #[test]
    fn tamper_that_repairs_every_hash_still_fails_the_signature() {
        let (_b, _r, body) = body_of("tamper-sig");
        let keys = flux_sqisign::hybrid::hybrid_keygen(&[
            flux_sqisign::hybrid::SchemeId::SQIsign,
            flux_sqisign::hybrid::SchemeId::Ed25519,
        ])
        .unwrap();
        let (sk, pk) = keys[&flux_sqisign::hybrid::SchemeId::SQIsign].clone();
        let (esk, epk) = keys[&flux_sqisign::hybrid::SchemeId::Ed25519].clone();
        let att = sign_body(body, &(sk, pk, esk, epk)).unwrap();
        assert!(verify(&att, false).is_ok());

        // The lie that matters: promote an InProgress item to Shipped, then repair both hashes.
        let mut bad = att.clone();
        bad.body.items[1].status = Status::Shipped;
        bad.body.items[1].release_ref = Some("https://example.invalid/fake".into()); // dodge the schema rule
        bad.body.items[1].tree_id = bad.body.items[1].recompute_tree_id();
        bad.bundle_id = bundle_id_of(&bad.body);

        match verify(&bad, false) {
            Err(RoadmapError::InvalidSignature(_)) => {}
            other => panic!("a status forgery with repaired hashes must fail the signature, got {other:?}"),
        }
    }

    /// Stripping the classical leg must be refused, not silently downgraded to SQIsign-only.
    #[test]
    fn stripping_the_ed25519_leg_is_refused_not_downgraded() {
        let (_b, _r, body) = body_of("strip");
        let keys = flux_sqisign::hybrid::hybrid_keygen(&[
            flux_sqisign::hybrid::SchemeId::SQIsign,
            flux_sqisign::hybrid::SchemeId::Ed25519,
        ])
        .unwrap();
        let (sk, pk) = keys[&flux_sqisign::hybrid::SchemeId::SQIsign].clone();
        let (esk, epk) = keys[&flux_sqisign::hybrid::SchemeId::Ed25519].clone();
        let mut att = sign_body(body, &(sk, pk, esk, epk)).unwrap();
        att.ed25519_sig_hex.clear();
        att.ed25519_pubkey_hex.clear();
        assert!(matches!(verify(&att, false), Err(RoadmapError::InvalidSignature(_))));
    }

    #[test]
    fn an_unsigned_attestation_is_refused_unless_explicitly_allowed() {
        let (_b, _r, body) = body_of("unsigned");
        let att = unsigned(body).unwrap();
        assert!(matches!(verify(&att, false), Err(RoadmapError::Unsigned)));
        assert!(verify(&att, true).is_ok());
    }

    /// The honesty invariant: Shipped without a release_ref is a schema violation, so no item can
    /// promote itself by editing one word.
    #[test]
    fn shipped_without_a_release_ref_is_a_schema_violation() {
        let (_b, _r, mut body) = body_of("shipped");
        body.items[0].status = Status::Shipped;
        match body.validate() {
            Err(RoadmapError::Schema(m)) => assert!(m.contains("release_ref"), "got: {m}"),
            other => panic!("Shipped with no release_ref must be refused, got {other:?}"),
        }
        body.items[0].release_ref = Some("sigil-top v7.4.2 signed manifest".into());
        assert!(body.validate().is_ok(), "Shipped WITH a release_ref is allowed");
    }

    /// A third item cannot be smuggled in, and neither can a renamed one.
    #[test]
    fn a_third_item_or_a_renamed_item_is_refused() {
        let (_b, _r, mut body) = body_of("third");
        let extra = body.items[0].clone();
        body.items.push(extra);
        assert!(matches!(body.validate(), Err(RoadmapError::Schema(_))), "3 items refused");

        let (_b2, _r2, mut body2) = body_of("rename");
        body2.items[0].id = "instant-txid-v2".into();
        assert!(matches!(body2.validate(), Err(RoadmapError::Schema(_))), "renamed id refused");
    }

    /// Canonical encoding must be injective where it counts: two bodies that differ only in where
    /// a field boundary falls must not encode identically. Length prefixes are what buy this.
    #[test]
    fn canonical_encoding_is_not_ambiguous_across_field_boundaries() {
        let (_b, _r, a) = body_of("canon");
        let mut b = a.clone();
        // move one character from the id into the title — same concatenation, different fields
        b.items[0].id = "instant-txi".into();
        b.items[0].title = format!("d{}", a.items[0].title);
        assert_ne!(
            canonical_bytes(&a),
            canonical_bytes(&b),
            "length-prefixed framing must distinguish these"
        );
    }

    /// Drift is reported, but is NOT tampering — an attestation of yesterday's tree still verifies.
    #[test]
    fn drift_is_reported_separately_from_tamper() {
        let (base, roots, body) = body_of("drift");
        let att = unsigned(body).unwrap();
        assert!(recheck(&att, &roots).unwrap().is_empty(), "fresh attestation: no drift");

        let e = &att.body.items[0].entries[0];
        fs::write(base.join(&e.root).join(&e.path), "// edited after the attestation\n").unwrap();
        let d = recheck(&att, &roots).unwrap();
        assert_eq!(d.len(), 1, "exactly the edited file drifted");
        assert!(d[0].contains("instant-txid"));
        assert!(verify(&att, true).is_ok(), "drift does not make the attestation a forgery");
    }

    /// The tree id must be the SAME function `snapshot` uses, or an item's id is not comparable
    /// with a revision's manifest id and the "content-addressed" claim is decorative.
    #[test]
    fn tree_id_is_the_snapshot_manifest_id() {
        let (_b, _r, body) = body_of("treeid");
        let it = &body.items[1];
        let mut es: Vec<Entry> = it
            .entries
            .iter()
            .map(|e| Entry { path: format!("{}:{}", e.root, e.path), hash: e.hash.clone(), mode: e.mode })
            .collect();
        es.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(it.tree_id, format!("full:{}", Manifest { entries: es }.id()));
        assert_eq!(it.files, 4, "onboard-ai attests four source paths");
    }
}
