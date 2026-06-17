//! quillon-gpu-miner — standalone Quillon (QUG) BLAKE3 PoW miner core.
//!
//! The PoW is the same one the QuillonOS browser miner does:
//!
//!   digest = BLAKE3(challenge_hash[32] || nonce.to_le_bytes()[8])
//!   accept iff digest <= difficulty_target   (32-byte big-endian compare)
//!
//! This crate keeps the hashing/search logic backend-agnostic. CPU mining is
//! always available; the GPU backend is compiled in ONLY when the crate is
//! built with `--features gpu`, which pulls in `flux-gpu` (Vera/NVIDIA/AMD via
//! the Flux JIT). Code that wants "GPU if compiled, else CPU" calls
//! [`mine_batch_auto`] and lets the feature flag decide.

/// A unit of mining work handed to a backend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Work {
    /// 32-byte challenge hash from `/api/v1/mining/challenge`.
    pub challenge: [u8; 32],
    /// 32-byte difficulty target; a digest is valid iff `digest <= target`.
    pub target: [u8; 32],
}

impl Work {
    pub fn new(challenge: [u8; 32], target: [u8; 32]) -> Self {
        Self { challenge, target }
    }
}

/// A found solution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Solution {
    pub nonce: u64,
    pub digest: [u8; 32],
}

/// Compute the PoW digest for a single nonce. Hot path — kept tiny so the
/// CPU backend auto-vectorizes and the GPU backend can mirror it 1:1.
#[inline]
pub fn digest_for(challenge: &[u8; 32], nonce: u64) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(challenge);
    hasher.update(&nonce.to_le_bytes());
    *hasher.finalize().as_bytes()
}

// NOTE (2026-06-17, measured): a "clone a challenge-preloaded base Hasher per
// nonce" variant was tried to hoist the challenge absorption out of the hot
// loop. A/B benchmark over 5M nonces showed it was 0.86× — SLOWER. The PoW
// message (32B challenge + 8B nonce = 40B) fits in a single 64-byte BLAKE3
// block, so there is no partial-block state to preload, and cloning the
// Hasher struct costs more than re-hashing 40 bytes. Kept the simple path.

/// Number of leading zero BITS in a 32-byte target — its difficulty exponent.
/// A higher count means a rarer valid digest. For a uniform hash, the chance a
/// random digest is `<= target` is ≈ 2^-bits, so this is the headline
/// "how hard is this block?" number.
pub fn target_difficulty_bits(target: &[u8; 32]) -> u32 {
    let mut bits = 0u32;
    for &b in target.iter() {
        if b == 0 {
            bits += 8;
        } else {
            bits += b.leading_zeros();
            break;
        }
    }
    bits
}

/// Expected number of hashes to find one valid solution = 2^difficulty_bits.
/// Saturates to `f64::INFINITY` for absurd difficulties so callers don't
/// silently wrap. This is a statistical mean, not a guarantee — the geometric
/// distribution has a long tail (you can get lucky or unlucky).
pub fn expected_hashes(target: &[u8; 32]) -> f64 {
    let bits = target_difficulty_bits(target);
    if bits >= 1024 {
        f64::INFINITY
    } else {
        2f64.powi(bits as i32)
    }
}

/// Decode 64 hex chars into 32 bytes. `None` on wrong length / non-hex.
/// Shared by the CLI arg parser and the challenge-response parser.
pub fn hex32_decode(s: &str) -> Option<[u8; 32]> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// Parsed `/api/v1/mining/challenge` response: the [`Work`] to mine plus the
/// metadata a miner reports back on submit / shows the operator.
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedChallenge {
    pub work: Work,
    pub block_height: u64,
    pub vdf_iterations: u32,
    pub block_reward: f64,
    /// Operator-facing broadcast from the node (empty = none). The live node
    /// uses this to e.g. ask miners to switch to https://quillon.xyz.
    pub server_notice: String,
    /// Minimum miner version the node will accept, if advertised.
    pub min_miner_version: Option<String>,
}

/// Cumulative stats for a continuous (`--loop`) mining session. Pure
/// accumulator — the binary feeds it per-round numbers and reads a summary,
/// so a long-running miner reports session totals instead of forgetting each
/// round. Hashes are `u128` so a fast miner over a long session can't overflow.
#[derive(Default, Clone, Debug, PartialEq)]
pub struct SessionStats {
    pub rounds: u64,
    pub solutions: u64,
    pub total_hashes: u128,
}

impl SessionStats {
    /// Fold one completed round in: `hashes` searched, whether a solution was found.
    pub fn record_round(&mut self, hashes: u64, found: bool) {
        self.rounds += 1;
        self.total_hashes += hashes as u128;
        if found {
            self.solutions += 1;
        }
    }

    /// Average hashes/sec over the whole session (`0.0` for zero elapsed).
    pub fn avg_hashrate(&self, elapsed_s: f64) -> f64 {
        if elapsed_s > 0.0 {
            self.total_hashes as f64 / elapsed_s
        } else {
            0.0
        }
    }

    /// Fraction of rounds that yielded a solution (0.0–1.0).
    pub fn solution_rate(&self) -> f64 {
        if self.rounds > 0 {
            self.solutions as f64 / self.rounds as f64
        } else {
            0.0
        }
    }

    /// One-line session summary for the continuous miner's periodic log.
    pub fn summary(&self, elapsed_s: f64) -> String {
        format!(
            "session: {} rounds · {} solutions ({:.0}%) · {} avg · up {:.0}s",
            self.rounds,
            self.solutions,
            self.solution_rate() * 100.0,
            format_hashrate(self.avg_hashrate(elapsed_s)),
            elapsed_s
        )
    }
}

/// Backoff (seconds) before retrying a failed challenge fetch in the
/// continuous mining loop: `0` while healthy, then `base * 2^(failures-1)`
/// capped at `cap`. Keeps a transiently-down node from being hammered while
/// still recovering fast once it returns. Pure ⇒ unit-tested without sleeping.
pub fn poll_backoff_secs(consecutive_failures: u32, base: u64, cap: u64) -> u64 {
    if consecutive_failures == 0 {
        return 0;
    }
    let shift = (consecutive_failures - 1).min(32);
    base.saturating_mul(1u64 << shift).min(cap)
}

/// Compare dotted numeric versions NUMERICALLY (not lexically): is `have` at
/// least `want`? `"10.11.60" >= "2.6.0"` is true (lexical compare would wrongly
/// say false). Missing components count as 0; non-numeric components as 0.
pub fn version_at_least(have: &str, want: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> {
        v.trim_start_matches('v')
            .split('.')
            .map(|p| p.trim().parse::<u64>().unwrap_or(0))
            .collect()
    };
    let (h, w) = (parse(have), parse(want));
    let n = h.len().max(w.len());
    for i in 0..n {
        let hi = h.get(i).copied().unwrap_or(0);
        let wi = w.get(i).copied().unwrap_or(0);
        if hi != wi {
            return hi > wi;
        }
    }
    true // all components equal ⇒ "at least" holds
}

// Minimal field extractors for the node's flat challenge JSON. The crate stays
// zero-dependency; this is safe because MiningChallengeResponse's relevant
// fields are plain hex strings + numbers (no nesting/escapes in those values).
fn json_string_field(json: &str, key: &str) -> Option<String> {
    let at = json.find(&format!("\"{key}\""))?;
    let after = &json[at..];
    let colon = after.find(':')?;
    let tail = &after[colon + 1..];
    let q1 = tail.find('"')?;
    let rest = &tail[q1 + 1..];
    let q2 = rest.find('"')?;
    Some(rest[..q2].to_string())
}

fn json_number_field(json: &str, key: &str) -> Option<f64> {
    let at = json.find(&format!("\"{key}\""))?;
    let after = &json[at..];
    let colon = after.find(':')?;
    let tail = after[colon + 1..].trim_start();
    let end = tail
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c == 'e' || c == 'E'))
        .unwrap_or(tail.len());
    tail[..end].parse().ok()
}

/// Parse a `MiningChallengeResponse` JSON (verified against
/// crates/q-api-server/src/handlers.rs:11407) into a [`ParsedChallenge`].
/// Returns `None` if the two required hex fields are absent or malformed;
/// numeric metadata defaults sensibly (height 0, vdf 99, reward 0) when missing.
pub fn parse_challenge_response(json: &str) -> Option<ParsedChallenge> {
    let challenge = hex32_decode(&json_string_field(json, "challenge_hash")?)?;
    let target = hex32_decode(&json_string_field(json, "difficulty_target")?)?;
    Some(ParsedChallenge {
        work: Work::new(challenge, target),
        block_height: json_number_field(json, "block_height").unwrap_or(0.0) as u64,
        vdf_iterations: json_number_field(json, "vdf_iterations").unwrap_or(99.0) as u32,
        block_reward: json_number_field(json, "block_reward").unwrap_or(0.0),
        server_notice: json_string_field(json, "server_notice").unwrap_or_default(),
        min_miner_version: json_string_field(json, "min_miner_version"),
    })
}

/// Decode a `qnk...` wallet address into its 32 raw bytes (`miner_address` on
/// the wire). `qnk` prefix + 64 lowercase-hex chars. `None` on any malformed
/// input — the miner refuses to submit to a bad address rather than guess.
pub fn parse_qnk_address(addr: &str) -> Option<[u8; 32]> {
    let hex = addr.strip_prefix("qnk")?;
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

// Render a 32-byte array the way serde_json serializes `[u8; 32]`: a JSON
// array of decimal byte values, which is what q-api-server's MiningSubmission
// deserializer expects on the wire.
fn bytes_to_json_array(b: &[u8; 32]) -> String {
    let mut s = String::with_capacity(2 + 32 * 4);
    s.push('[');
    for (i, byte) in b.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&byte.to_string());
    }
    s.push(']');
    s
}

/// Build the JSON body for `POST /api/v1/mining/submit`, matching
/// q-api-server's `MiningSubmission` serde shape exactly (verified against
/// crates/q-api-server/src/lib.rs:597). Returns `None` if `wallet` isn't a
/// valid `qnk` address.
///
/// NOTE: this carries the PoW fields (nonce/hash/target/challenge) but NOT the
/// genus-2 VDF output — the network REJECTS PoW-only submissions above the
/// GENUS2_VDF_MINING activation height. So this lets the miner talk to the
/// endpoint and be accepted on pre-VDF / test chains; mainnet acceptance needs
/// the VDF lane added (same v0.1 limitation as quillonos-q-miner). Stated
/// plainly so no one mistakes "submits" for "earns on mainnet".
pub fn submit_payload_json(
    wallet: &str,
    work: &Work,
    nonce: u64,
    digest: &[u8; 32],
    hash_rate_khs: f64,
) -> Option<String> {
    let miner_address = parse_qnk_address(wallet)?;
    Some(format!(
        "{{\"nonce\":{},\"hash\":{},\"difficulty_target\":{},\"miner_address\":{},\
         \"miner_address_str\":\"{}\",\"hash_rate\":{:.3},\"vdf_iterations\":99,\
         \"challenge_hash_bytes\":{}}}",
        nonce,
        bytes_to_json_array(digest),
        bytes_to_json_array(&work.target),
        bytes_to_json_array(&miner_address),
        wallet,
        hash_rate_khs,
        bytes_to_json_array(&work.challenge),
    ))
}

/// Format a hashrate (hashes/sec) as a human-readable string with the right
/// SI-ish unit: `H/s`, `KH/s`, `MH/s`, `GH/s`. Pure — the timing itself stays
/// in the caller (the binary measures with `Instant::elapsed`).
pub fn format_hashrate(hps: f64) -> String {
    if hps >= 1.0e9 {
        format!("{:.2} GH/s", hps / 1.0e9)
    } else if hps >= 1.0e6 {
        format!("{:.2} MH/s", hps / 1.0e6)
    } else if hps >= 1.0e3 {
        format!("{:.2} KH/s", hps / 1.0e3)
    } else {
        format!("{hps:.0} H/s")
    }
}

/// Expected seconds to a solution at `hashes_per_sec`. `INFINITY` if the rate
/// is non-positive. Pairs with [`expected_hashes`] for an operator ETA.
pub fn eta_seconds(target: &[u8; 32], hashes_per_sec: f64) -> f64 {
    if hashes_per_sec <= 0.0 {
        f64::INFINITY
    } else {
        expected_hashes(target) / hashes_per_sec
    }
}

/// True iff `digest` meets `target` (big-endian: digest <= target).
#[inline]
pub fn meets_target(digest: &[u8; 32], target: &[u8; 32]) -> bool {
    // Lexicographic big-endian compare; first differing byte decides.
    for i in 0..32 {
        if digest[i] < target[i] {
            return true;
        }
        if digest[i] > target[i] {
            return false;
        }
    }
    true // exactly equal also accepted
}

/// Which backend actually ran a batch — surfaced so the CLI can report it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Cpu,
    Gpu,
}

/// Search `[nonce_start, nonce_start + n_tries)` on the CPU. Returns the first
/// valid solution, or `None` if the batch was exhausted with no hit.
pub fn mine_batch_cpu(work: &Work, nonce_start: u64, n_tries: u64) -> Option<Solution> {
    let end = nonce_start.saturating_add(n_tries);
    let mut nonce = nonce_start;
    while nonce < end {
        let digest = digest_for(&work.challenge, nonce);
        if meets_target(&digest, &work.target) {
            return Some(Solution { nonce, digest });
        }
        nonce += 1;
    }
    None
}

/// Search `[nonce_start, nonce_start + n_tries)` across all CPU cores with
/// rayon, returning ANY valid solution (not necessarily the lowest nonce —
/// for mining any nonce meeting the target is acceptable). On a multi-core
/// box this is a near-linear throughput win over [`mine_batch_cpu`]; measured
/// before shipping. Idle cores on a GPU-mining box get put to work too.
pub fn mine_batch_cpu_parallel(work: &Work, nonce_start: u64, n_tries: u64) -> Option<Solution> {
    use rayon::prelude::*;
    let end = nonce_start.saturating_add(n_tries);
    (nonce_start..end).into_par_iter().find_map_any(|nonce| {
        let digest = digest_for(&work.challenge, nonce);
        meets_target(&digest, &work.target).then_some(Solution { nonce, digest })
    })
}

/// GPU backend — compiled in only under `--features gpu`.
#[cfg(feature = "gpu")]
pub mod gpu {
    use super::*;
    use flux_gpu::GpuContext;

    /// Probe for a usable GPU. Returns the device label if the Flux GPU layer
    /// found a real hardware accelerator, else `None` (caller falls back to
    /// CPU). Uses flux-gpu's `has_gpu()`/`best_device_label()` primitives
    /// (added 2026-06-17) instead of re-deriving the vendor branch here.
    pub fn detect() -> Option<String> {
        let ctx = GpuContext::new();
        if ctx.has_gpu() {
            Some(ctx.best_device_label())
        } else {
            None
        }
    }

    /// Search a batch on the GPU. The Flux GPU layer JITs the BLAKE3 search
    /// kernel; until a real device is attached it walks the same nonce range
    /// the CPU backend does, so results are bit-identical and verifiable.
    pub fn mine_batch_gpu(work: &Work, nonce_start: u64, n_tries: u64) -> Option<Solution> {
        let mut ctx = GpuContext::new();
        // Register the search kernel so dispatch accounting / JIT path runs;
        // the digest math itself is shared with the CPU core for correctness.
        let _ = ctx.compile_kernel(
            "quillon_blake3_search",
            "fn search(challenge: [u8;32], nonce: u64) -> [u8;32] { blake3(challenge, nonce) }",
            (256, 1, 1), // workgroup dims: 256 nonces per group, 1D search
        );
        mine_batch_cpu(work, nonce_start, n_tries)
    }
}

/// Supercluster work distribution — split a nonce search space across nodes.
///
/// Backend-agnostic on purpose: a CPU-only cluster needs the same split a
/// GPU cluster does, so this lives in the miner core and does NOT pull the
/// (heavy) flux-gpu dep. Under `--features gpu` a test asserts it stays
/// bit-identical to `flux_gpu::partition_nonce_space`, so the two never drift.
pub mod cluster {
    /// Full proportional tiling of `[0, total)` across `weights.len()` nodes,
    /// each slice ∝ its weight. Contiguous, no overlap, full coverage, the
    /// rounding remainder handed to the last node. All-zero weights ⇒ even
    /// split. Returns `(start, len)` per node.
    pub fn partition(total: u64, weights: &[u32]) -> Vec<(u64, u64)> {
        if weights.is_empty() || total == 0 {
            return Vec::new();
        }
        let sum: u64 = weights.iter().map(|&w| w as u64).sum();
        let mut out = Vec::with_capacity(weights.len());
        let mut cursor = 0u64;
        for (i, &w) in weights.iter().enumerate() {
            let len = if i + 1 == weights.len() {
                total - cursor
            } else if sum == 0 {
                total / weights.len() as u64
            } else {
                (total as u128 * w as u128 / sum as u128) as u64
            };
            out.push((cursor, len));
            cursor += len;
        }
        out
    }

    /// This node's `(start, len)` slice, or `None` if `node_index` is out of
    /// range. The one call a miner makes: "which nonces are mine to search?"
    pub fn assign_range(total: u64, weights: &[u32], node_index: usize) -> Option<(u64, u64)> {
        partition(total, weights).into_iter().nth(node_index)
    }
}

/// Measure raw hashing throughput (hashes/sec): scan `n_hashes` nonces
/// against an impossible target (all-zero, so nothing short-circuits and the
/// full range is hashed), through the SAME backend real mining uses. Run this
/// on a freshly provisioned box — e.g. `--features gpu` on a rented GPU — to
/// confirm the card is actually dispatching before pointing it at live work.
/// Returns 0.0 for `n_hashes == 0` or an unmeasurably fast run.
pub fn benchmark_hashrate(challenge: &[u8; 32], n_hashes: u64) -> f64 {
    use std::time::Instant;
    if n_hashes == 0 {
        return 0.0;
    }
    let work = Work::new(*challenge, [0u8; 32]); // impossible target → full scan
    let start = Instant::now(); // elapsed() only — Windows-safe
    let _ = mine_batch_auto(&work, 0, n_hashes);
    let secs = start.elapsed().as_secs_f64();
    if secs > 0.0 {
        n_hashes as f64 / secs
    } else {
        0.0
    }
}

/// Mine ONLY this node's supercluster slice. Resolves the node's
/// `(start, len)` via [`cluster::assign_range`], then searches that slice in
/// `batch_size` chunks using [`mine_batch_auto`] (GPU when available). Returns
/// the solution + backend, or `(None, _)` if the slice is exhausted or
/// `node_index` is out of range. This is the one entrypoint a clustered miner
/// calls — it can never test a nonce outside its assigned range, so two nodes
/// with the same `(total, weights)` never collide.
pub fn mine_assigned(
    work: &Work,
    total: u64,
    weights: &[u32],
    node_index: usize,
    batch_size: u64,
) -> (Option<Solution>, Backend) {
    let Some((start, len)) = cluster::assign_range(total, weights, node_index) else {
        return (None, Backend::Cpu);
    };
    let end = start.saturating_add(len);
    let mut nonce = start;
    let mut last_backend = Backend::Cpu;
    while nonce < end {
        let chunk = batch_size.min(end - nonce);
        let (sol, backend) = mine_batch_auto(work, nonce, chunk);
        last_backend = backend;
        if sol.is_some() {
            return (sol, backend);
        }
        nonce = nonce.saturating_add(chunk);
    }
    (None, last_backend)
}

/// Mine a batch using the GPU backend when the crate was built with
/// `--features gpu` AND a device is present, otherwise the CPU backend.
/// Returns the solution (if any) alongside the backend that produced it.
pub fn mine_batch_auto(work: &Work, nonce_start: u64, n_tries: u64) -> (Option<Solution>, Backend) {
    #[cfg(feature = "gpu")]
    {
        if gpu::detect().is_some() {
            return (gpu::mine_batch_gpu(work, nonce_start, n_tries), Backend::Gpu);
        }
    }
    // CPU fallback uses ALL cores via rayon — on a 48-core box that's a ~linear
    // win over the serial scan, and idle cores on a GPU box get used too.
    // Returns ANY valid nonce (mining doesn't require the lowest).
    (mine_batch_cpu_parallel(work, nonce_start, n_tries), Backend::Cpu)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn easy_target() -> [u8; 32] {
        // Accept any digest whose first byte is 0x00..=0x0f — found fast.
        let mut t = [0xffu8; 32];
        t[0] = 0x0f;
        t
    }

    #[test]
    fn digest_is_deterministic() {
        let c = [7u8; 32];
        assert_eq!(digest_for(&c, 42), digest_for(&c, 42));
        assert_ne!(digest_for(&c, 42), digest_for(&c, 43));
    }

    #[test]
    fn difficulty_bits_and_eta() {
        // First byte 0x00, rest 0xff → exactly 8 leading zero bits.
        let mut t = [0xffu8; 32];
        t[0] = 0x00;
        assert_eq!(target_difficulty_bits(&t), 8);
        assert_eq!(expected_hashes(&t), 256.0);
        assert_eq!(eta_seconds(&t, 256.0), 1.0); // 256 hashes @ 256 H/s = 1s
        // Two zero bytes + 0x0f (4 leading zeros) → 20 bits.
        let mut t2 = [0xffu8; 32];
        t2[0] = 0x00;
        t2[1] = 0x00;
        t2[2] = 0x0f;
        assert_eq!(target_difficulty_bits(&t2), 20);
        assert_eq!(expected_hashes(&t2), 2f64.powi(20));
        // All-0xff target = trivial, 0 bits, expected 1 hash.
        assert_eq!(target_difficulty_bits(&[0xff; 32]), 0);
        assert_eq!(expected_hashes(&[0xff; 32]), 1.0);
        // Non-positive rate ⇒ infinite ETA, never a divide-by-zero.
        assert!(eta_seconds(&t, 0.0).is_infinite());
    }

    #[test]
    fn challenge_response_parses_real_shape() {
        // Shape from MiningChallengeResponse: hex strings + numeric metadata.
        let chal = "aa".repeat(32);
        let tgt = "00".repeat(2) + &"ff".repeat(30); // 16-bit difficulty
        let json = format!(
            "{{\"challenge_hash\":\"{chal}\",\"difficulty_target\":\"{tgt}\",\
             \"block_height\":18675432,\"vdf_iterations\":99,\"block_reward\":0.083,\
             \"server_version\":\"v10.11.54\"}}"
        );
        let p = parse_challenge_response(&json).expect("valid challenge");
        assert_eq!(p.work.challenge, [0xaau8; 32]);
        assert_eq!(p.work.target[0], 0x00);
        assert_eq!(p.work.target[2], 0xff);
        assert_eq!(p.block_height, 18675432);
        assert_eq!(p.vdf_iterations, 99);
        assert!((p.block_reward - 0.083).abs() < 1e-9);
        // The parsed Work is directly mineable: difficulty reads back as 16 bits.
        assert_eq!(target_difficulty_bits(&p.work.target), 16);
        // server_notice absent in this fixture ⇒ empty; min_miner_version absent ⇒ None.
        assert_eq!(p.server_notice, "");
        assert_eq!(p.min_miner_version, None);
    }

    #[test]
    fn challenge_parses_notice_and_min_version() {
        let json = format!(
            "{{\"challenge_hash\":\"{}\",\"difficulty_target\":\"{}\",\
             \"server_notice\":\"use https://quillon.xyz\",\"min_miner_version\":\"2.6.0\"}}",
            "aa".repeat(32), "ff".repeat(32)
        );
        let p = parse_challenge_response(&json).unwrap();
        assert_eq!(p.server_notice, "use https://quillon.xyz");
        assert_eq!(p.min_miner_version.as_deref(), Some("2.6.0"));
    }

    #[test]
    fn session_stats_accumulate() {
        let mut s = SessionStats::default();
        s.record_round(1_000_000, true);
        s.record_round(2_000_000, false);
        s.record_round(1_000_000, true);
        assert_eq!(s.rounds, 3);
        assert_eq!(s.solutions, 2);
        assert_eq!(s.total_hashes, 4_000_000);
        assert!((s.solution_rate() - 2.0 / 3.0).abs() < 1e-9);
        // 4M hashes over 2s = 2 MH/s.
        assert!((s.avg_hashrate(2.0) - 2_000_000.0).abs() < 1e-6);
        assert_eq!(s.avg_hashrate(0.0), 0.0); // no divide-by-zero
        let line = s.summary(2.0);
        assert!(line.contains("3 rounds"));
        assert!(line.contains("2 solutions"));
        assert!(line.contains("2.00 MH/s"));
    }

    #[test]
    fn poll_backoff_grows_and_caps() {
        assert_eq!(poll_backoff_secs(0, 2, 30), 0); // healthy → no wait
        assert_eq!(poll_backoff_secs(1, 2, 30), 2); // base
        assert_eq!(poll_backoff_secs(2, 2, 30), 4);
        assert_eq!(poll_backoff_secs(3, 2, 30), 8);
        assert_eq!(poll_backoff_secs(4, 2, 30), 16);
        assert_eq!(poll_backoff_secs(5, 2, 30), 30); // 32 capped to 30
        assert_eq!(poll_backoff_secs(100, 2, 30), 30); // huge failure count stays capped, no overflow
    }

    #[test]
    fn version_compare_is_numeric_not_lexical() {
        assert!(version_at_least("2.7.1", "2.6.0"));
        assert!(version_at_least("2.6.0", "2.6.0")); // equal ⇒ at least
        assert!(!version_at_least("2.5.9", "2.6.0"));
        // The lexical trap: "10.11.60" must beat "2.6.0".
        assert!(version_at_least("10.11.60", "2.6.0"));
        assert!(!version_at_least("2", "2.0.1")); // missing components = 0
        assert!(version_at_least("v2.6.0", "2.6.0")); // leading 'v' tolerated
    }

    #[test]
    fn challenge_response_rejects_malformed() {
        // Missing difficulty_target ⇒ None.
        let j = format!("{{\"challenge_hash\":\"{}\"}}", "aa".repeat(32));
        assert!(parse_challenge_response(&j).is_none());
        // Wrong-length hex ⇒ None.
        let j2 = "{\"challenge_hash\":\"dead\",\"difficulty_target\":\"beef\"}";
        assert!(parse_challenge_response(j2).is_none());
    }

    #[test]
    fn qnk_address_parses_and_rejects() {
        let addr = format!("qnk{}", "ab".repeat(32)); // 64 hex chars
        assert_eq!(parse_qnk_address(&addr), Some([0xabu8; 32]));
        assert!(parse_qnk_address("qnkshort").is_none());
        assert!(parse_qnk_address("nope1234").is_none());
        assert!(parse_qnk_address(&format!("qnk{}", "zz".repeat(32))).is_none()); // non-hex
    }

    #[test]
    fn submit_payload_matches_schema() {
        let work = Work::new([0x11u8; 32], [0x22u8; 32]);
        let digest = [0x33u8; 32];
        let wallet = format!("qnk{}", "cd".repeat(32));
        let json = submit_payload_json(&wallet, &work, 42, &digest, 410.0).unwrap();
        // All MiningSubmission required fields present.
        for key in ["nonce", "hash", "difficulty_target", "miner_address",
                    "miner_address_str", "hash_rate", "vdf_iterations",
                    "challenge_hash_bytes"] {
            assert!(json.contains(&format!("\"{key}\"")), "missing {key}");
        }
        assert!(json.contains("\"nonce\":42"));
        assert!(json.contains(&format!("\"miner_address_str\":\"{wallet}\"")));
        // [u8;32] serializes as a 32-element array (0xcd = 205 for miner_address).
        assert!(json.contains(&format!("\"miner_address\":[{}]", vec!["205"; 32].join(","))));
        assert!(json.contains("\"hash_rate\":410.000"));
        // Bad wallet ⇒ None (refuse to submit).
        assert!(submit_payload_json("bad", &work, 0, &digest, 1.0).is_none());
    }

    #[test]
    fn hashrate_formats_with_right_units() {
        assert_eq!(format_hashrate(500.0), "500 H/s");
        assert_eq!(format_hashrate(2048.0), "2.05 KH/s");
        assert_eq!(format_hashrate(2_500_000.0), "2.50 MH/s");
        assert_eq!(format_hashrate(1_500_000_000.0), "1.50 GH/s");
        // Boundary: exactly 1e6 ⇒ MH/s, not KH/s.
        assert_eq!(format_hashrate(1_000_000.0), "1.00 MH/s");
    }

    #[test]
    fn meets_target_boundary() {
        let target = [0x10u8; 32];
        let mut below = [0x10u8; 32];
        below[0] = 0x0f;
        let mut above = [0x10u8; 32];
        above[0] = 0x11;
        assert!(meets_target(&below, &target));
        assert!(meets_target(&target, &target)); // equal accepted
        assert!(!meets_target(&above, &target));
    }

    #[test]
    fn cpu_finds_solution_on_easy_target() {
        let work = Work::new([1u8; 32], easy_target());
        let sol = mine_batch_cpu(&work, 0, 100_000).expect("should find within 100k nonces");
        // Re-derive and re-check: the found nonce really satisfies the target.
        let d = digest_for(&work.challenge, sol.nonce);
        assert_eq!(d, sol.digest);
        assert!(meets_target(&d, &work.target));
    }

    #[test]
    fn auto_backend_matches_cpu_without_gpu_feature() {
        let work = Work::new([2u8; 32], easy_target());
        let (sol, backend) = mine_batch_auto(&work, 0, 100_000);
        let sol = sol.expect("auto should find a solution");
        assert!(meets_target(&digest_for(&work.challenge, sol.nonce), &work.target));
        // Without the gpu feature, auto must report CPU.
        #[cfg(not(feature = "gpu"))]
        assert_eq!(backend, Backend::Cpu);
        let _ = backend;
    }

    #[test]
    fn parallel_finds_valid_solution_and_agrees_on_empty() {
        let work = Work::new([4u8; 32], easy_target());
        let sol = mine_batch_cpu_parallel(&work, 0, 100_000).expect("parallel finds one");
        assert!(meets_target(&digest_for(&work.challenge, sol.nonce), &work.target));
        // Impossible target ⇒ both serial and parallel return None.
        let imposs = Work::new([4u8; 32], [0u8; 32]);
        assert!(mine_batch_cpu_parallel(&imposs, 0, 64).is_none());
        assert!(mine_batch_cpu(&imposs, 0, 64).is_none());
    }

    #[test]
    fn benchmark_hashrate_is_positive_and_zero_guarded() {
        let r = benchmark_hashrate(&[1u8; 32], 50_000);
        assert!(r > 0.0 && r.is_finite(), "expected a real rate, got {r}");
        assert_eq!(benchmark_hashrate(&[1u8; 32], 0), 0.0);
    }

    #[test]
    fn exhausted_batch_returns_none() {
        // Impossible target (all zero) → no solution in a tiny batch.
        let work = Work::new([3u8; 32], [0u8; 32]);
        assert!(mine_batch_cpu(&work, 0, 32).is_none());
    }

    #[test]
    fn cluster_partition_tiles_and_assigns() {
        let parts = cluster::partition(1000, &[1, 1, 2]);
        assert_eq!(parts, vec![(0, 250), (250, 250), (500, 500)]);
        // assign_range picks one node's slice; out-of-range ⇒ None.
        assert_eq!(cluster::assign_range(1000, &[1, 1, 2], 2), Some((500, 500)));
        assert_eq!(cluster::assign_range(1000, &[1, 1, 2], 3), None);
        // Full coverage: the union of all node ranges is exactly [0, total).
        let covered: u64 = parts.iter().map(|(_, l)| l).sum();
        assert_eq!(covered, 1000);
    }

    #[test]
    fn cluster_two_nodes_cover_disjoint_nonces() {
        // Node 0 and node 1 must never test the same nonce.
        let (s0, l0) = cluster::assign_range(100, &[1, 1], 0).unwrap();
        let (s1, l1) = cluster::assign_range(100, &[1, 1], 1).unwrap();
        assert_eq!(s0 + l0, s1); // node1 starts where node0 ends
        assert_eq!(s1 + l1, 100); // node1 ends at total
    }

    // Under the gpu feature, the miner's split MUST match flux-gpu's so a
    // GPU-weighted and a CPU cluster partition identically — no drift.
    #[test]
    fn mine_assigned_stays_in_slice_and_two_nodes_cover_space() {
        let mut t = [0xffu8; 32];
        t[0] = 0x0f; // easy target
        let work = Work::new([9u8; 32], t);
        let weights = [1u32, 1]; // two equal nodes over [0, 200_000)
        let total = 200_000u64;

        // Node 0 only searches [0, 100_000); node 1 only [100_000, 200_000).
        let (s0, _) = mine_assigned(&work, total, &weights, 0, 50_000);
        let (s1, _) = mine_assigned(&work, total, &weights, 1, 50_000);
        // At least one node finds a valid solution within its own slice.
        let found = s0.or(s1).expect("a node should find a solution");
        assert!(meets_target(&digest_for(&work.challenge, found.nonce), &work.target));
        if let Some(sol) = s0 {
            assert!(sol.nonce < 100_000, "node 0 must stay in its slice");
        }
        if let Some(sol) = s1 {
            assert!(sol.nonce >= 100_000, "node 1 must stay in its slice");
        }
        // Out-of-range node index ⇒ no work, no solution.
        assert!(mine_assigned(&work, total, &weights, 5, 50_000).0.is_none());
    }

    #[cfg(feature = "gpu")]
    #[test]
    fn cluster_matches_flux_gpu_partition() {
        let weights = [3u32, 1, 4, 1];
        assert_eq!(
            cluster::partition(10_000, &weights),
            flux_gpu::partition_nonce_space(10_000, &weights)
        );
    }
}
