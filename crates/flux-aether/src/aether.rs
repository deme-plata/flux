//! The shard / mix / reassemble core — files as blocks.

use serde::{Deserialize, Serialize};

/// 32-byte BLAKE3 digest.
pub type Hash = [u8; 32];

/// A file's manifest — **structured like a SIGIL block header** (the atomic unit).
///
/// `content_root` is to a file what a state root is to a block; `shard_merkle_root`
/// is to its shards what `txs_merkle_root` is to a block's transactions. Holding
/// this is "HERE"; it's the secret that ties the mixed shards back into a file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileBlock {
    /// Schema version.
    pub version: u8,
    /// BLAKE3 of the original file — the integrity commitment (≈ a state root).
    pub content_root: Hash,
    /// Merkle root over the shard CIDs (≈ `txs_merkle_root`).
    pub shard_merkle_root: Hash,
    /// Data shards (any K reconstruct).
    pub k: u32,
    /// Total shards (K data + parity).
    pub n: u32,
    /// Original byte length (shards are padded).
    pub len: u64,
    /// Bytes per shard.
    pub shard_size: u32,
    /// Per-file encryption nonce.
    pub nonce: Hash,
    /// Producer identity (the agent who sharded it; a provenance `.proof` +
    /// SQIsign sig bind here in AETHER-2, exactly like a block's producer).
    pub producer: Hash,
}

/// One mixed shard: encrypted bytes + its content-address. Indistinguishable
/// from any other shard in the pool — reveals nothing about its file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shard {
    /// Position in the erasure set (0..k = data, k = parity).
    pub index: u32,
    /// Is this the parity shard?
    pub is_parity: bool,
    /// Content id = BLAKE3 of the *encrypted* bytes (what peers store/address).
    pub cid: Hash,
    /// Encrypted shard bytes (high-entropy; reveal nothing without the key+nonce).
    pub bytes: Vec<u8>,
}

/// Errors.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AetherError {
    /// Fewer than K usable shards (single-parity recovers at most one loss; a
    /// general K-of-N erasure lane lifts this).
    InsufficientShards,
    /// Reassembled bytes don't hash to `content_root` — corrupt or wrong key.
    ContentMismatch,
}

fn derive_key(key: &[u8]) -> [u8; 32] {
    *blake3::hash(key).as_bytes()
}

/// BLAKE3-keystream XOR (a real stream cipher; AEAD is the prod upgrade).
fn keystream_xor(key32: &[u8; 32], nonce: &Hash, index: u32, buf: &mut [u8]) {
    let mut h = blake3::Hasher::new_keyed(key32);
    h.update(nonce);
    h.update(&index.to_le_bytes());
    let mut xof = h.finalize_xof();
    let mut ks = vec![0u8; buf.len()];
    xof.fill(&mut ks);
    for (b, k) in buf.iter_mut().zip(ks) {
        *b ^= k;
    }
}

fn merkle_root(leaves: &[Hash]) -> Hash {
    if leaves.is_empty() {
        return [0u8; 32];
    }
    let mut level: Vec<Hash> = leaves.to_vec();
    while level.len() > 1 {
        let mut next = Vec::with_capacity((level.len() + 1) / 2);
        for pair in level.chunks(2) {
            let mut h = blake3::Hasher::new();
            h.update(&pair[0]);
            h.update(pair.get(1).unwrap_or(&pair[0]));
            next.push(*h.finalize().as_bytes());
        }
        level = next;
    }
    level[0]
}

/// Shard a file into an erasure-coded, encrypted, content-addressed, MIXED set
/// + its block-structured [`FileBlock`]. K data shards + 1 XOR parity (recover
/// any single loss); general K-of-N is the reed-solomon lane.
pub fn shard_file(data: &[u8], shard_size: usize, key: &[u8], producer: Hash) -> (FileBlock, Vec<Shard>) {
    let shard_size = shard_size.max(1);
    let len = data.len();
    let content_root = *blake3::hash(data).as_bytes();
    let k = ((len + shard_size - 1) / shard_size).max(1);

    // Pad to k*shard_size, split into k data shards.
    let mut padded = data.to_vec();
    padded.resize(k * shard_size, 0);
    let mut plain: Vec<Vec<u8>> = (0..k).map(|i| padded[i * shard_size..(i + 1) * shard_size].to_vec()).collect();

    // Parity = XOR of all data shards.
    let mut parity = vec![0u8; shard_size];
    for ds in &plain {
        for (p, d) in parity.iter_mut().zip(ds) {
            *p ^= *d;
        }
    }
    plain.push(parity);
    let n = plain.len();

    // Deterministic per-file nonce (from content + len).
    let mut nh = blake3::Hasher::new();
    nh.update(&content_root);
    nh.update(&(len as u64).to_le_bytes());
    let nonce = *nh.finalize().as_bytes();
    let key32 = derive_key(key);

    let mut shards = Vec::with_capacity(n);
    let mut cids = Vec::with_capacity(n);
    for (i, mut buf) in plain.into_iter().enumerate() {
        keystream_xor(&key32, &nonce, i as u32, &mut buf);
        let cid = *blake3::hash(&buf).as_bytes();
        cids.push(cid);
        shards.push(Shard { index: i as u32, is_parity: i == k, cid, bytes: buf });
    }

    let fb = FileBlock {
        version: 1,
        content_root,
        shard_merkle_root: merkle_root(&cids),
        k: k as u32,
        n: n as u32,
        len: len as u64,
        shard_size: shard_size as u32,
        nonce,
        producer,
    };
    (fb, shards)
}

/// Reassemble the file from any K usable shards. Recovers a single missing data
/// shard via parity, then VERIFIES the bytes hash to `content_root` (the same
/// integrity gate a block's state-root check is).
pub fn reassemble(fb: &FileBlock, available: &[Shard], key: &[u8]) -> Result<Vec<u8>, AetherError> {
    let ss = fb.shard_size as usize;
    let key32 = derive_key(key);
    let mut data_shards: Vec<Option<Vec<u8>>> = vec![None; fb.k as usize];
    let mut parity: Option<Vec<u8>> = None;

    for s in available {
        let mut buf = s.bytes.clone();
        keystream_xor(&key32, &fb.nonce, s.index, &mut buf);
        if s.index < fb.k {
            data_shards[s.index as usize] = Some(buf);
        } else {
            parity = Some(buf);
        }
    }

    let missing: Vec<usize> = (0..fb.k as usize).filter(|&i| data_shards[i].is_none()).collect();
    if missing.len() == 1 {
        let par = parity.as_ref().ok_or(AetherError::InsufficientShards)?;
        let mut rec = par.clone();
        for ds in data_shards.iter().flatten() {
            for (r, d) in rec.iter_mut().zip(ds) {
                *r ^= *d;
            }
        }
        data_shards[missing[0]] = Some(rec);
    } else if missing.len() > 1 {
        return Err(AetherError::InsufficientShards);
    }

    let mut out = Vec::with_capacity(fb.len as usize);
    for i in 0..fb.k as usize {
        out.extend_from_slice(data_shards[i].as_ref().ok_or(AetherError::InsufficientShards)?);
        let _ = ss;
    }
    out.truncate(fb.len as usize);
    if *blake3::hash(&out).as_bytes() != fb.content_root {
        return Err(AetherError::ContentMismatch);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio(seed: u8, n: usize) -> Vec<u8> {
        // a fake "WAV" payload — RIFF header + pseudo samples.
        let mut v = b"RIFF....WAVEfmt ".to_vec();
        for i in 0..n {
            v.push(((i as u8).wrapping_mul(31).wrapping_add(seed)) ^ 0x5a);
        }
        v
    }

    #[test]
    fn round_trip_all_shards() {
        let data = audio(7, 5000);
        let (fb, shards) = shard_file(&data, 1024, b"my-key", [0xAB; 32]);
        let back = reassemble(&fb, &shards, b"my-key").unwrap();
        assert_eq!(back, data);
    }

    #[test]
    fn reassembles_from_subset_here_and_there() {
        // Drop one data shard — peers "here and there" still reconstruct it.
        let data = audio(3, 4096);
        let (fb, mut shards) = shard_file(&data, 1024, b"k", [0; 32]);
        shards.remove(0); // a peer went offline
        let back = reassemble(&fb, &shards, b"k").unwrap();
        assert_eq!(back, data);
    }

    #[test]
    fn shard_reveals_nothing_the_mixer_property() {
        let data = audio(1, 2048);
        let (_fb, shards) = shard_file(&data, 512, b"secret", [0; 32]);
        // No shard's ciphertext contains the plaintext RIFF marker.
        for s in &shards {
            assert!(
                !s.bytes.windows(4).any(|w| w == b"RIFF"),
                "encrypted shard leaked plaintext"
            );
        }
        // Wrong key → cannot reconstruct (content gate fails).
        let (fb2, sh2) = shard_file(&data, 512, b"secret", [0; 32]);
        assert!(matches!(reassemble(&fb2, &sh2, b"wrong-key"), Err(AetherError::ContentMismatch)));
    }

    #[test]
    fn file_has_block_atomic_structure() {
        let data = audio(9, 3000);
        let (fb, _s) = shard_file(&data, 1024, b"k", [0; 32]);
        // Two distinct committed roots, like a SIGIL block's content + tx roots.
        assert_ne!(fb.content_root, [0u8; 32]);
        assert_ne!(fb.shard_merkle_root, [0u8; 32]);
        assert_ne!(fb.content_root, fb.shard_merkle_root);
        assert_eq!(fb.n, fb.k + 1); // k data + parity
    }

    #[test]
    fn deterministic_cids() {
        let data = audio(5, 1500);
        let a = shard_file(&data, 256, b"k", [0; 32]).1;
        let b = shard_file(&data, 256, b"k", [0; 32]).1;
        assert_eq!(a.iter().map(|s| s.cid).collect::<Vec<_>>(), b.iter().map(|s| s.cid).collect::<Vec<_>>());
    }
}
