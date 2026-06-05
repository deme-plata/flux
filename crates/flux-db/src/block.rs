//! Block-based SST data and index blocks.
//!
//! v0.12 replaces the monolithic LZ4 blob of v0.11 with RocksDB-style
//! block format:
//!
//!   ┌─────────────────────────────────────────────────────────┐
//!   │ data block 0   (~4 KiB lz4-compressed key/value pairs)  │
//!   │ data block 1                                            │
//!   │ ...                                                     │
//!   │ data block N-1                                          │
//!   │ index block                                             │
//!   │ footer: index_offset (u64), index_len (u32), magic (4)  │
//!   └─────────────────────────────────────────────────────────┘
//!
//! Why: pre-v0.12 a single `get()` read + decompressed the entire SST,
//! which on a 100 MB SST is two-to-three orders of magnitude slower than
//! decompressing just the relevant 4 KB block. The index block lets us
//! binary-search through last-keys to find the right data block in O(log
//! n_blocks), and the LRU block cache (see `cache.rs`) keeps recently-used
//! decompressed blocks in memory.
//!
//! Block layout (within one data block, uncompressed):
//!   for each entry:
//!     [key_len: u32 LE][val_len: u32 LE][key bytes][val bytes]
//!
//! Index block layout (uncompressed):
//!   [n_entries: u32 LE]
//!   for each block:
//!     [last_key_len: u32][last_key][block_offset: u64][compressed_len: u32]
//!         [uncompressed_len: u32]
//!
//! The footer is fixed 16 bytes: u64 index_offset, u32 index_len, u32
//! repeating BLK_MAGIC for sanity.

use std::io::Write;

pub const BLK_MAGIC: u32 = 0x424c_4b46; // "BLKF"
pub const TARGET_BLOCK_SIZE: usize = 4096;
pub const FOOTER_LEN: usize = 16;

/// One on-disk block descriptor — what the index points at.
#[derive(Debug, Clone)]
pub struct BlockHandle {
    pub offset: u64,
    pub compressed_len: u32,
    pub uncompressed_len: u32,
    pub last_key: Vec<u8>,
}

/// Builds a sequence of data blocks. Call `add(k, v)` for each entry in
/// sorted order; the builder flushes a new block whenever its in-progress
/// uncompressed buffer crosses TARGET_BLOCK_SIZE. `finish(...)` writes the
/// final index block + footer and returns the list of handles.
pub struct SstBuilder {
    cur_block: Vec<u8>,
    last_key_in_cur: Vec<u8>,
    blocks: Vec<BlockHandle>,
    /// Compressed payload + index. Caller owns the final SST framing
    /// (header, bloom, footer), so this builder only emits the bytes
    /// starting from block 0.
    output: Vec<u8>,
}

impl SstBuilder {
    pub fn new() -> Self {
        Self {
            cur_block: Vec::with_capacity(TARGET_BLOCK_SIZE * 2),
            last_key_in_cur: Vec::new(),
            blocks: Vec::new(),
            output: Vec::new(),
        }
    }

    pub fn add(&mut self, key: &[u8], value: &[u8]) {
        self.cur_block.extend_from_slice(&(key.len() as u32).to_le_bytes());
        self.cur_block.extend_from_slice(&(value.len() as u32).to_le_bytes());
        self.cur_block.extend_from_slice(key);
        self.cur_block.extend_from_slice(value);
        self.last_key_in_cur = key.to_vec();
        if self.cur_block.len() >= TARGET_BLOCK_SIZE {
            self.flush_cur_block();
        }
    }

    /// Force-flush the current in-progress block as compressed bytes into
    /// `output`, and record a BlockHandle.
    fn flush_cur_block(&mut self) {
        if self.cur_block.is_empty() {
            return;
        }
        let uncompressed_len = self.cur_block.len() as u32;
        let compressed = lz4::block::compress(
            &self.cur_block,
            Some(lz4::block::CompressionMode::FAST(1)),
            true,
        )
        .unwrap_or_else(|_| self.cur_block.clone());

        let offset = self.output.len() as u64;
        let compressed_len = compressed.len() as u32;
        self.output.extend_from_slice(&compressed);

        self.blocks.push(BlockHandle {
            offset,
            compressed_len,
            uncompressed_len,
            last_key: std::mem::take(&mut self.last_key_in_cur),
        });
        self.cur_block.clear();
    }

    /// Flush any in-progress block, then write the index block + footer.
    /// Returns the full SST body bytes (after the SST header/bloom that the
    /// caller will prepend).
    pub fn finish(mut self) -> Vec<u8> {
        self.flush_cur_block();

        // Build the index block.
        let mut index = Vec::new();
        index.extend_from_slice(&(self.blocks.len() as u32).to_le_bytes());
        for h in &self.blocks {
            index.extend_from_slice(&(h.last_key.len() as u32).to_le_bytes());
            index.extend_from_slice(&h.last_key);
            index.extend_from_slice(&h.offset.to_le_bytes());
            index.extend_from_slice(&h.compressed_len.to_le_bytes());
            index.extend_from_slice(&h.uncompressed_len.to_le_bytes());
        }

        let index_offset = self.output.len() as u64;
        let index_len = index.len() as u32;
        self.output.extend_from_slice(&index);

        // Footer.
        self.output.extend_from_slice(&index_offset.to_le_bytes());
        self.output.extend_from_slice(&index_len.to_le_bytes());
        self.output.extend_from_slice(&BLK_MAGIC.to_le_bytes());

        self.output
    }

    pub fn is_empty(&self) -> bool {
        self.cur_block.is_empty() && self.blocks.is_empty()
    }
}

/// Parsed footer of a block-based SST body. The body is everything after
/// the v0.10 SST header (magic+version+key_count+bloom).
pub struct BlockSstReader<'a> {
    body: &'a [u8],
    pub index: Vec<BlockHandle>,
}

impl<'a> BlockSstReader<'a> {
    pub fn new(body: &'a [u8]) -> Result<Self, String> {
        if body.len() < FOOTER_LEN {
            return Err(format!("body too short for footer: {}", body.len()));
        }
        let footer_start = body.len() - FOOTER_LEN;
        let index_offset = u64::from_le_bytes(
            body[footer_start..footer_start + 8].try_into().unwrap(),
        ) as usize;
        let index_len = u32::from_le_bytes(
            body[footer_start + 8..footer_start + 12].try_into().unwrap(),
        ) as usize;
        let magic = u32::from_le_bytes(
            body[footer_start + 12..footer_start + 16].try_into().unwrap(),
        );
        if magic != BLK_MAGIC {
            return Err(format!("bad block-SST footer magic 0x{magic:08x}"));
        }
        if index_offset + index_len > footer_start {
            return Err("index range overruns into footer".into());
        }
        let index_bytes = &body[index_offset..index_offset + index_len];
        let index = Self::parse_index(index_bytes)?;
        Ok(Self { body, index })
    }

    fn parse_index(buf: &[u8]) -> Result<Vec<BlockHandle>, String> {
        if buf.len() < 4 {
            return Err("index too short".into());
        }
        let n = u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize;
        let mut out: Vec<BlockHandle> = Vec::with_capacity(n);
        let mut pos = 4;
        for _ in 0..n {
            if pos + 4 > buf.len() {
                return Err("index truncated at key_len".into());
            }
            let klen = u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 4;
            if pos + klen + 16 > buf.len() {
                return Err("index truncated at body".into());
            }
            let last_key = buf[pos..pos + klen].to_vec();
            pos += klen;
            let offset = u64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap());
            pos += 8;
            let compressed_len = u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap());
            pos += 4;
            let uncompressed_len = u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap());
            pos += 4;
            out.push(BlockHandle {
                offset,
                compressed_len,
                uncompressed_len,
                last_key,
            });
        }
        Ok(out)
    }

    /// Find the block whose `last_key >= key`. None if key is past the end.
    pub fn locate_block(&self, key: &[u8]) -> Option<&BlockHandle> {
        self.index.iter().find(|h| h.last_key.as_slice() >= key)
    }

    /// Read + decompress one data block by handle. Does NOT consult the
    /// block cache — see `cache.rs` for that wrapper.
    pub fn read_block(&self, handle: &BlockHandle) -> Result<Vec<u8>, String> {
        let start = handle.offset as usize;
        let end = start + handle.compressed_len as usize;
        if end > self.body.len() {
            return Err("block compressed range outside body".into());
        }
        let compressed = &self.body[start..end];
        let decompressed = lz4::block::decompress(compressed, None)
            .map_err(|e| format!("decompress block: {}", e))?;
        if decompressed.len() != handle.uncompressed_len as usize {
            return Err(format!(
                "block decompress size mismatch: expected {}, got {}",
                handle.uncompressed_len,
                decompressed.len()
            ));
        }
        Ok(decompressed)
    }

    /// Walk one decompressed block and return the value for `key`, if present.
    pub fn lookup_in_block(decompressed: &[u8], key: &[u8]) -> Option<Vec<u8>> {
        let mut pos = 0;
        while pos + 8 <= decompressed.len() {
            let klen = u32::from_le_bytes(decompressed[pos..pos + 4].try_into().unwrap()) as usize;
            let vlen = u32::from_le_bytes(decompressed[pos + 4..pos + 8].try_into().unwrap()) as usize;
            pos += 8;
            if pos + klen + vlen > decompressed.len() {
                return None;
            }
            if &decompressed[pos..pos + klen] == key {
                return Some(decompressed[pos + klen..pos + klen + vlen].to_vec());
            }
            pos += klen + vlen;
        }
        None
    }

    /// Iterate ALL (key, value) pairs across all blocks. Used by compaction.
    pub fn pairs(&self) -> Result<Vec<(Vec<u8>, Vec<u8>)>, String> {
        let mut out = Vec::new();
        for h in &self.index {
            let decompressed = self.read_block(h)?;
            let mut pos = 0;
            while pos + 8 <= decompressed.len() {
                let klen = u32::from_le_bytes(decompressed[pos..pos + 4].try_into().unwrap()) as usize;
                let vlen = u32::from_le_bytes(decompressed[pos + 4..pos + 8].try_into().unwrap()) as usize;
                pos += 8;
                if pos + klen + vlen > decompressed.len() {
                    return Err("malformed block payload".into());
                }
                out.push((
                    decompressed[pos..pos + klen].to_vec(),
                    decompressed[pos + klen..pos + klen + vlen].to_vec(),
                ));
                pos += klen + vlen;
            }
        }
        Ok(out)
    }
}

/// Convenience: write a complete block-based SST body (data + index +
/// footer) from a sorted iterator of (key, value).
pub fn build_block_sst<I, K, V>(pairs: I) -> Vec<u8>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<[u8]>,
    V: AsRef<[u8]>,
{
    let mut b = SstBuilder::new();
    for (k, v) in pairs {
        b.add(k.as_ref(), v.as_ref());
    }
    b.finish()
}

/// Unused but kept so adding a builder method in v0.13 doesn't require
/// touching the trait import.
fn _writer_marker(_w: &mut dyn Write) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_block_roundtrip() {
        let pairs: Vec<(&[u8], &[u8])> = vec![
            (b"alpha", b"1"),
            (b"bravo", b"2"),
            (b"charlie", b"3"),
        ];
        let body = build_block_sst(pairs);
        let r = BlockSstReader::new(&body).unwrap();
        assert_eq!(r.index.len(), 1, "small input fits one block");
        // get path: locate → read → lookup
        for (k, v) in [("alpha", "1"), ("bravo", "2"), ("charlie", "3")] {
            let h = r.locate_block(k.as_bytes()).unwrap();
            let block = r.read_block(h).unwrap();
            assert_eq!(
                BlockSstReader::lookup_in_block(&block, k.as_bytes()),
                Some(v.as_bytes().to_vec()),
                "key {k} should resolve to {v}"
            );
        }
        // absent key
        let h = r.locate_block(b"alpha").unwrap();
        let block = r.read_block(h).unwrap();
        assert_eq!(BlockSstReader::lookup_in_block(&block, b"missing"), None);
    }

    #[test]
    fn multi_block_spans_and_pairs_are_ordered() {
        // 64 entries with ~256-byte values forces several 4 KiB blocks.
        let mut pairs: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        for i in 0..64u32 {
            pairs.push((format!("key{i:04}").into_bytes(), vec![b'v'; 256]));
        }
        let body = build_block_sst(pairs.iter().map(|(k, v)| (k.clone(), v.clone())));
        let r = BlockSstReader::new(&body).unwrap();
        assert!(r.index.len() > 1, "256B*64 should not fit in one 4KiB block");

        // pairs() returns every entry, in insertion (sorted) order.
        let all = r.pairs().unwrap();
        assert_eq!(all.len(), 64);
        assert_eq!(all[0].0, b"key0000");
        assert_eq!(all[63].0, b"key0063");

        // a mid-range key resolves through the located block
        let target = b"key0033";
        let h = r.locate_block(target).unwrap();
        let block = r.read_block(h).unwrap();
        assert_eq!(
            BlockSstReader::lookup_in_block(&block, target),
            Some(vec![b'v'; 256])
        );

        // a key past the end has no block
        assert!(r.locate_block(b"zzzz").is_none());
    }

    #[test]
    fn empty_builder_finish_is_parseable() {
        let body = SstBuilder::new().finish();
        let r = BlockSstReader::new(&body).unwrap();
        assert_eq!(r.index.len(), 0);
        assert!(r.pairs().unwrap().is_empty());
        assert!(r.locate_block(b"anything").is_none());
    }

    #[test]
    fn reader_rejects_short_body() {
        // Anything shorter than the 16-byte footer must error, not panic.
        for len in 0..FOOTER_LEN {
            assert!(BlockSstReader::new(&vec![0u8; len]).is_err());
        }
    }

    #[test]
    fn reader_rejects_bad_footer_magic() {
        let mut body = build_block_sst(vec![(&b"k"[..], &b"v"[..])]);
        let n = body.len();
        // Corrupt the trailing 4 magic bytes.
        body[n - 4..].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        let err = BlockSstReader::new(&body)
            .err()
            .expect("expected bad-magic error");
        assert!(err.contains("magic"), "got: {err}");
    }

    #[test]
    fn reader_rejects_index_overrun() {
        // Valid magic, but an index_offset/len that overruns the footer.
        let mut body = build_block_sst(vec![(&b"k"[..], &b"v"[..])]);
        let n = body.len();
        let footer_start = n - FOOTER_LEN;
        // index_offset = 0, index_len = huge → overruns footer_start
        body[footer_start..footer_start + 8].copy_from_slice(&0u64.to_le_bytes());
        body[footer_start + 8..footer_start + 12]
            .copy_from_slice(&(footer_start as u32 + 1).to_le_bytes());
        assert!(BlockSstReader::new(&body).is_err());
    }

    #[test]
    fn read_block_rejects_out_of_range_handle() {
        let body = build_block_sst(vec![(&b"k"[..], &b"v"[..])]);
        let r = BlockSstReader::new(&body).unwrap();
        let bad = BlockHandle {
            offset: body.len() as u64, // past the end
            compressed_len: 32,
            uncompressed_len: 32,
            last_key: b"k".to_vec(),
        };
        assert!(r.read_block(&bad).is_err());
    }

    #[test]
    fn lookup_in_block_tolerates_truncated_payload() {
        // A torn block (declared lengths exceed buffer) must return None, not panic.
        let mut buf = Vec::new();
        buf.extend_from_slice(&3u32.to_le_bytes()); // klen = 3
        buf.extend_from_slice(&99u32.to_le_bytes()); // vlen = 99 (way past end)
        buf.extend_from_slice(b"key"); // only the key, no value
        assert_eq!(BlockSstReader::lookup_in_block(&buf, b"key"), None);
    }
}
