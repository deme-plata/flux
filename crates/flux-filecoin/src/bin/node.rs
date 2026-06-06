//! Flux Filecoin Node — the main binary.
//!
//! Run a storage node:
//! ```bash
//! flux-filecoin-node --capacity 1TB --price 50000
//! ```
//!
//! Run a client (store a file):
//! ```bash
//! flux-filecoin-node store myfile.pdf
//! flux-filecoin-node search "quantum consensus"
//! ```

use std::sync::Arc;
use std::path::PathBuf;
use clap::{Parser, Subcommand};

use anyhow::{Context, Result};
use tracing::{info, warn};

use flux_filecoin::*;

#[derive(Parser)]
#[command(name = "flux-filecoin-node", version, about = "Flux Filecoin — decentralized storage node")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start a storage node (offer disk space to the network)
    Start {
        /// Storage capacity in GB
        #[arg(long, default_value = "10")]
        capacity_gb: u64,

        /// Price per GB/month in SIGIL base units
        #[arg(long, default_value = "100000")]
        price: u128,

        /// Data directory
        #[arg(long, default_value = "/var/lib/flux-filecoin")]
        data_dir: String,
    },
    /// Store a file on the network
    Store {
        /// Path to the file
        path: String,

        /// MIME type (auto-detect if not specified)
        #[arg(long)]
        mime: Option<String>,

        /// Number of replicas
        #[arg(long, default_value = "3")]
        replicas: u32,
    },
    /// Retrieve a file by CID
    Get {
        /// Content ID (hex)
        cid: String,
        /// Output path
        #[arg(short, long)]
        output: Option<String>,
    },
    /// Search for files on the network
    Search {
        /// Search query
        query: String,
        /// Maximum results
        #[arg(long, default_value = "10")]
        max: u32,
    },
    /// List known storage providers
    Providers,
    /// List active contracts
    Contracts,
    /// Show market statistics
    Market,
    /// Show node status
    Status,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Start { capacity_gb, price, data_dir } => {
            run_node(capacity_gb, price, &data_dir).await?;
        }
        Commands::Store { path, mime, replicas } => {
            store_file(&path, mime.as_deref(), replicas).await?;
        }
        Commands::Get { cid, output } => {
            retrieve_file(&cid, output.as_deref()).await?;
        }
        Commands::Search { query, max } => {
            search_files(&query, max).await?;
        }
        Commands::Providers => {
            list_providers().await?;
        }
        Commands::Contracts => {
            list_contracts().await?;
        }
        Commands::Market => {
            show_market().await?;
        }
        Commands::Status => {
            show_status().await?;
        }
    }

    Ok(())
}

async fn run_node(capacity_gb: u64, price: u128, data_dir: &str) -> Result<()> {
    let config = FilecoinConfig {
        storage_capacity: capacity_gb * 1_073_741_824,
        price_per_gb_month: price,
        data_dir: data_dir.to_string(),
        ..Default::default()
    };

    let node = FilecoinNode::new(config).await?;
    info!("🚀 Flux Filecoin node started!");
    info!("   Capacity: {} GB", capacity_gb);
    info!("   Price: {} per GB/month", price);
    info!("   Data: {}", data_dir);

    // Announce to network
    node.announce().await?;
    info!("📡 Announced to storage network");

    // Start proof cycle
    node.start_proof_cycle().await?;
    info!("🔐 Proof cycle started");

    // Keep running
    tokio::signal::ctrl_c().await?;
    info!("⏹ Shutting down...");
    Ok(())
}

async fn store_file(path: &str, mime: Option<&str>, _replicas: u32) -> Result<()> {
    let data = tokio::fs::read(path).await
        .context("Failed to read file")?;

    let mime = mime.unwrap_or("application/octet-stream").to_string();
    let name = PathBuf::from(path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let config = FilecoinConfig {
        data_dir: "/var/lib/flux-filecoin".into(),
        ..Default::default()
    };
    let node = FilecoinNode::new(config).await?;
    let stored = node.store(&name, &data, &mime).await?;

    println!("✅ Stored file:");
    println!("   Name: {}", stored.name);
    println!("   Size: {} bytes", stored.size);
    println!("   CID:  {}", stored.cid_hex());
    println!("   Type: {}", stored.mime_type);

    // Index for search
    node.index_for_search(&stored, &data).await?;
    println!("📑 Indexed for search");

    Ok(())
}

async fn retrieve_file(cid_hex: &str, output: Option<&str>) -> Result<()> {
    let cid = hex::decode(cid_hex)
        .context("Invalid CID hex")?;
    let mut cid_bytes = [0u8; 32];
    cid_bytes.copy_from_slice(&cid);

    let config = FilecoinConfig::default();
    let node = FilecoinNode::new(config).await?;
    let data = node.retrieve(&cid_bytes).await?;

    let output_path = output.unwrap_or("retrieved_file");
    tokio::fs::write(output_path, &data).await?;
    println!("✅ Retrieved {} bytes to {}", data.len(), output_path);
    Ok(())
}

async fn search_files(query: &str, max: u32) -> Result<()> {
    let config = FilecoinConfig::default();
    let node = FilecoinNode::new(config).await?;

    let search_query = StorageSearchQuery {
        query: query.to_string(),
        max_results: max,
        mime_filter: None,
        min_size: None,
        max_size: None,
    };

    let results = node.search(&search_query).await;
    println!("🔍 Search results for: '{}'", query);
    println!("{}", "-".repeat(60));
    for (i, r) in results.iter().enumerate() {
        println!("{}. {} (score: {:.2})", i + 1, r.name, r.score);
        println!("   CID: {}", hex::encode(&r.cid));
        println!("   Size: {} bytes | Type: {}", r.size, r.mime_type);
        println!("   {}", r.snippet);
        println!();
    }
    if results.is_empty() {
        println!("No results found.");
    }

    Ok(())
}

async fn list_providers() -> Result<()> {
    println!("📡 Storage Providers");
    println!("{}", "=".repeat(60));
    // Phase 0: read from local cache
    println!("(Run a node to discover providers)");
    Ok(())
}

async fn list_contracts() -> Result<()> {
    println!("📋 Storage Contracts");
    println!("{}", "=".repeat(60));
    println!("(Run a node to see active contracts)");
    Ok(())
}

async fn show_market() -> Result<()> {
    println!("📊 Storage Market");
    println!("{}", "=".repeat(60));
    println!("(Run a node to see live market data)");
    Ok(())
}

async fn show_status() -> Result<()> {
    println!("⚡ Flux Filecoin Status");
    println!("{}", "=".repeat(60));
    println!("Protocol: flux-filecoin v{}", env!("CARGO_PKG_VERSION"));
    println!("Substrate: flux-aether + flux-search + flux-p2p");
    println!("Crypto: SQIsign (PQ) + BLAKE3");
    println!("Network: libp2p gossipsub");
    Ok(())
}
