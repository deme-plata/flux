//! sigil-releases — tiny CLI for appending + listing 100-iteration SIGIL wallet
//! releases. Designed to be invoked by other Flux tools (or by hand) after a
//! release ships. Writes JSONL that the static page polls.
//!
//! Usage:
//!     sigil-releases ship A 0.1.0 "Fork import" --qug 0.5 --notes "..."  --url "..."
//!     sigil-releases list [--phase A]
//!     sigil-releases backfill                    # writes today's known 7 Phase A entries
//!     sigil-releases dump                        # print full grid as JSON
//!
//! The ledger file path defaults to:
//!     $SIGIL_RELEASES_PATH
//!     or /home/orobit/q-narwhalknight/dist-final/sigil-releases.jsonl

use anyhow::{anyhow, Context, Result};
use flux_sigil_releases::{
    append_jsonl, build_grid, grid_stats, now_ms, read_jsonl, Release, Status, PHASES,
};
use std::path::PathBuf;

fn ledger_path() -> PathBuf {
    if let Ok(p) = std::env::var("SIGIL_RELEASES_PATH") {
        return PathBuf::from(p);
    }
    PathBuf::from("/home/orobit/q-narwhalknight/dist-final/sigil-releases.jsonl")
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let sub = args.get(1).map(|s| s.as_str()).unwrap_or("");
    match sub {
        "ship" => cmd_ship(&args[2..]),
        "in-flight" | "inflight" => cmd_in_flight(&args[2..]),
        "list" => cmd_list(&args[2..]),
        "backfill" => cmd_backfill(),
        "dump" => cmd_dump(),
        "" | "-h" | "--help" => {
            print_help();
            Ok(())
        }
        other => Err(anyhow!("unknown subcommand: {}", other)),
    }
}

fn print_help() {
    println!(
        "sigil-releases v{} — SIGIL wallet 100-iteration ledger\n\n\
         USAGE:\n\
         \x20 sigil-releases ship PHASE VERSION TITLE [--qug N] [--notes ...] [--url ...] [--agent NAME]\n\
         \x20 sigil-releases in-flight PHASE VERSION TITLE [--notes ...]\n\
         \x20 sigil-releases list [--phase A]\n\
         \x20 sigil-releases backfill        # writes today's 7 Phase A entries\n\
         \x20 sigil-releases dump            # full 100-grid as JSON\n\n\
         LEDGER: $SIGIL_RELEASES_PATH or default\n",
        env!("CARGO_PKG_VERSION")
    );
}

fn parse_flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let mut iter = args.iter().peekable();
    while let Some(a) = iter.next() {
        if a == name {
            return iter.peek().map(|s| s.as_str());
        }
        if let Some(rest) = a.strip_prefix(&format!("{}=", name)) {
            return Some(rest);
        }
    }
    None
}

fn parse_phase(s: &str) -> Result<char> {
    let c = s.chars().next().ok_or_else(|| anyhow!("empty phase"))?;
    let c = c.to_ascii_uppercase();
    if !PHASES.iter().any(|(p, _)| *p == c) {
        return Err(anyhow!("phase must be A..J, got {:?}", s));
    }
    Ok(c)
}

fn cmd_ship(args: &[String]) -> Result<()> {
    let phase = parse_phase(args.first().context("ship needs PHASE")?)?;
    let version = args.get(1).context("ship needs VERSION")?.to_string();
    let title = args.get(2).context("ship needs TITLE")?.to_string();
    let qug: Option<f64> = parse_flag(args, "--qug").and_then(|s| s.parse().ok());
    let notes = parse_flag(args, "--notes").map(|s| s.to_string());
    let url = parse_flag(args, "--url").map(|s| s.to_string());
    let agent = parse_flag(args, "--agent").map(|s| s.to_string()).or(Some("rocky-sigil".into()));
    let r = Release {
        phase,
        version: version.clone(),
        title,
        status: Status::Shipped,
        settled_qug: qug.or(Some(0.5)),
        ts_ms: now_ms(),
        notes,
        url,
        agent,
    };
    if !r.is_canonical() {
        return Err(anyhow!(
            "non-canonical: phase {} expects 0.{}.X for X in 0..=9, got {}",
            phase, version, version
        ));
    }
    let path = ledger_path();
    append_jsonl(&path, &r)?;
    println!("✓ shipped {} {} — {}  (qug={:?})", r.phase, r.version, r.title, r.settled_qug);
    Ok(())
}

fn cmd_in_flight(args: &[String]) -> Result<()> {
    let phase = parse_phase(args.first().context("in-flight needs PHASE")?)?;
    let version = args.get(1).context("in-flight needs VERSION")?.to_string();
    let title = args.get(2).context("in-flight needs TITLE")?.to_string();
    let notes = parse_flag(args, "--notes").map(|s| s.to_string());
    let agent = parse_flag(args, "--agent").map(|s| s.to_string()).or(Some("rocky-sigil".into()));
    let r = Release {
        phase,
        version,
        title,
        status: Status::InFlight,
        settled_qug: None,
        ts_ms: now_ms(),
        notes,
        url: None,
        agent,
    };
    if !r.is_canonical() {
        return Err(anyhow!("non-canonical slot for phase {} {}", phase, r.version));
    }
    let path = ledger_path();
    append_jsonl(&path, &r)?;
    println!("↻ in-flight {} {} — {}", r.phase, r.version, r.title);
    Ok(())
}

fn cmd_list(args: &[String]) -> Result<()> {
    let phase_filter = parse_flag(args, "--phase").and_then(|s| s.chars().next()).map(|c| c.to_ascii_uppercase());
    let hist = read_jsonl(&ledger_path())?;
    for r in &hist {
        if let Some(p) = phase_filter {
            if r.phase != p {
                continue;
            }
        }
        let icon = match r.status {
            Status::Shipped => "✓",
            Status::InFlight => "↻",
            Status::Pending => "○",
            Status::Aborted => "✗",
        };
        println!(
            "  {} {} {}  {}  ({:?})",
            icon,
            r.phase,
            r.version,
            r.title,
            r.agent.as_deref().unwrap_or("?")
        );
    }
    Ok(())
}

fn cmd_dump() -> Result<()> {
    let hist = read_jsonl(&ledger_path())?;
    let grid = build_grid(&hist);
    let stats = grid_stats(&grid);
    let payload = serde_json::json!({
        "stats": stats,
        "grid": grid,
        "phases": PHASES.iter().map(|(p, n)| serde_json::json!({"phase": p, "name": n})).collect::<Vec<_>>(),
        "generated_ms": now_ms(),
    });
    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}

fn cmd_backfill() -> Result<()> {
    // Today's 7 known Phase A releases — written only if not already present.
    let entries = [
        ("A", "0.1.0", "Fork import — quantum-wallet → sigil-wallet (6.4 MB, 117 components)"),
        ("A", "0.1.1", "Tree taxonomy — auto-generated INDEX.md"),
        ("A", "0.1.3", "API URL swap — 145 quillon.xyz → sigilgraph.com"),
        ("A", "0.1.4", "Brand string sweep — 124 Quillon → SIGIL"),
        ("A", "0.1.5", "Coin symbol QUG → SGL — 730 hits"),
        ("A", "0.1.6", "Logo + favicon — placeholder violet sigil SVG"),
        ("A", "0.1.7", "First deploy + TLS — sigilgraph.quillon.xyz SAN expanded"),
    ];
    let path = ledger_path();
    let existing = read_jsonl(&path)?;
    let mut count = 0;
    for (phase, version, title) in &entries {
        let id = format!("{}-{}", phase, version);
        if existing.iter().any(|r| r.slot_id() == id && r.status == Status::Shipped) {
            continue;
        }
        let r = Release {
            phase: phase.chars().next().unwrap(),
            version: version.to_string(),
            title: title.to_string(),
            status: Status::Shipped,
            settled_qug: Some(0.5),
            ts_ms: now_ms() - (entries.len() - count) as u64 * 1000,
            notes: Some("Phase A bootstrap — shipped 2026-05-30".into()),
            url: Some("https://sigilgraph.quillon.xyz/sigil-wallet/index.html".into()),
            agent: Some("rocky-sigil".into()),
        };
        append_jsonl(&path, &r)?;
        count += 1;
    }
    println!("backfilled {} releases (skipped {} already present)", count, entries.len() - count);
    Ok(())
}
