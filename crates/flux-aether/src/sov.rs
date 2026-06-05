//! AETHER-SOV — dynamic, server-less version/artifact convergence across a node mesh.
//!
//! This is the layer that wires the **flux-native version layer (③)** into
//! flux-aether: every node holds a *sovereign manifest* of
//! `{artifact -> version + content-hash + producer}`, gossips compact digests,
//! computes an [`AetherSyncPlan`], downloads only what it lacks (the
//! *aether-download path*), and merges via a **last-writer-wins CRDT**
//! (tie-broken by content-hash). Because the merge is commutative, associative,
//! and idempotent, **any node can go offline, mutate locally, and re-converge on
//! rejoin** — no central server, each node self-sovereign.
//!
//! Scope discipline: this module touches **no consensus / balance / crypto
//! logic**. "Signing identity" is the *same* structural blake3-keyed seam
//! `aether.rs` already uses for the producer field — a SQIsign signature plugs in
//! at [`VersionEntry::sig`] later exactly like the rest of the crate. Convergence
//! does not depend on asymmetric verification.

use crate::aether::Hash;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A semver triple. Mirrors `fluxc-core::version::VersionInfo` (the ③ layer) but
/// kept dependency-free so flux-aether stays a leaf crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Ver {
    /// Major component.
    pub major: u32,
    /// Minor component.
    pub minor: u32,
    /// Patch component.
    pub patch: u32,
}

impl Ver {
    /// Construct a version triple.
    pub fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self { major, minor, patch }
    }
    /// `"v{major}.{minor}.{patch}"`.
    pub fn display(&self) -> String {
        format!("v{}.{}.{}", self.major, self.minor, self.patch)
    }
    fn pack(&self) -> [u8; 12] {
        let mut o = [0u8; 12];
        o[0..4].copy_from_slice(&self.major.to_le_bytes());
        o[4..8].copy_from_slice(&self.minor.to_le_bytes());
        o[8..12].copy_from_slice(&self.patch.to_le_bytes());
        o
    }
}

/// A per-node sovereign identity. `id` is the public node id (`H(secret)`);
/// `secret` keys the blake3 MAC that stamps this node's entries. This is the
/// SQIsign seam — asymmetric signing/verification plugs in here without changing
/// any convergence logic.
#[derive(Clone, Debug)]
pub struct NodeIdentity {
    /// Public node id, advertised to peers.
    pub id: Hash,
    secret: [u8; 32],
}

impl NodeIdentity {
    /// Derive a stable identity from a seed (e.g. the node name `b"epsilon"`).
    /// Deterministic so a node keeps its identity across restarts.
    pub fn from_seed(seed: &[u8]) -> Self {
        let secret = *blake3::hash(seed).as_bytes();
        let id = *blake3::hash(&secret).as_bytes();
        Self { id, secret }
    }

    fn mac(&self, name: &str, ver: &Ver, content: &Hash, ts: u64) -> Hash {
        let mut h = blake3::Hasher::new_keyed(&self.secret);
        h.update(name.as_bytes());
        h.update(&ver.pack());
        h.update(content);
        h.update(&ts.to_le_bytes());
        h.update(&self.id);
        *h.finalize().as_bytes()
    }

    /// Author (and stamp) a new artifact version under this identity.
    pub fn author(&self, name: &str, ver: Ver, content: Hash, ts: u64) -> VersionEntry {
        let sig = self.mac(name, &ver, &content, ts);
        VersionEntry { name: name.to_string(), ver, content, producer: self.id, ts, sig }
    }
}

/// One artifact version, authored by a node. The unit that gossips and converges.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionEntry {
    /// Logical artifact name (e.g. `"fluxc"`, `"flux-aether"`).
    pub name: String,
    /// Semantic version (the ③ layer).
    pub ver: Ver,
    /// BLAKE3 of the artifact bytes — the content commitment (≈ a state root).
    pub content: Hash,
    /// Authoring node's public id.
    pub producer: Hash,
    /// Logical/wall-clock timestamp of authorship (the last-writer key).
    pub ts: u64,
    /// Producer's blake3-MAC over the entry (SQIsign signature seam).
    pub sig: Hash,
}

impl VersionEntry {
    /// Stable fingerprint of the entry's *identity* (everything but the sig).
    /// Two nodes holding the same logical entry produce the same fingerprint.
    pub fn fingerprint(&self) -> Hash {
        let mut h = blake3::Hasher::new();
        h.update(self.name.as_bytes());
        h.update(&self.ver.pack());
        h.update(&self.content);
        h.update(&self.producer);
        h.update(&self.ts.to_le_bytes());
        *h.finalize().as_bytes()
    }

    /// **Conflict resolution** for two entries of the *same* artifact name:
    /// last-writer wins (higher `ts`), tie-broken by higher semver, then by
    /// larger content-hash. This is a *total order*, so every node converges to
    /// the identical winner regardless of the order entries arrive in.
    pub fn dominates(&self, other: &VersionEntry) -> bool {
        (self.ts, self.ver, self.content) > (other.ts, other.ver, other.content)
    }
}

/// A node's sovereign view of the mesh: the winning version per artifact.
/// An LWW-map CRDT — `merge` is commutative, associative, and idempotent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Winning entry per artifact name (BTreeMap → deterministic iteration).
    pub entries: BTreeMap<String, VersionEntry>,
}

impl Manifest {
    /// Empty manifest.
    pub fn new() -> Self {
        Self { entries: BTreeMap::new() }
    }

    /// Insert/replace an entry, keeping whichever [`VersionEntry::dominates`].
    /// Idempotent: re-applying the same or an older entry is a no-op.
    pub fn put(&mut self, e: VersionEntry) {
        match self.entries.get(&e.name) {
            Some(cur) if cur.dominates(&e) || *cur == e => {}
            _ => {
                self.entries.insert(e.name.clone(), e);
            }
        }
    }

    /// CRDT merge of a peer manifest into this one. Convergence guarantee:
    /// `a.merge(b); b.merge(a)` leaves `a == b` for any pair of states.
    pub fn merge(&mut self, other: &Manifest) {
        for e in other.entries.values() {
            self.put(e.clone());
        }
    }

    /// Compact gossip digest: `name -> entry fingerprint`. Peers diff digests to
    /// decide what to pull without shipping full entries first.
    pub fn digest(&self) -> BTreeMap<String, Hash> {
        self.entries.iter().map(|(k, e)| (k.clone(), e.fingerprint())).collect()
    }

    /// Single convergence hash over the whole manifest (sorted name+fingerprint).
    /// Two manifests are byte-identical iff their roots match → `divergence == 0`.
    pub fn root(&self) -> Hash {
        let mut h = blake3::Hasher::new();
        for (name, e) in &self.entries {
            h.update(name.as_bytes());
            h.update(&e.fingerprint());
        }
        *h.finalize().as_bytes()
    }

    /// Number of artifacts tracked.
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    /// Whether the manifest is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Collect the entries for a set of artifact names (the bytes a peer pulls).
    pub fn entries_for(&self, names: &[String]) -> Vec<VersionEntry> {
        names.iter().filter_map(|n| self.entries.get(n).cloned()).collect()
    }
}

/// The plan a node computes against a peer's digest: what to pull, what to offer.
/// This is the `AETHER_SYNC_PLAN` — server-less, symmetric, content-addressed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AetherSyncPlan {
    /// Artifact names this node should download from the peer (missing or differing).
    pub pull: Vec<String>,
    /// Artifact names this node holds that the peer lacks or has a different copy of.
    pub push: Vec<String>,
}

impl AetherSyncPlan {
    /// Nothing to exchange — the two sides are already in sync.
    pub fn is_noop(&self) -> bool {
        self.pull.is_empty() && self.push.is_empty()
    }
}

/// Compute the [`AetherSyncPlan`] for `local` against a peer's advertised digest.
/// Fingerprint mismatch (or absence) marks an artifact for transfer; the LWW
/// merge on the *download path* then resolves which copy actually wins.
pub fn plan(local: &Manifest, peer_digest: &BTreeMap<String, Hash>) -> AetherSyncPlan {
    let local_digest = local.digest();
    let mut pull = Vec::new();
    let mut push = Vec::new();
    for (name, pfp) in peer_digest {
        match local_digest.get(name) {
            Some(lfp) if lfp == pfp => {}
            _ => pull.push(name.clone()),
        }
    }
    for (name, lfp) in &local_digest {
        match peer_digest.get(name) {
            Some(pfp) if pfp == lfp => {}
            _ => push.push(name.clone()),
        }
    }
    pull.sort();
    push.sort();
    AetherSyncPlan { pull, push }
}

/// The **aether-download path**: apply pulled entries into the local manifest via
/// the LWW merge. Returns how many entries actually changed local state.
pub fn apply_download(local: &mut Manifest, incoming: &[VersionEntry]) -> usize {
    let before = local.root();
    let mut changed = 0usize;
    for e in incoming {
        let prev = local.entries.get(&e.name).cloned();
        local.put(e.clone());
        if local.entries.get(&e.name) != prev.as_ref() {
            changed += 1;
        }
    }
    let _ = before;
    changed
}

/// One full bidirectional gossip round between two sovereign nodes. After it
/// returns, both manifests are converged (`a.root() == b.root()`). This is the
/// live gossip-propagation primitive; over a real mesh it rides flux-p2p, but it
/// is transport-agnostic by design (pass the two states in).
pub fn sync_pair(a: &mut Manifest, b: &mut Manifest) {
    let pa = plan(a, &b.digest());
    let pb = plan(b, &a.digest());
    let from_b = b.entries_for(&pa.pull);
    let from_a = a.entries_for(&pb.pull);
    apply_download(a, &from_b);
    apply_download(b, &from_a);
}

/// Gossip until fixpoint over a set of nodes (each a `(name, Manifest)`),
/// excluding any whose name is in `partitioned`. Models live propagation across
/// the mesh: pairwise rounds repeat until no manifest changes (convergence).
/// Returns the number of rounds taken.
pub fn gossip_until_converged(nodes: &mut [(String, Manifest)], partitioned: &[&str]) -> usize {
    let live: Vec<usize> = (0..nodes.len())
        .filter(|&i| !partitioned.contains(&nodes[i].0.as_str()))
        .collect();
    let mut rounds = 0usize;
    loop {
        let before: Vec<Hash> = live.iter().map(|&i| nodes[i].1.root()).collect();
        // ring gossip: each live node syncs with the next live node
        for w in 0..live.len() {
            let i = live[w];
            let j = live[(w + 1) % live.len().max(1)];
            if i == j {
                continue;
            }
            let (lo, hi) = if i < j { (i, j) } else { (j, i) };
            let (left, right) = nodes.split_at_mut(hi);
            sync_pair(&mut left[lo].1, &mut right[0].1);
        }
        rounds += 1;
        let after: Vec<Hash> = live.iter().map(|&i| nodes[i].1.root()).collect();
        if before == after || rounds > 16 {
            break;
        }
    }
    rounds
}

/// Largest pairwise divergence across a node set: count of names whose winning
/// fingerprint is not identical on every node. `0` ⇒ fully converged.
pub fn divergence(nodes: &[(String, Manifest)]) -> usize {
    if nodes.is_empty() {
        return 0;
    }
    let mut names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for (_, m) in nodes {
        for k in m.entries.keys() {
            names.insert(k.clone());
        }
    }
    let mut diverged = 0usize;
    for name in &names {
        let mut fps: std::collections::BTreeSet<Option<Hash>> = std::collections::BTreeSet::new();
        for (_, m) in nodes {
            fps.insert(m.entries.get(name).map(|e| e.fingerprint()));
        }
        if fps.len() > 1 {
            diverged += 1;
        }
    }
    diverged
}

fn hex8(h: &Hash) -> String {
    h[..4].iter().map(|b| format!("{:02x}", b)).collect()
}

/// Render mesh sync-status as JSON for the `fluxc serve` SSE dashboard
/// (`/api/aether`). Hand-built (no serde_json dep) to keep the crate a leaf.
pub fn mesh_status_json(nodes: &[(String, Manifest)]) -> String {
    let div = divergence(nodes);
    let converged = div == 0;
    let node_json: Vec<String> = nodes
        .iter()
        .map(|(name, m)| {
            format!(
                r#"{{"name":"{}","root":"{}","artifacts":{}}}"#,
                name,
                hex8(&m.root()),
                m.len()
            )
        })
        .collect();
    // artifact roll-up from the first node's view (all converged nodes agree)
    let arts: Vec<String> = nodes
        .first()
        .map(|(_, m)| {
            m.entries
                .values()
                .map(|e| {
                    format!(
                        r#"{{"name":"{}","ver":"{}","producer":"{}","ts":{}}}"#,
                        e.name,
                        e.ver.display(),
                        hex8(&e.producer),
                        e.ts
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    format!(
        r#"{{"converged":{},"divergence":{},"nodes":[{}],"artifacts":[{}],"ts":{}}}"#,
        converged,
        div,
        node_json.join(","),
        arts.join(","),
        now_ms()
    )
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content(tag: &str) -> Hash {
        *blake3::hash(tag.as_bytes()).as_bytes()
    }

    #[test]
    fn lww_conflict_resolution_is_deterministic() {
        let eps = NodeIdentity::from_seed(b"epsilon");
        let old = eps.author("fluxc", Ver::new(0, 18, 0), content("a"), 100);
        let new = eps.author("fluxc", Ver::new(0, 18, 1), content("b"), 200);
        // last-writer (higher ts) wins regardless of insertion order
        let mut m1 = Manifest::new();
        m1.put(old.clone());
        m1.put(new.clone());
        let mut m2 = Manifest::new();
        m2.put(new.clone());
        m2.put(old.clone());
        assert_eq!(m1.entries["fluxc"], new);
        assert_eq!(m1, m2);
    }

    #[test]
    fn merge_is_commutative_and_idempotent() {
        let n = NodeIdentity::from_seed(b"n");
        let mut a = Manifest::new();
        a.put(n.author("x", Ver::new(1, 0, 0), content("x1"), 10));
        let mut b = Manifest::new();
        b.put(n.author("y", Ver::new(1, 0, 0), content("y1"), 11));
        let mut ab = a.clone();
        ab.merge(&b);
        let mut ba = b.clone();
        ba.merge(&a);
        assert_eq!(ab, ba); // commutative
        let mut ab2 = ab.clone();
        ab2.merge(&b);
        ab2.merge(&a);
        assert_eq!(ab, ab2); // idempotent
    }

    #[test]
    fn sync_plan_pulls_only_the_difference() {
        let n = NodeIdentity::from_seed(b"n");
        let mut a = Manifest::new();
        a.put(n.author("shared", Ver::new(1, 0, 0), content("s"), 1));
        let mut b = a.clone();
        b.put(n.author("only_b", Ver::new(1, 0, 0), content("ob"), 2));
        let p = plan(&a, &b.digest());
        assert_eq!(p.pull, vec!["only_b".to_string()]);
        assert!(p.push.is_empty());
    }

    #[test]
    fn pair_sync_converges() {
        let e = NodeIdentity::from_seed(b"epsilon");
        let d = NodeIdentity::from_seed(b"delta");
        let mut a = Manifest::new();
        a.put(e.author("fluxc", Ver::new(0, 18, 1), content("f1"), 100));
        let mut b = Manifest::new();
        b.put(d.author("flux-aether", Ver::new(0, 18, 0), content("ae"), 90));
        sync_pair(&mut a, &mut b);
        assert_eq!(a.root(), b.root());
        assert_eq!(a.len(), 2);
    }

    #[test]
    fn partition_then_reconverge_zero_divergence() {
        // 4-node mesh, all sovereign.
        let ids: Vec<NodeIdentity> =
            ["epsilon", "delta", "beta", "gamma"].iter().map(|n| NodeIdentity::from_seed(n.as_bytes())).collect();
        let eps = &ids[0];
        let mut nodes: Vec<(String, Manifest)> = ["epsilon", "delta", "beta", "gamma"]
            .iter()
            .map(|n| (n.to_string(), Manifest::new()))
            .collect();

        // genesis artifact, gossiped to everyone → converged
        let g = eps.author("fluxc", Ver::new(0, 18, 0), content("g"), 1);
        for (_, m) in nodes.iter_mut() {
            m.put(g.clone());
        }
        gossip_until_converged(&mut nodes, &[]);
        assert_eq!(divergence(&nodes), 0);

        // PARTITION: drop delta. epsilon bumps fluxc + adds a new artifact;
        // gossip only among {epsilon,beta,gamma}.
        nodes[0].1.put(eps.author("fluxc", Ver::new(0, 18, 1), content("h"), 200));
        nodes[0].1.put(eps.author("flux-sov", Ver::new(0, 1, 0), content("sov"), 201));
        gossip_until_converged(&mut nodes, &["delta"]);
        // delta is now behind → the mesh has diverged
        assert!(divergence(&nodes) > 0, "expected divergence during partition");
        let delta_root_before = nodes[1].1.root();
        assert_ne!(delta_root_before, nodes[0].1.root());

        // REJOIN delta → gossip with everyone → catch up
        let rounds = gossip_until_converged(&mut nodes, &[]);
        assert_eq!(divergence(&nodes), 0, "must re-converge after rejoin");
        // delta caught up to the bumped version + new artifact
        assert_eq!(nodes[1].1.entries["fluxc"].ver, Ver::new(0, 18, 1));
        assert!(nodes[1].1.entries.contains_key("flux-sov"));
        // every node holds the identical root
        let r0 = nodes[0].1.root();
        assert!(nodes.iter().all(|(_, m)| m.root() == r0));
        assert!(rounds >= 1);
    }
}
