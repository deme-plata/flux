//! Build + headless render-verify a Vite project, the agent way.
//!
//!   cargo run --example verify -- /path/to/vite-app [screenshot.png]
//!
//! Runs `vite build`, grades it (build-SAP score), then loads the built page
//! in headless Chromium and checks it rendered without console errors —
//! returning a screenshot. Exit code is non-zero if the build or render fails,
//! so it drops straight into a deploy gate.

use flux_vite_engine::{build_project, verify_dist, ViteConfig};
use std::path::PathBuf;

fn human(b: u64) -> String {
    let u = ["B", "KB", "MB"];
    let (mut v, mut i) = (b as f64, 0);
    while v >= 1024.0 && i < u.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{:.1} {}", v, u[i])
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let project = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    let shot = PathBuf::from(
        std::env::args().nth(2).unwrap_or_else(|| "/home/storage/tmp/vite-verify.png".into()),
    );

    println!("▶ build + verify  {}", project.display());
    let cfg = ViteConfig::for_project(&project);

    // ── build ──
    let b = build_project(&cfg).await?;
    println!("\n── build ──  {}", if b.ok { "✓ OK" } else { "✗ FAILED" });
    println!("  time:    {} ms", b.build_ms);
    println!("  assets:  {} ({} total, {} gzip, {} js-gzip)", b.assets.len(), human(b.total_bytes), human(b.total_gzip_bytes), human(b.js_gzip_bytes));
    for a in &b.assets {
        println!("    {:<34} {:>9}{}", a.name, human(a.bytes), a.gzip_bytes.map(|g| format!("  gz {}", human(g))).unwrap_or_default());
    }
    for w in &b.warnings {
        println!("  ⚠ {w}");
    }
    for e in &b.errors {
        println!("  ✗ {e}");
    }
    println!(
        "  build-SAP: {}/100  (speed {} · errors {} · bundle {} · warns {})",
        b.score.composite, b.score.speed, b.score.errors_clean, b.score.bundle_health, b.score.warnings_clean
    );

    if !b.ok {
        eprintln!("\n✗ build failed — skipping render");
        std::process::exit(1);
    }

    // ── verify (headless render) ──
    let v = verify_dist(&project, &shot).await?;
    println!("\n── render ──  {}", if v.ok { "✓ OK" } else { "✗ ISSUE" });
    println!("  chrome:     {}", v.chrome.display());
    println!("  rendered:   {} (DOM {} chars)", v.rendered, v.dom_chars);
    println!("  console:    {} error(s), {} warning(s)", v.console_errors.len(), v.console_warnings.len());
    for e in v.console_errors.iter().take(8) {
        println!("    ✗ {e}");
    }
    if let Some(p) = &v.screenshot {
        println!("  screenshot: {}", p.display());
    }
    println!("  → {}", v.note);

    std::process::exit(if v.ok { 0 } else { 2 });
}
