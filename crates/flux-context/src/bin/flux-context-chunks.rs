//! flux-context-chunks — emit the semantic-chunk manifest for a workspace.
//!
//! Usage: `flux-context-chunks [workspace_root]`  (defaults to cwd)
//! Writes `<root>/.whale/context/chunks.json` and prints the top-15 by ripple.

use std::path::PathBuf;

fn main() {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("cwd"));

    let manifest = match flux_context::chunk::compute_manifest(&root) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("flux-context-chunks: {e}");
            std::process::exit(1);
        }
    };

    let out = root.join(".whale/context/chunks.json");
    if let Some(parent) = out.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string_pretty(&manifest) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&out, json) {
                eprintln!("flux-context-chunks: write {}: {e}", out.display());
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("flux-context-chunks: serialize: {e}");
            std::process::exit(1);
        }
    }

    eprintln!(
        "flux-context: {} crates · ~{} tokens → {}",
        manifest.crate_count,
        manifest.total_tokens_estimated,
        out.display()
    );
    eprintln!("Top-15 by dependency ripple:");
    for c in manifest.chunks.iter().take(15) {
        eprintln!(
            "  {:>5.3}  {:<26} {:<10?} ~{:>6}tok  ({} deps, {} dependents)",
            c.ripple_score,
            c.crate_name,
            c.category,
            c.estimated_tokens,
            c.deps.len(),
            c.rev_deps.len()
        );
    }
}
