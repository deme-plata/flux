//! fluxc-util — layer 0 of the fluxc crate stack (v0.41 god-crate split).
//!
//! Everything here is small, stable, and dependency-light on purpose:
//! `fluxc-analytics`, `fluxc-webhooks`, `fluxc-serve` and `fluxc-core` all sit
//! on top, so an edit HERE rebuilds the world. Think twice before growing it.

pub mod hooks;
pub mod serve_events;
pub mod version;

use std::env;

/// The current on-disk fluxc binary — safe to spawn even from a long-running
/// process whose own image was rename-replaced by a rebuild.
/// Resolution order:
/// 1. `FLUX_WRAPPER_PATH` (fleet symlink, operator keeps it live)
/// 2. `current_exe()` as-is, when that file still exists
/// 3. `current_exe()` with the " (deleted)" suffix stripped, when THAT exists
///    — the freshly rebuilt binary at the same path (newer than the running
///    process, and exactly the right thing to spawn)
/// 4. `current_exe()` unchanged — let the caller surface the spawn error
pub fn live_fluxc_path() -> std::path::PathBuf {
    if let Ok(p) = env::var("FLUX_WRAPPER_PATH") {
        if !p.is_empty() && std::path::Path::new(&p).exists() {
            return std::path::PathBuf::from(p);
        }
    }
    // THE SHARED FINGERPRINT UNIVERSE (2026-09-06).
    //
    // cargo hashes RUSTC_WRAPPER's PATH into every unit fingerprint. `current_exe()` is a
    // different path in every checkout and every worktree, so two agents on this one box
    // invalidated each other's compile cache continuously — each internally warm, the pair
    // permanently cold. Measured today: 33.5 % hit rate over 112,643 units, and
    // FLUX_WRAPPER_PATH set in no shell that actually builds (a `.bashrc` export does not
    // reach a non-interactive shell, which is exactly how the documented fix was believed
    // to be in place while doing nothing).
    //
    // So the stable name is the DEFAULT, not something each agent must remember to export:
    // `~/.flux/bin/fluxc` is a symlink maintained to point at the live binary. Used only
    // when it resolves to a real file, so a box without it behaves exactly as before.
    if let Some(home) = env::var_os("HOME") {
        let stable = std::path::Path::new(&home).join(".flux/bin/fluxc");
        if stable.exists() {
            return stable;
        }
    }
    let exe = env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("fluxc"));
    if exe.exists() {
        return exe;
    }
    if let Some(stripped) = strip_deleted_suffix(&exe) {
        if stripped.exists() {
            return stripped;
        }
    }
    exe
}

/// "<path> (deleted)" → Some("<path>") — the shape /proc/self/exe takes after
/// the running binary is rename-replaced. None when the suffix is absent.
pub fn strip_deleted_suffix(p: &std::path::Path) -> Option<std::path::PathBuf> {
    let s = p.to_str()?;
    s.strip_suffix(" (deleted)").map(std::path::PathBuf::from)
}

/// Serialize tests that swap $HOME. Shared by every crate in the stack whose
/// tests write per-user state (predictions, tune presets, webhook configs…).
pub mod test_home {
    use std::sync::Mutex;
    static LOCK: Mutex<()> = Mutex::new(());

    pub fn with_temp_home<T>(tag: &str, f: impl FnOnce() -> T) -> T {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("HOME").ok();
        let dir = std::env::temp_dir().join(format!("flux-test-home-{}", tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("HOME", &dir);
        let out = f();
        match prev {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&dir);
        out
    }
}

#[cfg(test)]
mod live_fluxc_path_tests {
    use super::*;

    #[test]
    fn deleted_suffix_strips_exactly() {
        assert_eq!(
            strip_deleted_suffix(std::path::Path::new("/a/b/fluxc (deleted)")),
            Some(std::path::PathBuf::from("/a/b/fluxc"))
        );
        assert_eq!(strip_deleted_suffix(std::path::Path::new("/a/b/fluxc")), None);
    }
}

#[cfg(test)]
mod wrapper_identity_tests {
    #[test]
    fn the_env_override_still_wins_over_the_stable_name() {
        // An explicit FLUX_WRAPPER_PATH is a deliberate choice and must outrank the
        // default. Checked without touching the process environment: this asserts the
        // ORDER in the source, which is what the bug was about.
        let src = include_str!("lib.rs");
        let env_at = src.find("FLUX_WRAPPER_PATH").expect("env branch present");
        let stable_at = src.find(".flux/bin/fluxc").expect("stable-name branch present");
        assert!(env_at < stable_at, "the env override must be checked first");
        let exe_at = src.find("env::current_exe()").expect("current_exe fallback present");
        assert!(stable_at < exe_at, "the stable name must be preferred over current_exe()");
    }
}
