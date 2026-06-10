//! quillonos-q-miner — browser-side BLAKE3 mining core.
//!
//! Compiled to wasm32-unknown-unknown as a cdylib. The host (os.html JS)
//! drives the loop:
//!
//!   1. host fetches GET /api/v1/mining/challenge?wallet=... → parses JSON.
//!   2. host writes the 32-byte `challenge_hash` and 32-byte `difficulty_target`
//!      into the wasm linear memory at offsets returned by `alloc`.
//!   3. host calls `mine_batch(chal_ptr, target_ptr, nonce_start, n_tries)`.
//!      Returns `u64::MAX` if nothing was found in the batch (host loops with
//!      bumped `nonce_start`); otherwise the winning nonce.
//!   4. host POSTs the solution to /api/v1/mining/submit.
//!
//! v0.1 doesn't compute the VDF output — submissions will be rejected by the
//! network until v0.2 adds the genus-2 jacobian VDF. The PoW hashing is real
//! though, so the tab is doing actual mining work end-to-end.
//!
//! WASM size target: <70 KB stripped. BLAKE3's portable backend (no SIMD,
//! no rayon) keeps the binary small and side-effect-free for the browser.

// no_std only for the wasm32 cdylib target; native workspace builds keep std
// (otherwise the panic_handler below duplicates std's panic_impl lang item).
#![cfg_attr(target_arch = "wasm32", no_std)]

use blake3::Hasher;

// ─── Scratch allocator ───────────────────────────────────────────────
//
// The host writes the 32-byte challenge + 32-byte target into a fixed
// scratch region. A bump pointer is enough — we never grow beyond 2 KB.

const SCRATCH_LEN: usize = 2048;
static mut SCRATCH: [u8; SCRATCH_LEN] = [0; SCRATCH_LEN];
static mut SCRATCH_HEAD: usize = 0;

/// Allocate `n` bytes from scratch. Returns a pointer the host writes into.
/// Returns a null offset (0) if the request exceeds remaining capacity.
#[no_mangle]
pub extern "C" fn alloc(n: usize) -> *mut u8 {
    unsafe {
        // SEC-014: checked_add so a pathological `n` near usize::MAX can't wrap
        // `SCRATCH_HEAD + n` past the bounds check and hand out an in-bounds
        // pointer with a bogus head. Overflow OR over-capacity → null.
        match SCRATCH_HEAD.checked_add(n) {
            Some(end) if end <= SCRATCH_LEN => {
                let p = SCRATCH.as_mut_ptr().add(SCRATCH_HEAD);
                SCRATCH_HEAD = end;
                p
            }
            _ => core::ptr::null_mut(),
        }
    }
}

/// Reset the scratch allocator. Host calls between challenges.
#[no_mangle]
pub extern "C" fn reset_scratch() {
    unsafe {
        SCRATCH_HEAD = 0;
    }
}

// ─── Mining ──────────────────────────────────────────────────────────

/// Run a BLAKE3 PoW batch.
///
/// Inputs:
/// * `challenge_ptr` — 32-byte challenge hash from the server.
/// * `target_ptr`    — 32-byte difficulty target (big-endian compare).
/// * `nonce_start`   — first nonce to try.
/// * `n_tries`       — how many sequential nonces to try.
///
/// Returns: the winning nonce, or `u64::MAX` if none in this batch satisfied
/// `hash < target`. Host should loop with `nonce_start += n_tries` until
/// the challenge expires or a winner is found.
///
/// Loop body:
///   BLAKE3(challenge || nonce.to_le_bytes()) → 32 bytes → big-endian compare to target.
///
/// Real q-miner also folds wallet + extra nonce material into the hash; we
/// keep it minimal here. If the network rejects submissions we'll evolve.
#[no_mangle]
pub extern "C" fn mine_batch(
    challenge_ptr: *const u8,
    target_ptr: *const u8,
    nonce_start: u64,
    n_tries: u64,
) -> u64 {
    if challenge_ptr.is_null() || target_ptr.is_null() {
        return u64::MAX;
    }
    let challenge = unsafe { core::slice::from_raw_parts(challenge_ptr, 32) };
    let target = unsafe { core::slice::from_raw_parts(target_ptr, 32) };

    // Hot loop. blake3 portable backend is ~120 MH/s on a modern x86 core
    // and roughly half that in wasm (vendor BLAKE3 SIMD doesn't reach into
    // wasm). Plenty for v0.1 demo of real PoW in the tab.
    for i in 0..n_tries {
        let nonce = nonce_start.wrapping_add(i);
        let mut h = Hasher::new();
        h.update(challenge);
        h.update(&nonce.to_le_bytes());
        let out = h.finalize();
        if hash_lt_target(out.as_bytes(), target) {
            return nonce;
        }
    }
    u64::MAX
}

/// Returns true if `hash` (big-endian) is strictly less than `target` (big-endian).
#[inline(always)]
fn hash_lt_target(hash: &[u8], target: &[u8]) -> bool {
    debug_assert_eq!(hash.len(), 32);
    debug_assert_eq!(target.len(), 32);
    for i in 0..32 {
        if hash[i] < target[i] {
            return true;
        }
        if hash[i] > target[i] {
            return false;
        }
    }
    // Equal → not strictly less than.
    false
}

/// Helper exported so the host can write the same comparison logic, for
/// solution verification before submitting.
#[no_mangle]
pub extern "C" fn hash_meets_target(hash_ptr: *const u8, target_ptr: *const u8) -> u32 {
    if hash_ptr.is_null() || target_ptr.is_null() {
        return 0;
    }
    let h = unsafe { core::slice::from_raw_parts(hash_ptr, 32) };
    let t = unsafe { core::slice::from_raw_parts(target_ptr, 32) };
    if hash_lt_target(h, t) {
        1
    } else {
        0
    }
}

// ─── BLAKE3 over arbitrary bytes (utility for the host) ──────────────

/// One-shot BLAKE3. Used by the host to verify a winning nonce before
/// submitting, or to derive intermediate state.
#[no_mangle]
pub extern "C" fn blake3_oneshot(
    in_ptr: *const u8,
    in_len: usize,
    out_ptr: *mut u8,
) {
    if in_ptr.is_null() || out_ptr.is_null() {
        return;
    }
    let input = unsafe { core::slice::from_raw_parts(in_ptr, in_len) };
    let out = blake3::hash(input);
    unsafe {
        core::ptr::copy_nonoverlapping(out.as_bytes().as_ptr(), out_ptr, 32);
    }
}

// ─── Build info ──────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn miner_version() -> u32 {
    // Encodes "0.1.0" → 0x000100 (major.minor.patch each as 8 bits).
    0x0001_00
}

// ─── Panic handler (size matters; abort is fine) ─────────────────────
// wasm32 only: native builds (e.g. `fluxc build` over the whole workspace)
// use std's panic_impl — defining our own there is a duplicate-lang-item error.

#[cfg(target_arch = "wasm32")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // No std::process::abort in wasm32-unknown-unknown — divergent loop is
    // the canonical small alternative.
    loop {}
}
