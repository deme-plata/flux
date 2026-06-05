// quillonos-readfile — demo binary that reads a file via std::fs.
// Validates the WasiHost's OPFS-backed path_open + fd_read shims:
// the JS host pre-loads any absolute-path argv entry from OPFS into
// an in-memory map, path_open resolves against it, fd_read serves
// the bytes.
//
// Why a separate "readfile" rather than touching rocky-arena-1's cat:
// keeps blast radius contained to my own crate while proving the
// shim works. Once verified, rocky-arena-1's cat.wasm rides the same
// path without changes.

use std::env;
use std::fs;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = env::args().skip(1).peekable();
    if args.peek().is_none() {
        eprintln!("readfile: missing path (try `readfile /home/citizen/file.txt`)");
        eprintln!("readfile: this binary tests OPFS-WASI mount — paths must already exist in your OPFS root");
        return ExitCode::from(2);
    }

    let mut exit = ExitCode::SUCCESS;
    for path in args {
        match fs::read_to_string(&path) {
            Ok(s) => {
                // Print verbatim — no extra newline. cat-like.
                print!("{s}");
            }
            Err(e) => {
                eprintln!("readfile: {path}: {e}");
                exit = ExitCode::from(1);
            }
        }
    }
    exit
}
