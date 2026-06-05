//! sigil-dashboard — emit the snapshot JSON consumed by the wallet apiShim.
//!
//!     sigil-dashboard gen [PATH]   # default path is the q-flux dist-final
//!     sigil-dashboard print        # pretty-print to stdout
//!
//! Run this any time the seeded address list / block-time / version line
//! changes, then redeploy the wallet bundle.

use anyhow::Result;
use flux_sigil_dashboard::make_snapshot;
use std::path::PathBuf;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let sub = args.get(1).map(|s| s.as_str()).unwrap_or("");
    match sub {
        "gen" => cmd_gen(args.get(2).map(|s| s.as_str())),
        "print" => cmd_print(),
        "" | "-h" | "--help" => { print_help(); Ok(()) }
        other => Err(anyhow::anyhow!("unknown subcommand: {}", other)),
    }
}

fn print_help() {
    println!(
        "sigil-dashboard v{} — SIGIL wallet apiShim data generator\n\n\
        USAGE:\n\
        \x20 sigil-dashboard gen [PATH]    # default: /home/orobit/q-narwhalknight/dist-final/sigil-dashboard.json\n\
        \x20 sigil-dashboard print          # pretty-print snapshot to stdout\n",
        env!("CARGO_PKG_VERSION")
    );
}

fn cmd_gen(path_arg: Option<&str>) -> Result<()> {
    let path = path_arg
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/orobit/q-narwhalknight/dist-final/sigil-dashboard.json"));
    let snap = make_snapshot();
    let json = serde_json::to_string_pretty(&snap)?;
    if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
    std::fs::write(&path, &json)?;
    println!(
        "✓ wrote snapshot ({} bytes)\n  tip:    {} (network {})\n  blocks: {}\n  addrs:  {}\n  path:   {}",
        json.len(),
        snap.status.height,
        snap.status.network_id,
        snap.recent_blocks.len(),
        snap.address_balances.len(),
        path.display(),
    );
    Ok(())
}

fn cmd_print() -> Result<()> {
    let snap = make_snapshot();
    println!("{}", serde_json::to_string_pretty(&snap)?);
    Ok(())
}
