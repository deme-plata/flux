//! echo — QuillonOS coreutils
//!
//! Compiles to wasm32-wasip1. Loaded by quillon.xyz/os.html as the
//! first non-stub WASI module: when the user types `echo foo bar`, the
//! shell fetches echo.wasm, instantiates it with WASI imports, sets argv
//! via the shim, and reads stdout from a captured pipe.
//!
//! Behaves like POSIX echo — `-n` suppresses the trailing newline.

fn main() {
    let mut args = std::env::args().skip(1);
    let mut suppress_newline = false;
    let mut first = true;

    // POSIX-ish: -n suppresses newline; everything else is text.
    let mut buf = String::new();
    while let Some(a) = args.next() {
        if first && a == "-n" {
            suppress_newline = true;
            first = false;
            continue;
        }
        first = false;
        if !buf.is_empty() {
            buf.push(' ');
        }
        buf.push_str(&a);
    }

    if suppress_newline {
        print!("{}", buf);
    } else {
        println!("{}", buf);
    }
}
