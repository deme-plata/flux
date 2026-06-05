//! rs.rs — real Reed-Solomon K-of-N erasure. Lose up to `N-K` shards (hosts)
//! and still reconstruct, vs the v1 single-parity (lose 1). The "wider blast
//! radius" durability dial: 16+8 means any 8 of 24 hosts can vanish.

use reed_solomon_erasure::galois_8::ReedSolomon;

/// Erasure-code `data` into `k` data + `parity` parity shards (all equal length).
/// Returns `(orig_len, shards)`. Any `k` of the `k+parity` shards reconstruct.
pub fn rs_shard(data: &[u8], k: usize, parity: usize) -> (usize, Vec<Vec<u8>>) {
    let shard_len = ((data.len() + k - 1) / k).max(1);
    let mut shards: Vec<Vec<u8>> = (0..k)
        .map(|i| {
            let start = i * shard_len;
            let end = ((i + 1) * shard_len).min(data.len());
            let mut s = vec![0u8; shard_len];
            if start < end {
                s[..end - start].copy_from_slice(&data[start..end]);
            }
            s
        })
        .collect();
    shards.extend((0..parity).map(|_| vec![0u8; shard_len]));
    let rs = ReedSolomon::new(k, parity).expect("valid rs params");
    rs.encode(&mut shards).expect("rs encode");
    (data.len(), shards)
}

/// Reconstruct from a sparse shard set (`None` = lost host). Succeeds iff at
/// least `k` of the `k+parity` shards survive. Returns the original `data`.
pub fn rs_reassemble(
    orig_len: usize,
    k: usize,
    parity: usize,
    mut shards: Vec<Option<Vec<u8>>>,
) -> Option<Vec<u8>> {
    let rs = ReedSolomon::new(k, parity).ok()?;
    rs.reconstruct(&mut shards).ok()?;
    let mut out = Vec::with_capacity(orig_len);
    for s in shards.iter().take(k) {
        out.extend_from_slice(s.as_ref()?);
    }
    out.truncate(orig_len);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loses_up_to_parity_and_recovers() {
        let data: Vec<u8> = (0..5000u32).map(|i| (i % 256) as u8).collect();
        let (len, shards) = rs_shard(&data, 16, 8); // 24 shards, tolerate losing 8
        assert_eq!(shards.len(), 24);
        // drop the maximum survivable: 8 shards
        let sparse: Vec<Option<Vec<u8>>> = shards
            .iter()
            .enumerate()
            .map(|(i, s)| if i < 8 { None } else { Some(s.clone()) })
            .collect();
        let rec = rs_reassemble(len, 16, 8, sparse).expect("recover from 16 survivors");
        assert_eq!(rec, data);
    }

    #[test]
    fn fails_past_parity() {
        let data = vec![7u8; 1000];
        let (len, shards) = rs_shard(&data, 4, 2); // tolerate losing 2
        let sparse: Vec<Option<Vec<u8>>> = shards
            .iter()
            .enumerate()
            .map(|(i, s)| if i < 3 { None } else { Some(s.clone()) }) // lose 3 > 2
            .collect();
        assert!(rs_reassemble(len, 4, 2, sparse).is_none());
    }
}
