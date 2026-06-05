// quillonos-uname — prints QuillonOS host info from the env the host shim
// hands us. Validates the `environ_get` / `environ_sizes_get` WASI surface
// end-to-end.
//
// `uname -a` prints everything. `uname -s` prints the kernel only, etc.
// Matches POSIX-ish flags so future composability with `sh -c` is natural.

use std::env;
use std::process::ExitCode;

fn val(key: &str, fallback: &str) -> String {
    env::var(key).unwrap_or_else(|_| fallback.into())
}

fn main() -> ExitCode {
    let kernel    = val("QUILLONOS_KERNEL",       "wasi-preview1");
    let compiler  = val("QUILLONOS_COMPILER",     "fluxc 0.17.0");
    let version   = val("QUILLONOS_VERSION",      "0.1.0-alpha");
    let wallet    = val("QUILLONOS_AGENT_WALLET", "(none)");
    let pwd       = val("PWD",                    "/");
    let machine   = "wasm32-wasip1";
    let hostname  = "quillon.xyz";
    let signing   = "SQIsign Level 5 (pending Slice \u{03b2})";

    let mut want_all  = true;
    let mut want_kern = false;
    let mut want_node = false;
    let mut want_mach = false;
    let mut want_ver  = false;
    for arg in env::args().skip(1) {
        match arg.as_str() {
            "-a" | "--all"      => { want_all = true; }
            "-s" | "--kernel-name"    => { want_kern = true; want_all = false; }
            "-n" | "--nodename" => { want_node = true; want_all = false; }
            "-m" | "--machine"  => { want_mach = true; want_all = false; }
            "-r" | "--kernel-release" | "-v" | "--kernel-version"
                                => { want_ver  = true; want_all = false; }
            "-h" | "--help"     => {
                println!("uname — print QuillonOS host info");
                println!();
                println!("flags:");
                println!("  -a   all (default)");
                println!("  -s   kernel name");
                println!("  -n   hostname");
                println!("  -m   machine triple");
                println!("  -r   kernel release / OS version");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("uname: unknown flag {other:?} — try `uname -h`");
                return ExitCode::from(2);
            }
        }
    }

    if want_all {
        // Full multi-line readout — friendly for the terminal welcome
        // case `:about` / `uname` at the prompt.
        println!("kernel       {kernel}");
        println!("machine      {machine}");
        println!("compiler     {compiler}");
        println!("version      {version}");
        println!("hostname     {hostname}");
        println!("signing      {signing}");
        println!("citizen      {}", short_wallet(&wallet));
        println!("cwd          {pwd}");
        return ExitCode::SUCCESS;
    }

    // POSIX-shaped single-line outputs. Each flag prints exactly its piece.
    let mut bits = Vec::new();
    if want_kern { bits.push(kernel.clone()); }
    if want_node { bits.push(hostname.into()); }
    if want_mach { bits.push(machine.into()); }
    if want_ver  { bits.push(version.clone()); }
    println!("{}", bits.join(" "));
    ExitCode::SUCCESS
}

fn short_wallet(w: &str) -> String {
    if w.len() < 12 { return w.into(); }
    format!("{}…{}", &w[..6], &w[w.len() - 4..])
}
