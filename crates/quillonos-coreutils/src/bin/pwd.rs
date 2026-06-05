//! pwd — QuillonOS coreutils
//!
//! Prints the current working directory. In a WASI browser context the
//! "cwd" is what the host shim set when it invoked us — typically
//! /home/citizen on first boot. The shell tracks `cd` outside the
//! sandbox and re-sets cwd per invocation.

use std::env;

fn main() {
    match env::current_dir() {
        Ok(p)  => println!("{}", p.display()),
        Err(_) => println!("/"),
    }
}
