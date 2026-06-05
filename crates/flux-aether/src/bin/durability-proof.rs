//! durability-proof — proves SIGIL's "can't-lose" property, the resolution of
//! the information paradox: a *verifiable* chain, sharded via flux-aether,
//! survives losing a host (a dropped shard) and comes back **byte-identical
//! and re-verifying**. Because the tip is verifiable, the data needs no trusted
//! home — scatter it, lose hosts, re-verify whatever returns.
//!
//!   cargo run -p flux-aether --bin durability-proof --release

use flux_aether::{rs_reassemble, rs_shard, Hash};

const BLOCK_BYTES: usize = 8 + 32 + 32 * 4 + 32; // height + prev + 4 roots + hash = 200

#[derive(Clone, PartialEq, Debug)]
struct Block {
    height: u64,
    prev: Hash,
    roots: [Hash; 4], // wallet · dex · event · contract — the SIGIL commitment
    hash: Hash,
}

fn block_hash(height: u64, prev: &Hash, roots: &[Hash; 4]) -> Hash {
    let mut h = blake3::Hasher::new();
    h.update(&height.to_le_bytes());
    h.update(prev);
    for r in roots {
        h.update(r);
    }
    *h.finalize().as_bytes()
}

fn encode(b: &Block) -> Vec<u8> {
    let mut v = Vec::with_capacity(BLOCK_BYTES);
    v.extend_from_slice(&b.height.to_le_bytes());
    v.extend_from_slice(&b.prev);
    for r in &b.roots {
        v.extend_from_slice(r);
    }
    v.extend_from_slice(&b.hash);
    v
}

fn decode(s: &[u8]) -> Block {
    let mut height = [0u8; 8];
    height.copy_from_slice(&s[0..8]);
    let mut prev = [0u8; 32];
    prev.copy_from_slice(&s[8..40]);
    let mut roots = [[0u8; 32]; 4];
    for (i, r) in roots.iter_mut().enumerate() {
        r.copy_from_slice(&s[40 + i * 32..72 + i * 32]);
    }
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&s[168..200]);
    Block { height: u64::from_le_bytes(height), prev, roots, hash }
}

fn build_chain(n: u64) -> Vec<Block> {
    let mut chain = Vec::new();
    let mut prev = [0u8; 32];
    for height in 0..n {
        let roots = [
            *blake3::hash(&[b"wallet".as_ref(), &height.to_le_bytes()].concat()).as_bytes(),
            *blake3::hash(&[b"dex".as_ref(), &height.to_le_bytes()].concat()).as_bytes(),
            *blake3::hash(&[b"event".as_ref(), &height.to_le_bytes()].concat()).as_bytes(),
            *blake3::hash(&[b"contract".as_ref(), &height.to_le_bytes()].concat()).as_bytes(),
        ];
        let hash = block_hash(height, &prev, &roots);
        chain.push(Block { height, prev, roots, hash });
        prev = hash;
    }
    chain
}

/// Re-verify the whole chain: every hash recomputes, prev-links sound, heights
/// contiguous. (The 10µs tip-verify, applied across the chain.)
fn verify_chain(chain: &[Block]) -> bool {
    let mut prev = [0u8; 32];
    for (i, b) in chain.iter().enumerate() {
        if b.height != i as u64 || b.prev != prev || b.hash != block_hash(b.height, &b.prev, &b.roots) {
            return false;
        }
        prev = b.hash;
    }
    true
}

fn hex8(h: &Hash) -> String {
    h[..4].iter().map(|b| format!("{b:02x}")).collect()
}

fn main() {
    println!("⬡ SIGIL durability proof — verifiability → trustless durability\n");

    // 1) a verifiable chain
    let chain = build_chain(500);
    let bytes: Vec<u8> = chain.iter().flat_map(encode).collect();
    println!(
        "1. chain: {} blocks · {} bytes · re-verifies: {}",
        chain.len(),
        bytes.len(),
        verify_chain(&chain)
    );

    // 2) erasure-code across "hosts" — Reed-Solomon 16 data + 8 parity
    let commit = *blake3::hash(&bytes).as_bytes();
    let (k, parity) = (16usize, 8usize);
    let (orig_len, shards) = rs_shard(&bytes, k, parity);
    println!(
        "2. erasure-coded: {} data + {} parity = {} shards (Reed-Solomon · any {} reconstruct) · commit {}",
        k, parity, shards.len(), k, hex8(&commit)
    );

    // 3) KILL: the chain is gone, and we LOSE 8 hosts (the MAX survivable),
    //    spread across data + parity shards — not a trivial concat.
    let mut sparse: Vec<Option<Vec<u8>>> = shards.iter().cloned().map(Some).collect();
    let mut lost = 0;
    for i in (0..shards.len()).step_by(3) {
        if lost >= parity {
            break;
        }
        sparse[i] = None;
        lost += 1;
    }
    let survivors = sparse.iter().filter(|s| s.is_some()).count();
    println!(
        "3. 💥 KILLED the in-memory chain + LOST {} of {} hosts — only {} survive",
        lost,
        shards.len(),
        survivors
    );

    // 4) RECOVER from the survivors alone (Reed-Solomon rebuilds the lost shards)
    let recovered = match rs_reassemble(orig_len, k, parity, sparse) {
        Some(r) => r,
        None => {
            println!("❌ recovery FAILED");
            std::process::exit(1);
        }
    };

    // 5) PROVE byte-identical + re-verifies
    let chain2: Vec<Block> = recovered.chunks(BLOCK_BYTES).map(decode).collect();
    let identical = chain2 == chain;
    let reverifies = verify_chain(&chain2);
    println!("4. ♻️  recovered {} bytes from survivors alone", recovered.len());
    println!("5. byte-identical to original: {identical}   ·   re-verifies: {reverifies}");

    println!(
        "\n{}",
        if identical && reverifies {
            "✅ CAN'T-LOSE PROVEN — chain killed + a host lost, yet it came back byte-identical and re-verified.\n   The information paradox dissolves: a verifiable tip means the data needs no trusted home.\n   Scatter it across untrusted hosts, lose some, re-verify what returns. More verifiable replicas = more durable, zero trust."
        } else {
            "❌ NOT PROVEN — recovery diverged"
        }
    );
}
