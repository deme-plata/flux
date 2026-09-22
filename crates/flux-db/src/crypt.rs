//! At-rest encryption for flux-db — sealed SST blocks and sealed WAL records.
//!
//! ## What this is for
//!
//! Before this module, every byte flux-db wrote was plaintext. An LSM store
//! is a *file*, and a file gets backed up, snapshotted, replicated onto a
//! RAID array, and eventually handed to somebody who repairs the disk. On a
//! SIGIL node that file is the whole ledger: wallet addresses, balances,
//! nonces, shielded note ciphertexts and the keys that index them. LZ4 is
//! not a cipher — `strings` reads a compressed block just fine once you
//! decompress it, and `lz4::block::decompress` is one line.
//!
//! So: after we compress a block, we encrypt it. Nothing else in the LSM
//! changes.
//!
//! ```text
//!   key/value pairs ──lz4──▶ compressed ──ChaCha20-Poly1305──▶ frame on disk
//!                                            ▲
//!                             nonce (random, 12 B, stored in the frame)
//!                             AAD   (position binding, NOT stored — recomputed)
//! ```
//!
//! ## The three properties, and the one that is easy to miss
//!
//! 1. **Confidentiality.** Whoever holds the file without the key learns the
//!    file's *length* and nothing else. ChaCha20 is a stream cipher; the
//!    ciphertext is indistinguishable from random.
//!
//! 2. **Integrity.** Poly1305 appends a 16-byte tag. Flip one bit of a block
//!    and `open` returns [`CryptError::Open`] instead of a wrong balance.
//!    This is strictly stronger than the CRC32 the WAL uses today: a CRC
//!    detects *accidents*, a MAC detects *adversaries* (you can forge a CRC
//!    in your head; you cannot forge Poly1305 without the key).
//!
//! 3. **Position binding — the one people forget.** AEAD authenticates the
//!    *contents* of a frame, not *where it sits*. An attacker who can write
//!    to the file cannot forge a block, but they can take block 7's perfectly
//!    valid ciphertext and drop it at block 3's offset. Every tag still
//!    verifies. The database now returns the wrong rows, authenticated.
//!
//!    The fix costs nothing: fold the block's position into the AEAD's
//!    *associated data*. AAD is authenticated but not transmitted — we
//!    recompute it at read time from where we actually found the frame. If
//!    the frame moved, the recomputed AAD differs, and the tag check fails.
//!    See [`BlockAad`].
//!
//! ## Nonce discipline (the thing that kills stream ciphers)
//!
//! Reusing a (key, nonce) pair with ChaCha20 XORs two plaintexts together and
//! is a total break. We avoid it structurally rather than by bookkeeping:
//!
//! * SSTs in an LSM are **immutable** — written once, never updated in place.
//!   A block therefore gets exactly one nonce for its entire life.
//! * The nonce is **random per frame** (96 bits from the OS CSPRNG), so there
//!   is no counter to persist, nothing to resynchronise after a crash, and no
//!   way for two writers to collide by both restarting at zero. The birthday
//!   bound at 96 bits is ~2^48 frames per key before a 50 % collision chance;
//!   at 4 KiB per frame that is on the order of 10^15 bytes under one key.
//! * The key is **derived per file** from the file's random salt
//!   ([`FileKey::derive`]), so the budget above is per SST, not global.
//!
//! ## Honest scope — what this does NOT do
//!
//! * **It does not stop whole-file rollback.** Replacing today's SST with a
//!   valid SST from last week authenticates fine, because every frame in it
//!   is genuine. Defeating that needs the *manifest* to commit to file
//!   digests, which is a level above this module. Named here so nobody reads
//!   "authenticated" as "cannot be rolled back".
//! * **It does not hide access patterns or sizes.** Frame lengths and the
//!   number of blocks are visible.
//! * **It is not key management.** [`MasterKey`] loads bytes from a file or
//!   env var and zeroizes them on drop. Where that file lives, who may read
//!   it, and how it is rotated are operator decisions.
//! * **The key lives in process memory** while the DB is open. Anyone who can
//!   read `/proc/<pid>/mem` has the key regardless of what is on disk.

use blake3::Hasher;
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    ChaCha20Poly1305, Key, Nonce,
};
use zeroize::Zeroize;

/// Length of the AEAD nonce stored at the head of every frame.
pub const NONCE_LEN: usize = 12;
/// Length of the Poly1305 tag ChaCha20-Poly1305 appends to the ciphertext.
pub const TAG_LEN: usize = 16;
/// Bytes a frame adds on top of its plaintext: the nonce plus the tag.
pub const FRAME_OVERHEAD: usize = NONCE_LEN + TAG_LEN;
/// Length of the per-file salt written in the clear at the head of a sealed SST.
pub const SALT_LEN: usize = 16;

/// BLAKE3 `derive_key` context for the per-file data key. Changing this
/// string changes every derived key, so it is versioned and never edited.
const KDF_CTX_FILE: &str = "flux-db v1 per-file data key (blake3-derive-key)";
/// Domain-separation prefix inside every AAD. Keeps flux-db frames from ever
/// verifying against another Flux subsystem's frames under a shared key.
const AAD_CTX: &[u8] = b"flux-db-frame-v1";

/// Frame kinds. Part of the AAD, so an index frame can never be substituted
/// for a data frame even at the same offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameKind {
    /// A compressed SST data block.
    Data = 0,
    /// The SST index block. Encrypted too — it holds the last key of every
    /// data block, which on a ledger means a list of real wallet addresses.
    Index = 1,
    /// One WAL record.
    Wal = 2,
}

/// Errors from the sealing layer. Deliberately coarse: a failed `open` must
/// not tell an attacker *why* it failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptError {
    /// Authentication failed: wrong key, tampered ciphertext, or a frame that
    /// was moved to a position it was not sealed for.
    Open,
    /// The frame is shorter than a nonce plus a tag, so it cannot be a frame.
    Truncated(usize),
    /// The master key material was not the required 32 bytes.
    BadKeyLen(usize),
    /// The key file or env var could not be read.
    KeySource(String),
}

impl std::fmt::Display for CryptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CryptError::Open => write!(
                f,
                "flux-db: AEAD open failed (wrong key, tampered frame, or frame moved)"
            ),
            CryptError::Truncated(n) => {
                write!(f, "flux-db: frame too short to be sealed: {n} bytes")
            }
            CryptError::BadKeyLen(n) => {
                write!(f, "flux-db: master key must be 32 bytes, got {n}")
            }
            CryptError::KeySource(e) => write!(f, "flux-db: cannot read master key: {e}"),
        }
    }
}

impl std::error::Error for CryptError {}

impl From<CryptError> for String {
    fn from(e: CryptError) -> String {
        e.to_string()
    }
}

// ─────────────────────────────── keys ───────────────────────────────

/// The 32-byte root secret for one database. Zeroized on drop.
///
/// This is never used to encrypt anything directly — every file derives its
/// own key from it via [`FileKey::derive`], so one file's nonce space is
/// independent of every other file's.
pub struct MasterKey([u8; 32]);

impl MasterKey {
    /// Wrap raw key bytes.
    pub fn from_bytes(b: [u8; 32]) -> Self {
        Self(b)
    }

    /// Read the key from a file. Accepts either 32 raw bytes or 64 hex
    /// characters (with optional trailing newline), so an operator can
    /// `head -c 32 /dev/urandom > key` or paste a hex string.
    pub fn from_file(path: &std::path::Path) -> Result<Self, CryptError> {
        let raw = std::fs::read(path)
            .map_err(|e| CryptError::KeySource(format!("{}: {e}", path.display())))?;
        Self::from_slice_or_hex(&raw)
    }

    /// Read the key from an environment variable holding 64 hex characters.
    pub fn from_env(var: &str) -> Result<Self, CryptError> {
        let s = std::env::var(var)
            .map_err(|e| CryptError::KeySource(format!("${var}: {e}")))?;
        Self::from_slice_or_hex(s.as_bytes())
    }

    /// The standard resolution order for a node: `$FLUX_DB_KEY_HEX` first
    /// (containers, systemd `EnvironmentFile`), then `$FLUX_DB_KEY_FILE`,
    /// then no key at all. `Ok(None)` means "run in plaintext mode", which is
    /// exactly what every existing database does today.
    pub fn from_environment() -> Result<Option<Self>, CryptError> {
        if std::env::var_os("FLUX_DB_KEY_HEX").is_some() {
            return Self::from_env("FLUX_DB_KEY_HEX").map(Some);
        }
        if let Some(p) = std::env::var_os("FLUX_DB_KEY_FILE") {
            return Self::from_file(std::path::Path::new(&p)).map(Some);
        }
        Ok(None)
    }

    fn from_slice_or_hex(raw: &[u8]) -> Result<Self, CryptError> {
        let trimmed: &[u8] = {
            let mut end = raw.len();
            while end > 0 && (raw[end - 1] == b'\n' || raw[end - 1] == b'\r') {
                end -= 1;
            }
            &raw[..end]
        };
        if trimmed.len() == 32 {
            let mut k = [0u8; 32];
            k.copy_from_slice(trimmed);
            return Ok(Self(k));
        }
        if trimmed.len() == 64 && trimmed.iter().all(|c| c.is_ascii_hexdigit()) {
            let mut k = [0u8; 32];
            for (i, chunk) in trimmed.chunks_exact(2).enumerate() {
                let hi = (chunk[0] as char).to_digit(16).unwrap() as u8;
                let lo = (chunk[1] as char).to_digit(16).unwrap() as u8;
                k[i] = (hi << 4) | lo;
            }
            return Ok(Self(k));
        }
        Err(CryptError::BadKeyLen(trimmed.len()))
    }
}

impl Drop for MasterKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl std::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print key material, not even truncated.
        write!(f, "MasterKey(<redacted>)")
    }
}

/// A per-file key derived from the [`MasterKey`] and that file's public salt.
///
/// Holding this is what lets you read one SST. It cannot open any other file,
/// because every other file has a different salt.
pub struct FileKey {
    cipher: ChaCha20Poly1305,
    salt: [u8; SALT_LEN],
}

impl FileKey {
    /// Derive the key for the file identified by `salt`.
    ///
    /// `BLAKE3::derive_key` is the right primitive here rather than a plain
    /// hash: it is a KDF with a hard-coded, non-attacker-controlled context
    /// string, so a key derived for flux-db frames can never collide with a
    /// key derived elsewhere from the same master secret.
    pub fn derive(master: &MasterKey, salt: [u8; SALT_LEN]) -> Self {
        let mut h = Hasher::new_derive_key(KDF_CTX_FILE);
        h.update(&master.0);
        h.update(&salt);
        let mut sub = [0u8; 32];
        h.finalize_xof().fill(&mut sub);
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&sub));
        sub.zeroize();
        Self { cipher, salt }
    }

    /// Derive a key for a brand-new file, minting a fresh random salt.
    /// Returns the salt so the caller can write it in the clear.
    pub fn fresh(master: &MasterKey) -> Self {
        let mut salt = [0u8; SALT_LEN];
        fill_random(&mut salt);
        Self::derive(master, salt)
    }

    /// The public salt for this file. Written in the clear at the head of the
    /// sealed SST — a salt is a domain separator, not a secret.
    pub fn salt(&self) -> [u8; SALT_LEN] {
        self.salt
    }

    /// Seal one plaintext frame at a known position.
    ///
    /// Layout of the returned frame: `[12-byte nonce][ciphertext][16-byte tag]`.
    /// The AAD is *not* stored — it is recomputed on read from where the frame
    /// was actually found, which is what makes a moved frame fail to open.
    pub fn seal(&self, plain: &[u8], aad: &BlockAad) -> Vec<u8> {
        let mut nonce = [0u8; NONCE_LEN];
        fill_random(&mut nonce);
        let aad_bytes = aad.encode(&self.salt);
        let ct = self
            .cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload { msg: plain, aad: &aad_bytes },
            )
            // ChaCha20-Poly1305 encryption is infallible for any in-memory
            // plaintext we can construct (the only documented failure is a
            // payload beyond 256 GiB, which a 4 KiB block cannot reach).
            .expect("chacha20poly1305 encrypt cannot fail for a block-sized payload");
        let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ct);
        out
    }

    /// Open a frame that is expected to sit at `aad`'s position.
    ///
    /// Fails with [`CryptError::Open`] for a wrong key, a tampered byte, *or*
    /// a frame that was relocated — the three are deliberately indistinguishable
    /// to the caller.
    pub fn open(&self, frame: &[u8], aad: &BlockAad) -> Result<Vec<u8>, CryptError> {
        if frame.len() < FRAME_OVERHEAD {
            return Err(CryptError::Truncated(frame.len()));
        }
        let (nonce, ct) = frame.split_at(NONCE_LEN);
        let aad_bytes = aad.encode(&self.salt);
        self.cipher
            .decrypt(Nonce::from_slice(nonce), Payload { msg: ct, aad: &aad_bytes })
            .map_err(|_| CryptError::Open)
    }
}

impl std::fmt::Debug for FileKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FileKey(salt={}..)", hex4(&self.salt))
    }
}

// ─────────────────────────── position binding ───────────────────────────

/// Everything about a frame's *position* that must be authenticated even
/// though it is not encrypted.
///
/// This is the anti-substitution device. Two frames sealed with the same key
/// but different `BlockAad` cannot be swapped: the tag check recomputes the
/// AAD from the position the frame was read at, and a mismatch is a failure,
/// not a wrong answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockAad {
    /// Which kind of frame this is — a data block can never pose as the index.
    pub kind: FrameKind,
    /// Ordinal within the file: block number for data, `0` for the index,
    /// record sequence for the WAL. Pins the frame to its slot.
    pub ordinal: u64,
    /// Plaintext length. Cheap extra binding, and it lets the reader reject a
    /// length mismatch before it ever looks at the decompressed bytes.
    pub plain_len: u32,
}

impl BlockAad {
    /// AAD for data block `index`, whose compressed plaintext is `plain_len`.
    pub fn data(index: u64, plain_len: u32) -> Self {
        Self { kind: FrameKind::Data, ordinal: index, plain_len }
    }
    /// AAD for the SST index block.
    pub fn index(plain_len: u32) -> Self {
        Self { kind: FrameKind::Index, ordinal: 0, plain_len }
    }
    /// AAD for WAL record number `seq`.
    pub fn wal(seq: u64, plain_len: u32) -> Self {
        Self { kind: FrameKind::Wal, ordinal: seq, plain_len }
    }

    /// Canonical byte encoding. Fixed-width fields only — no length-prefixed
    /// or variable-width parts, so two different positions can never encode
    /// to the same bytes.
    fn encode(&self, salt: &[u8; SALT_LEN]) -> Vec<u8> {
        let mut v = Vec::with_capacity(AAD_CTX.len() + SALT_LEN + 1 + 8 + 4);
        v.extend_from_slice(AAD_CTX);
        v.extend_from_slice(salt);
        v.push(self.kind as u8);
        v.extend_from_slice(&self.ordinal.to_le_bytes());
        v.extend_from_slice(&self.plain_len.to_le_bytes());
        v
    }
}

// ───────────────────────────── randomness ─────────────────────────────

/// Fill `buf` with cryptographically secure random bytes.
///
/// flux-db deliberately carries no `rand` dependency — it is a storage engine
/// and every extra crate in its tree is a crate in every SIGIL binary. The OS
/// CSPRNG is right here and needs nothing: on Linux `/dev/urandom` after boot
/// is the same ChaCha20 DRBG that `getrandom(2)` returns.
///
/// The handle is opened **once per thread** and kept. The first version of this
/// function opened `/dev/urandom` per call, and the seal/open benchmark caught
/// it immediately: sealing measured 27 us/block against 12 us/block for opening,
/// which is nonsense for a symmetric cipher — the two directions do the same
/// arithmetic. The 15 us gap was one `open(2)` + `close(2)` per 12-byte nonce,
/// i.e. the randomness plumbing cost more than the encryption. Caching the fd
/// removes it. Worth remembering as a shape: when one direction of a symmetric
/// operation is much slower than the other, the cost is not in the mathematics.
fn fill_random(buf: &mut [u8]) {
    use std::cell::RefCell;
    use std::io::Read;
    thread_local! {
        static URANDOM: RefCell<std::fs::File> = RefCell::new(
            std::fs::File::open("/dev/urandom")
                .expect("flux-db: /dev/urandom unavailable — cannot generate a nonce safely"),
        );
    }
    URANDOM.with(|f| {
        f.borrow_mut()
            .read_exact(buf)
            .expect("flux-db: short read from /dev/urandom")
    });
}

fn hex4(b: &[u8]) -> String {
    b.iter().take(4).map(|x| format!("{x:02x}")).collect()
}

// ─────────────────────────────── tests ───────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn master() -> MasterKey {
        MasterKey::from_bytes([7u8; 32])
    }

    #[test]
    fn roundtrip_recovers_the_plaintext() {
        let fk = FileKey::fresh(&master());
        let plain = b"balance=1250 QUG; wallet=qnk4973498a";
        let aad = BlockAad::data(3, plain.len() as u32);
        let frame = fk.seal(plain, &aad);
        assert_eq!(fk.open(&frame, &aad).unwrap(), plain);
    }

    #[test]
    fn ciphertext_does_not_leak_the_plaintext() {
        let fk = FileKey::fresh(&master());
        let plain = b"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let frame = fk.seal(plain, &BlockAad::data(0, plain.len() as u32));
        assert!(
            !frame.windows(8).any(|w| w == &plain[..8]),
            "plaintext survived into the frame"
        );
        assert_eq!(frame.len(), plain.len() + FRAME_OVERHEAD);
    }

    #[test]
    fn every_frame_gets_a_distinct_nonce() {
        let fk = FileKey::fresh(&master());
        let aad = BlockAad::data(0, 4);
        let a = fk.seal(b"same", &aad);
        let b = fk.seal(b"same", &aad);
        assert_ne!(&a[..NONCE_LEN], &b[..NONCE_LEN], "nonce repeated");
        assert_ne!(a, b, "identical plaintext produced identical ciphertext");
    }

    #[test]
    fn a_flipped_bit_fails_to_open() {
        let fk = FileKey::fresh(&master());
        let aad = BlockAad::data(1, 5);
        let mut frame = fk.seal(b"hello", &aad);
        let last = frame.len() - 1;
        frame[last] ^= 0x01;
        assert_eq!(fk.open(&frame, &aad), Err(CryptError::Open));
    }

    #[test]
    fn a_frame_moved_to_another_slot_fails_to_open() {
        // The attack this module exists for: the frame is genuine and its tag
        // is genuine, but it was relocated. Position is in the AAD, so it dies.
        let fk = FileKey::fresh(&master());
        let plain = b"block seven";
        let frame = fk.seal(plain, &BlockAad::data(7, plain.len() as u32));
        assert!(fk.open(&frame, &BlockAad::data(7, plain.len() as u32)).is_ok());
        assert_eq!(
            fk.open(&frame, &BlockAad::data(3, plain.len() as u32)),
            Err(CryptError::Open),
            "a data block was accepted at the wrong offset"
        );
    }

    #[test]
    fn a_data_frame_cannot_pose_as_the_index() {
        let fk = FileKey::fresh(&master());
        let plain = b"index-shaped bytes";
        let frame = fk.seal(plain, &BlockAad::data(0, plain.len() as u32));
        assert_eq!(
            fk.open(&frame, &BlockAad::index(plain.len() as u32)),
            Err(CryptError::Open)
        );
    }

    #[test]
    fn a_frame_from_another_file_fails_to_open() {
        let m = master();
        let a = FileKey::fresh(&m);
        let b = FileKey::fresh(&m);
        let aad = BlockAad::data(0, 4);
        let frame = a.seal(b"mine", &aad);
        assert_eq!(b.open(&frame, &aad), Err(CryptError::Open));
    }

    #[test]
    fn the_wrong_master_key_fails_to_open() {
        let salt = [9u8; SALT_LEN];
        let good = FileKey::derive(&MasterKey::from_bytes([1u8; 32]), salt);
        let bad = FileKey::derive(&MasterKey::from_bytes([2u8; 32]), salt);
        let aad = BlockAad::data(0, 6);
        let frame = good.seal(b"secret", &aad);
        assert_eq!(bad.open(&frame, &aad), Err(CryptError::Open));
    }

    #[test]
    fn the_same_master_and_salt_derive_the_same_key() {
        let salt = [42u8; SALT_LEN];
        let a = FileKey::derive(&MasterKey::from_bytes([3u8; 32]), salt);
        let b = FileKey::derive(&MasterKey::from_bytes([3u8; 32]), salt);
        let aad = BlockAad::data(2, 8);
        let frame = a.seal(b"reopened", &aad);
        assert_eq!(b.open(&frame, &aad).unwrap(), b"reopened");
    }

    #[test]
    fn a_truncated_frame_is_reported_as_truncated_not_as_forgery() {
        let fk = FileKey::fresh(&master());
        assert_eq!(
            fk.open(&[0u8; 4], &BlockAad::data(0, 0)),
            Err(CryptError::Truncated(4))
        );
    }

    #[test]
    fn wal_records_are_pinned_to_their_sequence_number() {
        let fk = FileKey::fresh(&master());
        let plain = b"put wallet=alice 500";
        let frame = fk.seal(plain, &BlockAad::wal(11, plain.len() as u32));
        assert!(fk.open(&frame, &BlockAad::wal(11, plain.len() as u32)).is_ok());
        // Replaying record 11 as record 12 is a rollback attempt; it fails.
        assert_eq!(
            fk.open(&frame, &BlockAad::wal(12, plain.len() as u32)),
            Err(CryptError::Open)
        );
    }

    #[test]
    fn key_parses_from_raw_bytes_and_from_hex_with_a_newline() {
        let raw = MasterKey::from_slice_or_hex(&[0xABu8; 32]).unwrap();
        let hex = MasterKey::from_slice_or_hex(b"abababababababababababababababababababababababababababababababab\n")
            .unwrap();
        assert_eq!(raw.0, hex.0);
    }

    #[test]
    fn a_key_of_the_wrong_length_is_rejected() {
        // `MasterKey` deliberately does not derive `PartialEq` — comparing key
        // material with `==` is a timing-leak footgun — so match on the error.
        match MasterKey::from_slice_or_hex(b"too short") {
            Err(CryptError::BadKeyLen(9)) => {}
            other => panic!("expected BadKeyLen(9), got {:?}", other.map(|_| "<key>")),
        }
    }

    #[test]
    fn an_empty_plaintext_still_round_trips() {
        // Degenerate but reachable: an SST with an empty final block.
        let fk = FileKey::fresh(&master());
        let aad = BlockAad::data(0, 0);
        let frame = fk.seal(b"", &aad);
        assert_eq!(frame.len(), FRAME_OVERHEAD);
        assert_eq!(fk.open(&frame, &aad).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn a_realistic_4kib_block_round_trips() {
        let fk = FileKey::fresh(&master());
        let plain: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
        let aad = BlockAad::data(17, plain.len() as u32);
        let frame = fk.seal(&plain, &aad);
        assert_eq!(fk.open(&frame, &aad).unwrap(), plain);
    }

    /// What sealing actually costs, measured rather than asserted.
    ///
    /// Run with:
    ///   fluxc test -p flux-db --features encryption --lib -- --ignored --nocapture seal_throughput
    ///
    /// The number that matters is not MB/s in the abstract — it is the cost per
    /// 4 KiB SST block set against flux-db's own measured ~130 microseconds per
    /// KV entry in `batch_put`. If sealing is a low single-digit percentage of
    /// that, encryption is free in practice and the honest recommendation is
    /// "turn it on".
    #[test]
    #[ignore = "benchmark — run explicitly with --ignored"]
    fn seal_throughput_bench() {
        use std::time::Instant;
        let fk = FileKey::fresh(&master());
        let block: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
        let n = 20_000;

        let t0 = Instant::now();
        let mut frames = Vec::with_capacity(n);
        for i in 0..n {
            frames.push(fk.seal(&block, &BlockAad::data(i as u64, block.len() as u32)));
        }
        let seal_el = t0.elapsed();

        let t1 = Instant::now();
        for (i, f) in frames.iter().enumerate() {
            let out = fk
                .open(f, &BlockAad::data(i as u64, block.len() as u32))
                .expect("open must succeed");
            std::hint::black_box(out);
        }
        let open_el = t1.elapsed();

        let bytes = (n * block.len()) as f64;
        let seal_us = seal_el.as_secs_f64() * 1e6 / n as f64;
        let open_us = open_el.as_secs_f64() * 1e6 / n as f64;
        println!("\n  flux-db seal/open — {n} x 4 KiB blocks");
        println!("  seal: {:>7.2} us/block  {:>7.1} MB/s", seal_us, bytes / seal_el.as_secs_f64() / 1e6);
        println!("  open: {:>7.2} us/block  {:>7.1} MB/s", open_us, bytes / open_el.as_secs_f64() / 1e6);
        println!("  overhead per block: {} bytes ({:.2}%)", FRAME_OVERHEAD,
                 FRAME_OVERHEAD as f64 / block.len() as f64 * 100.0);
        println!("  vs flux-db's measured ~130 us per KV batch_put entry: seal is {:.1}% of one entry\n",
                 seal_us / 130.0 * 100.0);
    }

    #[test]
    fn the_master_key_never_prints_itself() {
        assert_eq!(format!("{:?}", master()), "MasterKey(<redacted>)");
    }
}
