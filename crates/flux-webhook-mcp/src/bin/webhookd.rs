//! flux-webhookd — the Webhook-MCP Combo v2 daemon.
//!
//! Run:
//! ```bash
//! flux-webhookd                          # default :4199
//! flux-webhookd --port 4199 --watch ./flux
//! ```
//!
//! Then POST webhooks to:
//! ```bash
//! curl -X POST http://localhost:4199/webhook \
//!   -H "Content-Type: application/json" \
//!   -d '{"event":"file_stored","file_cid":"abc123"}'
//!
//! # Direct MCP tool call via webhook:
//! curl -X POST http://localhost:4199/mcp/flux_iterate \
//!   -H "Content-Type: application/json" \
//!   -d '{"package":"flux-aether"}'
//! ```

use std::sync::Arc;
use clap::Parser;
use anyhow::Result;
use tracing::info;

use flux_webhook_mcp::*;

#[derive(Parser)]
#[command(name = "flux-webhookd", version, about = "Flux Webhook-MCP Combo v2 daemon")]
struct Cli {
    /// Port for the inbound webhook server
    #[arg(long, default_value = "4199")]
    port: u16,

    /// Directories to watch for file changes
    #[arg(long, default_values = &["/var/lib/flux-aether"])]
    watch: Vec<String>,

    /// Fluxc binary path
    #[arg(long, default_value = "fluxc")]
    fluxc: String,

    /// Disable auto-fluxfood
    #[arg(long)]
    no_fluxfood: bool,

    /// Disable auto-search-reindex
    #[arg(long)]
    no_search: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    let cli = Cli::parse();

    let config = WebhookMcpConfig {
        server_port: cli.port,
        fluxc_bin: cli.fluxc,
        watch_dirs: cli.watch,
        auto_fluxfood: !cli.no_fluxfood,
        auto_search_reindex: !cli.no_search,
        ..Default::default()
    };

    let orchestrator = WebhookMcpOrchestrator::new(config);
    orchestrator.start().await?;

    info!("⚡ Webhook-MCP Combo v2 running");
    info!("   Inbound:  http://0.0.0.0:{}", cli.port);
    info!("   Routes:");
    info!("     POST /webhook              — generic webhook receiver");
    info!("     POST /webhook/:event_type  — typed webhook receiver");
    info!("     POST /mcp/:tool_name       — direct MCP tool call");
    info!("     GET  /health               — health check");

    // Keep alive
    tokio::signal::ctrl_c().await?;
    info!("⏹ Shutting down");
    Ok(())
}
