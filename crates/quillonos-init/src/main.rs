// quillonos-init — the OS's first process.
//
// In a real Linux-like setup, pid 1 supervises orphaned children, mounts /,
// and launches getty. In QuillonOS the kernel is the browser tab's host shim,
// so all init does is:
//
//   1. Print the boot banner so the terminal has something to display
//      while the JS loader is still wiring shims.
//   2. Report the WASI environment + argv it was given, so future shells
//      can audit the boot record by `cat /proc/init/argv`.
//   3. Exit 0. The host then proceeds to load `sh.wasm`.
//
// This file is intentionally ~zero-dep — every byte ships across the wire
// on first boot. Total wasm should land well under 50 KB stripped.

use std::env;
use std::io::{self, Write};
use std::process::ExitCode;

const BANNER: &str = r#"
   ╔═══════════════════════════════════════════════╗
   ║           ⚡  QuillonOS v0.1.0-alpha          ║
   ║          wasi-preview1 / fluxc 0.17.0         ║
   ║       SQIsign L5 — the OS is the proof.       ║
   ╚═══════════════════════════════════════════════╝
"#;

fn main() -> ExitCode {
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "{BANNER}");

    let _ = writeln!(out, "[init] pid 1 — boot phase 4 (wasm modules verified)");
    let _ = writeln!(out, "[init] argv: {:?}", env::args().collect::<Vec<_>>());
    let _ = writeln!(out, "[init] env QUILLONOS_KERNEL={}",
        env::var("QUILLONOS_KERNEL").unwrap_or_else(|_| "wasi-preview1".into()));
    let _ = writeln!(out, "[init] env QUILLONOS_AGENT_WALLET={}",
        env::var("QUILLONOS_AGENT_WALLET").unwrap_or_else(|_| "(none)".into()));
    let _ = writeln!(out, "[init] handing control to /bin/sh");
    let _ = writeln!(out, "");
    ExitCode::SUCCESS
}
