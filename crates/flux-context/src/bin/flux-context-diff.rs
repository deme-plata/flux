//! flux-context-diff — diff the current workspace context against the latest
//! snapshot, and optionally save a new snapshot.
//!
//! Usage: `flux-context-diff [workspace_root] [--save]`  (root defaults to cwd)

use std::path::PathBuf;

fn main() {
    let mut root = std::env::current_dir().expect("cwd");
    let mut do_save = false;
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--save" => do_save = true,
            other => root = PathBuf::from(other),
        }
    }
    let ctx_dir = root.join(".whale/context");

    let manifest = match flux_context::chunk::compute_manifest(&root) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("flux-context-diff: {e}");
            std::process::exit(1);
        }
    };
    let now_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let version = flux_context::diff::next_version(&ctx_dir);
    let new_snap = flux_context::diff::ContextSnapshot::from_manifest(&manifest, version, now_ns);

    match flux_context::diff::load_latest(&ctx_dir) {
        Some(old) => {
            let d = flux_context::diff::diff_snapshots(&old, &new_snap);
            if d.is_empty() {
                eprintln!(
                    "context-diff v{}→v{}: no changes ({} chunks, Δ{} tokens)",
                    d.from_version, version, manifest.crate_count, d.delta_tokens
                );
            } else {
                eprintln!(
                    "context-diff v{}→v{}: +{} added · ~{} modified · -{} deleted · {} stale-ripple · Δ{} tokens",
                    d.from_version, version,
                    d.added.len(), d.modified.len(), d.deleted.len(), d.stale_ripple.len(), d.delta_tokens
                );
                for p in &d.added { eprintln!("  + {p}"); }
                for p in &d.modified { eprintln!("  ~ {p}"); }
                for p in &d.deleted { eprintln!("  - {p}"); }
                for (p, o, n) in d.stale_ripple.iter().take(12) {
                    eprintln!("  Δripple {p}: {o:.3}→{n:.3}");
                }
            }
        }
        None => eprintln!(
            "context-diff: no prior snapshot — baseline will be v{version} ({} chunks, ~{} tokens)",
            manifest.crate_count, manifest.total_tokens_estimated
        ),
    }

    if do_save {
        match flux_context::diff::save_snapshot(&ctx_dir, &new_snap) {
            Ok(h) => eprintln!("saved snapshot v{version} → {}", &h[..16.min(h.len())]),
            Err(e) => {
                eprintln!("flux-context-diff: save error: {e}");
                std::process::exit(1);
            }
        }
    }
}
