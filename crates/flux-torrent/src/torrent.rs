//! The aether ↔ torrent mapping.
use flux_aether::{FileBlock, Hash, Shard};
use serde::{Deserialize, Serialize};

/// A torrent derived from a FileBlock — the magnet/info that lets a peer find +
/// verify the encrypted shards in the swarm. The info_hash commits the shard
/// set (it IS the FileBlock's shard_merkle_root).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorrentInfo {
    /// = FileBlock.shard_merkle_root — the swarm key + integrity commitment.
    pub info_hash: Hash,
    /// = FileBlock.content_root — verifies the file after decrypt+reassemble.
    pub content_root: Hash,
    /// Piece (shard) size.
    pub piece_len: u32,
    /// Number of data pieces needed to reconstruct (K-of-N).
    pub k: u32,
    /// Total pieces (data + parity).
    pub n: u32,
    /// Original file length.
    pub len: u64,
}

impl TorrentInfo {
    /// Derive the torrent metadata from a FileBlock.
    pub fn from_file_block(fb: &FileBlock) -> Self {
        TorrentInfo {
            info_hash: fb.shard_merkle_root,
            content_root: fb.content_root,
            piece_len: fb.shard_size,
            k: fb.k,
            n: fb.n,
            len: fb.len,
        }
    }
}

fn hex(h: &Hash) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

/// A magnet URI for the torrent: the info_hash (swarm key) + the content_root
/// (final integrity) + erasure params. Holding this is "HERE".
pub fn magnet(t: &TorrentInfo) -> String {
    format!(
        "magnet:?xt=urn:flux-aether:{}&cr={}&pl={}&k={}&n={}&xl={}",
        hex(&t.info_hash), hex(&t.content_root), t.piece_len, t.k, t.n, t.len
    )
}

/// Parse a flux-aether magnet back into TorrentInfo (best-effort).
pub fn parse_magnet(uri: &str) -> Option<TorrentInfo> {
    let q = uri.strip_prefix("magnet:?")?;
    let mut info_hash = [0u8; 32];
    let mut content_root = [0u8; 32];
    let (mut piece_len, mut k, mut n, mut len) = (0u32, 0u32, 0u32, 0u64);
    for kv in q.split('&') {
        let (key, val) = kv.split_once('=')?;
        match key {
            "xt" => { unhex(val.strip_prefix("urn:flux-aether:").unwrap_or(val), &mut info_hash); }
            "cr" => { unhex(val, &mut content_root); }
            "pl" => piece_len = val.parse().ok()?,
            "k" => k = val.parse().ok()?,
            "n" => n = val.parse().ok()?,
            "xl" => len = val.parse().ok()?,
            _ => {}
        }
    }
    Some(TorrentInfo { info_hash, content_root, piece_len, k, n, len })
}

fn unhex(s: &str, out: &mut [u8; 32]) {
    for (i, b) in out.iter_mut().enumerate() {
        if let Some(h) = s.get(i * 2..i * 2 + 2) {
            *b = u8::from_str_radix(h, 16).unwrap_or(0);
        }
    }
}

/// The piece content-ids (shard CIDs) a peer must fetch from the swarm. The
/// swarm carries only these encrypted, indistinguishable pieces.
pub fn piece_cids(shards: &[Shard]) -> Vec<Hash> {
    shards.iter().map(|s| s.cid).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux_aether::shard_file;

    #[test]
    fn fileblock_becomes_a_torrent_and_back() {
        let data = b"RIFF....WAVE fake audio payload for the swarm".repeat(64);
        let (fb, shards) = shard_file(&data, 256, b"key", [0; 32]);
        let t = TorrentInfo::from_file_block(&fb);
        assert_eq!(t.info_hash, fb.shard_merkle_root);
        assert_eq!(t.content_root, fb.content_root);
        // magnet round-trips.
        let m = magnet(&t);
        assert!(m.starts_with("magnet:?xt=urn:flux-aether:"));
        let t2 = parse_magnet(&m).unwrap();
        assert_eq!(t, t2);
        // pieces are the content-addressed shards.
        assert_eq!(piece_cids(&shards).len(), fb.n as usize);
    }

    #[test]
    fn swarm_only_carries_ciphertext() {
        let data = b"secret song bytes RIFF".repeat(32);
        let (_fb, shards) = shard_file(&data, 128, b"k", [0; 32]);
        // every piece (what the torrent swarm holds) is encrypted — no plaintext.
        for s in &shards {
            assert!(!s.bytes.windows(4).any(|w| w == b"RIFF"));
        }
    }
}
