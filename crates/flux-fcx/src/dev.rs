//! FCX-HMR — edit a `.fcx`, watch the native window update live.
//!
//! Slint bakes its UI in at compile time, so there's no in-process hot-swap for
//! a normal `fcx pack` binary. Instead `fcx dev` keeps a `.slint` file in sync
//! with the `.fcx` on every save and lets **`slint-viewer`** (Slint's native
//! live-preview, which already hot-reloads the file it's watching) render the
//! window. So the loop is: edit `.fcx` → we re-transpile → rewrite `ui.slint`
//! → slint-viewer notices and reloads → the native window updates. No rebuild.
//!
//! This module is std-only (no new deps): a millisecond-cheap mtime poll. The
//! viewer is spawned best-effort; if it isn't installed the sync loop still
//! runs (so a separately-launched viewer or `fluxc serve` preview picks it up).

use anyhow::{Context, Result};
use std::path::Path;
use std::time::{Duration, SystemTime};

/// Transpile `fcx_path` → write `.slint` to `out_path`. Returns the slint text.
pub fn transpile_file(fcx_path: &Path, out_path: &Path) -> Result<String> {
    let src = std::fs::read_to_string(fcx_path)
        .with_context(|| format!("reading {}", fcx_path.display()))?;
    let slint = crate::transpile_fcx(&src)?;
    std::fs::write(out_path, &slint)
        .with_context(|| format!("writing {}", out_path.display()))?;
    Ok(slint)
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

/// Watch `fcx_path`; on every change re-run [`transpile_file`] into `out_path`
/// and invoke `on_reload(Ok|Err)`. Polls every `poll`. Runs until `should_stop`
/// returns true (use `|| false` to run forever). Returns the number of reloads.
///
/// Transpile errors are reported via `on_reload` but do not stop the loop — fix
/// the `.fcx` and save again, exactly like a web HMR server.
pub fn watch<F, S>(
    fcx_path: &Path,
    out_path: &Path,
    poll: Duration,
    mut on_reload: F,
    mut should_stop: S,
) -> Result<u64>
where
    F: FnMut(Result<()>),
    S: FnMut() -> bool,
{
    // initial transpile
    on_reload(transpile_file(fcx_path, out_path).map(|_| ()));
    let mut last = mtime(fcx_path);
    let mut reloads = 0u64;
    while !should_stop() {
        std::thread::sleep(poll);
        let now = mtime(fcx_path);
        if now != last && now.is_some() {
            last = now;
            reloads += 1;
            on_reload(transpile_file(fcx_path, out_path).map(|_| ()));
        }
    }
    Ok(reloads)
}

/// Spawn `slint-viewer <slint_path>` if it's on PATH. Best-effort: returns the
/// child on success, or `None` with a hint logged when the viewer is absent.
pub fn spawn_viewer(slint_path: &Path) -> Option<std::process::Child> {
    match std::process::Command::new("slint-viewer").arg(slint_path).spawn() {
        Ok(child) => Some(child),
        Err(_) => {
            eprintln!(
                "  (slint-viewer not found — install with `cargo install slint-viewer`, \
                 or point one at {} yourself; the sync loop is running regardless)",
                slint_path.display()
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("fcx-dev-{}-{}", std::process::id(), name))
    }

    const A: &str = "export function A() { return ( <text>one</text> ); }";
    const B: &str = "export function A() { return ( <text>two</text> ); }";

    #[test]
    fn transpile_file_writes_valid_slint() {
        let fcx = tmp("a.fcx");
        let out = tmp("a.slint");
        std::fs::write(&fcx, A).unwrap();
        let slint = transpile_file(&fcx, &out).unwrap();
        assert!(slint.contains("export component A inherits Window {"));
        assert!(std::fs::read_to_string(&out).unwrap().contains("one"));
        let _ = std::fs::remove_file(&fcx);
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn watch_reloads_on_change_and_survives_errors() {
        let fcx = tmp("w.fcx");
        let out = tmp("w.slint");
        std::fs::write(&fcx, A).unwrap();

        // Drive the loop deterministically: mutate the file across iterations,
        // then stop. Counts reloads + proves a bad edit doesn't kill the loop.
        let mut step = 0;
        let reloads = watch(
            &fcx,
            &out,
            Duration::from_millis(1),
            |_res| {},
            || {
                step += 1;
                match step {
                    2 => {
                        // a broken edit — transpile will Err, loop must continue
                        std::fs::write(&fcx, "garbage not fcx").unwrap();
                        // nudge mtime forward
                        std::thread::sleep(Duration::from_millis(5));
                        false
                    }
                    4 => {
                        std::fs::write(&fcx, B).unwrap();
                        std::thread::sleep(Duration::from_millis(5));
                        false
                    }
                    n if n >= 8 => true, // stop
                    _ => false,
                }
            },
        )
        .unwrap();

        assert!(reloads >= 2, "expected >=2 reloads (broken + good), got {reloads}");
        // final good edit landed
        assert!(std::fs::read_to_string(&out).unwrap().contains("two"));
        let _ = std::fs::remove_file(&fcx);
        let _ = std::fs::remove_file(&out);
    }
}
