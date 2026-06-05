//! cat — QuillonOS coreutils
//!
//! Reads each argv file path and writes its contents to stdout. The host
//! WASI shim mounts the user's OPFS as the wasi filesystem root, so
//! `cat /home/citizen/welcome.txt` reads the real persistent file the
//! browser stores.

use std::fs::File;
use std::io::{copy, stdout, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1).peekable();
    if args.peek().is_none() {
        // POSIX cat with no args reads stdin; in the browser shim, stdin
        // is the line the shell just captured (empty by default). For
        // v0.1 we just print a friendly hint instead of hanging.
        eprintln!("cat: missing file operand (try `cat <path>`)");
        return ExitCode::from(2);
    }

    let mut exit = ExitCode::SUCCESS;
    let mut out = stdout().lock();
    for path in args {
        match File::open(&path) {
            Ok(mut f) => {
                if let Err(e) = copy(&mut f, &mut out) {
                    let _ = writeln!(std::io::stderr(), "cat: {}: {}", path, e);
                    exit = ExitCode::from(1);
                }
            }
            Err(e) => {
                let _ = writeln!(std::io::stderr(), "cat: {}: {}", path, e);
                exit = ExitCode::from(1);
            }
        }
    }
    exit
}
