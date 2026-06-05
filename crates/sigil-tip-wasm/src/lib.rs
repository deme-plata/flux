//! sigil-tip-wasm — the SIGIL lightweight node, in the browser.
//!
//! A joining client verifies a tip-proof (height + 4 state roots + flavor +
//! signature) in ≤10ms instead of downloading the chain. This runs that exact
//! `sigil_tip_proof::TipProof::verify` as WASM, client-side — no server, no
//! transport stack (sigil-net's WireGuard/Tor are feature-gated out).
//!
//! Raw C-ABI (no wasm-bindgen), identical recipe to flux-fcx-wasm:
//!   1. `p = tip_alloc(len)` → write the proof JSON (UTF-8) at p
//!   2. `packed = tip_verify(p, len)` → (out_ptr << 32) | out_len  (u64/BigInt)
//!   3. read out_len bytes at out_ptr → a JSON verdict:
//!        {"ok":true,"height":N,"flavor":"Blake3Fingerprint","network":"sigil-g0"}
//!        {"ok":false,"error":"..."}

use sigil_tip_proof::{TipProof, NETWORK_ID_BYTES};
use std::alloc::{alloc, Layout};

#[no_mangle]
pub extern "C" fn tip_alloc(len: usize) -> *mut u8 {
    if len == 0 {
        return std::ptr::null_mut();
    }
    unsafe { alloc(Layout::from_size_align(len, 1).expect("layout")) }
}

#[no_mangle]
pub extern "C" fn tip_verify(ptr: *const u8, len: usize) -> u64 {
    let out = verdict(ptr, len);
    let bytes = out.into_bytes();
    let out_len = bytes.len() as u64;
    let out_ptr = bytes.as_ptr() as u64;
    std::mem::forget(bytes); // hand to JS (leak — fine for a verify-on-tap REPL)
    (out_ptr << 32) | out_len
}

fn verdict(ptr: *const u8, len: usize) -> String {
    if ptr.is_null() || len == 0 {
        return err("empty input");
    }
    let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
    let proof: TipProof = match serde_json::from_slice(slice) {
        Ok(p) => p,
        Err(e) => return err(&format!("bad proof json: {e}")),
    };
    // The light client pins the network id (sigil-g0) and verifies against it —
    // exactly the full node's check, just running in your tab.
    match proof.verify(NETWORK_ID_BYTES) {
        Ok(()) => format!(
            "{{\"ok\":true,\"height\":{},\"flavor\":\"{:?}\",\"network\":\"sigil-g0\"}}",
            proof.height, proof.flavor
        ),
        Err(e) => err(&e.to_string()),
    }
}

fn err(m: &str) -> String {
    format!("{{\"ok\":false,\"error\":\"{}\"}}", m.replace('"', "'"))
}
