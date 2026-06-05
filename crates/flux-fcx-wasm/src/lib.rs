//! flux-fcx-wasm — the FCX→Slint transpiler as a browser WASM module.
//!
//! Raw C-ABI (no wasm-bindgen / no CLI dependency). JS flow:
//!   1. `let p = fcx_alloc(bytes.len())` → write UTF-8 FCX source into wasm memory at p
//!   2. `let packed = fcx_transpile(p, len)` → returns (out_ptr << 32) | out_len  (u64/BigInt)
//!   3. read `out_len` bytes at `out_ptr` from `memory.buffer`, UTF-8 decode → Slint
//!
//! On a transpile error the output is `ERR: <message>` (still a valid UTF-8 string).
//! MVP leaks the output + input buffers (one transpile per keystroke is cheap); a
//! `fcx_free` can be added if churn ever matters.

use std::alloc::{alloc, Layout};

/// Allocate `len` bytes in wasm linear memory for the caller to write input into.
#[no_mangle]
pub extern "C" fn fcx_alloc(len: usize) -> *mut u8 {
    if len == 0 {
        return std::ptr::null_mut();
    }
    // align 1 is fine for a byte buffer
    let layout = Layout::from_size_align(len, 1).expect("layout");
    unsafe { alloc(layout) }
}

/// Transpile the UTF-8 FCX source at `ptr[..len]` to Slint. Returns a packed
/// `(out_ptr as u64) << 32 | (out_len as u64)`; on wasm32 the pointer is 32-bit
/// so both halves fit. The output buffer is leaked (read-only from JS).
#[no_mangle]
pub extern "C" fn fcx_transpile(ptr: *const u8, len: usize) -> u64 {
    let src = if ptr.is_null() || len == 0 {
        ""
    } else {
        let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
        std::str::from_utf8(slice).unwrap_or("ERR: input was not valid UTF-8")
    };

    let out = match flux_fcx::transpile_fcx(src) {
        Ok(slint) => slint,
        Err(e) => format!("ERR: {e}"),
    };

    let bytes = out.into_bytes();
    let out_len = bytes.len() as u64;
    let out_ptr = bytes.as_ptr() as u64;
    std::mem::forget(bytes); // hand ownership to JS (leak — fine for a REPL)
    (out_ptr << 32) | out_len
}
