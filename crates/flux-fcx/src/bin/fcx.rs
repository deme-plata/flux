//! `fcx` — the FCX command line.
//!
//! ```text
//! fcx slint <file.fcx>                 print the transpiled .slint to stdout
//! fcx pack  <file.fcx> <out> [name]    emit a buildable Slint app project
//! ```
//!
//! `pack` writes Cargo.toml + build.rs + ui.slint + src/main.rs into <out>;
//! then `fluxc build` (in <out>) cross-compiles a native desktop binary —
//! the Electron-killer payoff: authored in FCX/TS, rendered by Slint, no
//! Chromium, no bundled JS runtime.

use anyhow::{bail, Result};
use std::path::Path;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("slint") => {
            let file = args.get(2).ok_or_else(|| anyhow::anyhow!("fcx slint <file.fcx>"))?;
            let src = std::fs::read_to_string(file)?;
            print!("{}", flux_fcx::transpile_fcx(&src)?);
        }
        Some("pack") => {
            let file = args.get(2).ok_or_else(|| anyhow::anyhow!("fcx pack <file.fcx> <out> [name]"))?;
            let out = args.get(3).ok_or_else(|| anyhow::anyhow!("fcx pack <file.fcx> <out> [name]"))?;
            let name = args.get(4).map(String::as_str).unwrap_or("fcx-app");
            let src = std::fs::read_to_string(file)?;
            let written = flux_fcx::write_app(&src, name, Path::new(out))?;
            for p in written {
                println!("wrote {}", p.display());
            }
            println!("\nnext:  cd {out} && fluxc build            # native binary");
            println!("       cd {out} && fluxc build --target x86_64-pc-windows-gnu   # .exe");
        }
        Some("dev") => {
            let file = args.get(2).ok_or_else(|| anyhow::anyhow!("fcx dev <file.fcx>"))?;
            let fcx_path = Path::new(file);
            let out = fcx_path.with_extension("slint");
            println!("fcx dev — live: {} → {}", fcx_path.display(), out.display());
            flux_fcx::dev::transpile_file(fcx_path, &out)?; // write before viewer attaches
            let _viewer = flux_fcx::dev::spawn_viewer(&out);
            println!("watching for edits — save the .fcx and the window reloads. Ctrl-C to stop.");
            flux_fcx::dev::watch(
                fcx_path,
                &out,
                std::time::Duration::from_millis(300),
                |res| match res {
                    Ok(()) => println!("↻ reloaded {}", out.display()),
                    Err(e) => eprintln!("✗ transpile error (fix & save): {e}"),
                },
                || false, // run until Ctrl-C
            )?;
        }
        _ => {
            bail!("usage: fcx slint <f.fcx> | fcx pack <f.fcx> <out> [name] | fcx dev <f.fcx>");
        }
    }
    Ok(())
}
