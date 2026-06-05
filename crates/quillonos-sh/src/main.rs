// quillonos-sh — minimal POSIX-shaped shell.
//
// The browser shim does the actual exec: it owns the OPFS filesystem, the
// keyboard input, and the WebAssembly.instantiate boundary. This binary
// implements the parts that *should* be inside the userspace:
//
//   1. Argv-driven dispatch (so `sh -c "echo hi"` works headlessly when
//      a future remote agent SSH-style shells in).
//   2. Builtin commands the host shim shouldn't reimplement: `help`,
//      `:version`, `:modules`, `:env`.
//   3. A printed "dispatch plan" for any unknown command so the host
//      can pick it up and call WebAssembly.instantiate on the right
//      `wasm/<name>.wasm` file.
//
// The interactive line-loop lives in the host shim because POSIX stdin
// in a browser tab is awkward — it's a polled BroadcastChannel, not a
// blocking read. This binary's stdin-loop path is reachable via `sh -i`
// when run via wasmtime locally for testing.

use std::env;
use std::io::{self, BufRead, Write};
use std::process::ExitCode;

const BUILTINS: &[(&str, &str)] = &[
    ("help",      "List builtins."),
    (":version",  "Print QuillonOS + shell version."),
    (":modules",  "List runtime modules known to the manifest."),
    (":env",      "Dump environment variables."),
    ("exit",      "Quit the shell (host shim closes the tab)."),
];

const KNOWN_MODULES: &[&str] = &["init", "sh", "echo", "cat", "pwd"];

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let mut out = io::stdout().lock();

    // Non-interactive: `sh -c "echo hi"`
    if let Some(idx) = args.iter().position(|a| a == "-c") {
        if let Some(cmd) = args.get(idx + 1) {
            return run_one(&mut out, cmd);
        }
        let _ = writeln!(out, "sh: -c requires an argument");
        return ExitCode::from(2);
    }

    // Interactive (works under wasmtime; in browser the host shim drives this).
    if args.iter().any(|a| a == "-i") {
        return run_interactive();
    }

    // Default: print one-line dispatch help, then exit.
    let _ = writeln!(out, "quillonos-sh — type :help for builtins, or pass `-c \"cmd\"`");
    ExitCode::SUCCESS
}

fn run_interactive() -> ExitCode {
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    loop {
        let _ = write!(stdout, "$ ");
        let _ = stdout.flush();
        let mut line = String::new();
        if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
            return ExitCode::SUCCESS;
        }
        let line = line.trim();
        if line.is_empty() { continue; }
        if line == "exit" { return ExitCode::SUCCESS; }
        let _ = run_one(&mut stdout, line);
    }
}

fn run_one(out: &mut io::StdoutLock<'_>, cmd: &str) -> ExitCode {
    let mut parts = cmd.split_whitespace();
    let head = match parts.next() {
        Some(h) => h,
        None => return ExitCode::SUCCESS,
    };
    let rest: Vec<&str> = parts.collect();

    // Builtins.
    match head {
        "help" | ":help" => {
            let _ = writeln!(out, "sh builtins:");
            for (n, d) in BUILTINS { let _ = writeln!(out, "  {:<10} {}", n, d); }
            return ExitCode::SUCCESS;
        }
        ":version" => {
            let _ = writeln!(out, "quillonos-sh {}", env!("CARGO_PKG_VERSION"));
            let _ = writeln!(out, "fluxc 0.17.0 / wasm32-wasip1 / SQIsign L5");
            return ExitCode::SUCCESS;
        }
        ":modules" => {
            let _ = writeln!(out, "modules (per manifest.json):");
            for m in KNOWN_MODULES { let _ = writeln!(out, "  {}", m); }
            return ExitCode::SUCCESS;
        }
        ":env" => {
            for (k, v) in env::vars() { let _ = writeln!(out, "{}={}", k, v); }
            return ExitCode::SUCCESS;
        }
        _ => {}
    }

    // External: print a dispatch line the host shim picks up.
    // Format is stable so JS regex parsing is trivial.
    let _ = writeln!(out, "[sh:dispatch] module={head} argv={rest:?}");
    ExitCode::SUCCESS
}
