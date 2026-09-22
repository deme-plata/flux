//! `chronos_life` — run the Quillon Graph Skyskraber for a lifetime and keep
//! everything.
//!
//!   chronos_life --scenario canonical --years 256 --out /home/storage/skyskraber-chronos/canonical
//!   chronos_life --scenario endowed --endowment-uqug 10000000000 --years 256 --out …/endowed
//!   chronos_life --scenario custom --income-uqug 120000 --years 32 --no-ticks --out …/custom
//!
//! Flags: --years N (256) · --seed S (42) · --hardened · --endowment-uqug N ·
//! --income-uqug N (per day) · --no-ticks · --allow-any-fs · --min-free-gb N
//!
//! Refuses to start unless the output sits on the big array (/home/storage or
//! /home/orobit) with room for the whole run — the root partition is 40 GB and
//! a 256-year tick stream is ~485 GB. That guard cost this box an outage once.

use flux_skyskraber::life::{self, LifeConfig};
use std::path::PathBuf;
use std::process::Command;

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn free_bytes(dir: &std::path::Path) -> Option<u64> {
    let out = Command::new("df").args(["-B1", "--output=avail"]).arg(dir).output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().nth(1)?.trim().parse().ok()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") || args.len() < 2 {
        eprintln!("usage: chronos_life --scenario NAME --out DIR [--years 256] [--seed 42] [--hardened] [--endowment-uqug N] [--income-uqug N] [--no-ticks] [--allow-any-fs] [--min-free-gb N]");
        std::process::exit(2);
    }
    let scenario = arg(&args, "--scenario").unwrap_or_else(|| "canonical".into());
    let years: u32 = arg(&args, "--years").and_then(|s| s.parse().ok()).unwrap_or(256);
    let seed: u64 = arg(&args, "--seed").and_then(|s| s.parse().ok()).unwrap_or(42);
    let endowment: u128 = arg(&args, "--endowment-uqug").and_then(|s| s.parse().ok()).unwrap_or(0);
    let income: u128 = arg(&args, "--income-uqug").and_then(|s| s.parse().ok()).unwrap_or(0);
    let ticks = !args.iter().any(|a| a == "--no-ticks");
    let hardened = args.iter().any(|a| a == "--hardened");
    let out = PathBuf::from(arg(&args, "--out").unwrap_or_else(|| format!("/home/storage/skyskraber-chronos/{scenario}")));

    // ── the disk guard ────────────────────────────────────────────────────
    std::fs::create_dir_all(&out).expect("create output dir");
    let canon = out.canonicalize().expect("canonical output path");
    let big_array = canon.starts_with("/home/storage") || canon.starts_with("/home/orobit");
    if !big_array && !args.iter().any(|a| a == "--allow-any-fs") {
        eprintln!("🛑 {} is not on /home/storage or /home/orobit — the root partition is 40 GB. Pass --allow-any-fs if you really mean it.", canon.display());
        std::process::exit(78);
    }
    let days = life::days_in(years);
    let est = if ticks { days * 86_400 * 60 } else { 0 } + days * 800 + days * 24 * 260;
    let min_free_gb: u64 = arg(&args, "--min-free-gb").and_then(|s| s.parse().ok()).unwrap_or(100);
    if let Some(free) = free_bytes(&canon) {
        let need = est + est / 5 + min_free_gb * (1 << 30);
        if free < need {
            eprintln!("🛑 need ≈{:.1} GB (+{} GB margin) but only {:.1} GB free at {}", est as f64 / 1e9, min_free_gb, free as f64 / 1e9, canon.display());
            std::process::exit(78);
        }
    }

    let cfg = LifeConfig { scenario: scenario.clone(), years, seed, hardened, endowment_uqug: endowment, daily_income_uqug: income, out: canon.clone(), ticks };
    println!("═══ chronos_life · {} · {} years = {} days · seed {} · ticks {} · est {:.1} GB → {}",
        scenario, years, days, seed, if ticks { "on" } else { "off" }, est as f64 / 1e9, canon.display());
    println!("    endowment {} uQUG · daily income {} uQUG · hardened {}", endowment, income, hardened);
    let t0 = std::time::Instant::now();
    let m = life::run_life(&cfg, |y| {
        println!(
            "  year {:>3} │ day {:>6} │ {:>6.1} d/s │ rss {:>6.0} MB │ audit {:>7} · anchors {:>6} │ chain {} · witnessed {} │ treasury {:>14} uQUG │ vault {:>4} bars │ head {}…",
            y.year + 1, y.days_done, y.days_per_s, y.rss_mb, y.audit_entries, y.anchors,
            if y.full_chain_ok { "ok" } else { "BROKEN" },
            match y.witnessed_ok { Some(true) => "ok", Some(false) => "BROKEN", None => "—" },
            y.treasury_uqug, y.vault_bars, &y.head[..12]
        );
    })
    .expect("life run");
    println!("═══ done in {:.0} s · {} days · head {}", t0.elapsed().as_secs_f64(), m.days, m.head);
    println!("    first insolvent day    : {:?}", m.first_insolvent_day);
    println!("    first vault refusal day: {:?}", m.first_vault_refusal_day);
    println!("    first RefillTreasury   : {:?}   first AddRobotCar: {:?}", m.first_refill_decision_day, m.first_add_car_decision_day);
    println!("    payroll failures total : {}   vault refusals total: {}   chain breaks: {}", m.total_payroll_failures, m.total_vault_refusals, m.chain_breaks);
    let total: u64 = m.files.iter().map(|f| f.bytes).sum();
    println!("    files: {} · {:.2} GB · manifest {}", m.files.len(), total as f64 / 1e9, canon.join("manifest.json").display());
}
