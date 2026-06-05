//! Minimal example: attach to a Vite project, print events for 30 seconds.
//!
//!   cargo run --example attach -- /path/to/my-vite-app

use flux_vite_engine::{ViteConfig, ViteEngine, ViteEvent};
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let project = std::env::args()
        .nth(1)
        .unwrap_or_else(|| ".".to_string());

    let cfg = ViteConfig::for_project(&project);
    let engine = ViteEngine::spawn(cfg).await?;
    println!("attached to {}", engine.project_path().display());

    let mut events = engine.subscribe();
    let stop = tokio::time::sleep(Duration::from_secs(30));
    tokio::pin!(stop);

    loop {
        tokio::select! {
            _ = &mut stop => break,
            ev = events.recv() => match ev {
                Ok(ViteEvent { kind, ts_ms }) => println!("{ts_ms} {kind:?}"),
                Err(_) => break,
            }
        }
    }

    let snap = engine.snapshot().await;
    println!(
        "\n--- snapshot ---\nHMR total: {} | rate: {:.2}/s | score: {}/100",
        snap.hmr_total, snap.hmr_rate_60s, snap.score.composite
    );

    engine.shutdown().await;
    Ok(())
}
