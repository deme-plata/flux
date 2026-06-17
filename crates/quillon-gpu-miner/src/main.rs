//! quillon-gpu-miner CLI — standalone Quillon BLAKE3 miner.
//!
//! Usage:
//!   quillon-gpu-miner --challenge <64hex> --target <64hex> [--start N] [--batches B] [--batch-size S]
//!   quillon-gpu-miner --selftest
//!
//! Build CPU-only (default):   fluxc build --package quillon-gpu-miner
//! Build with GPU backend:     fluxc build --package quillon-gpu-miner --features gpu
//!
//! The `gpu` feature is the headline flag: it links flux-gpu and routes batches
//! to the GPU when a device is present, transparently falling back to CPU.

use quillon_gpu_miner::{
    benchmark_hashrate, cluster, eta_seconds, expected_hashes, format_hashrate, mine_assigned,
    mine_batch_auto, target_difficulty_bits, Backend, Work,
};
use std::time::Instant;

mod http;

fn parse_hex32(s: &str) -> Result<[u8; 32], String> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    if s.len() != 64 {
        return Err(format!("expected 64 hex chars, got {}", s.len()));
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16)
            .map_err(|e| format!("bad hex at byte {i}: {e}"))?;
    }
    Ok(out)
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

struct Args {
    challenge: Option<[u8; 32]>,
    target: [u8; 32],
    start: u64,
    batches: u64,
    batch_size: u64,
    selftest: bool,
    // When Some(n), benchmark raw hashrate over n hashes and exit.
    bench: Option<u64>,
    // Live mode: fetch challenge from this node base-URL and mine it.
    server: Option<String>,
    wallet: Option<String>,
    submit: bool,
    keep_looping: bool,
    interval: u64,
    // Supercluster role: this node's index into a comma-separated weight list.
    // When set, the miner searches ONLY its assigned slice of [0, total).
    node_index: Option<usize>,
    weights: Vec<u32>,
    total: u64,
}

fn parse_args() -> Result<Args, String> {
    let mut challenge = None;
    // Default target accepts a digest with a leading zero byte — quick demo difficulty.
    let mut target = [0xffu8; 32];
    target[0] = 0x00;
    let mut start = 0u64;
    let mut batches = 64u64;
    let mut batch_size = 100_000u64;
    let mut selftest = false;
    let mut bench = None;
    let mut server = None;
    let mut wallet = None;
    let mut submit = false;
    let mut keep_looping = false;
    let mut interval = 2u64;
    let mut node_index = None;
    let mut weights = Vec::new();
    let mut total = 0u64;

    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--challenge" => {
                challenge = Some(parse_hex32(&it.next().ok_or("--challenge needs a value")?)?)
            }
            "--target" => target = parse_hex32(&it.next().ok_or("--target needs a value")?)?,
            "--start" => start = it.next().ok_or("--start needs a value")?.parse().map_err(|e| format!("{e}"))?,
            "--batches" => batches = it.next().ok_or("--batches needs a value")?.parse().map_err(|e| format!("{e}"))?,
            "--batch-size" => batch_size = it.next().ok_or("--batch-size needs a value")?.parse().map_err(|e| format!("{e}"))?,
            "--node-index" => node_index = Some(it.next().ok_or("--node-index needs a value")?.parse().map_err(|e| format!("{e}"))?),
            "--weights" => {
                weights = it.next().ok_or("--weights needs a value")?
                    .split(',')
                    .map(|s| s.trim().parse::<u32>())
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| format!("bad --weights: {e}"))?;
            }
            "--total" => total = it.next().ok_or("--total needs a value")?.parse().map_err(|e| format!("{e}"))?,
            "--server" => server = Some(it.next().ok_or("--server needs a URL")?),
            "--wallet" => wallet = Some(it.next().ok_or("--wallet needs a qnk address")?),
            "--submit" => submit = true,
            "--loop" => keep_looping = true,
            "--interval" => interval = it.next().ok_or("--interval needs seconds")?.parse().map_err(|e| format!("{e}"))?,
            "--selftest" => selftest = true,
            "--bench" => {
                // Optional count; default 5M. Don't consume a following flag.
                let n = match it.next() {
                    Some(v) if !v.starts_with("--") => v.parse().map_err(|e| format!("bad --bench: {e}"))?,
                    Some(v) => return Err(format!("--bench expects a number, got {v}")),
                    None => 5_000_000,
                };
                bench = Some(n);
            }
            "-h" | "--help" => {
                println!("quillon-gpu-miner — standalone Quillon BLAKE3 miner");
                println!("  --challenge <64hex>  --target <64hex>  --start N  --batches B  --batch-size S");
                println!("  --selftest           run a built-in easy-target mine and verify");
                println!("  --bench [N]          measure raw hashrate over N hashes (default 5M) and exit");
                println!("  live mode:           --server <url> --wallet <qnk...> [--submit]");
                println!("                       fetch a challenge from the node, mine it, optionally POST");
                println!("  continuous mining:   add --loop [--interval S]  (re-fetch + mine forever)");
                println!("  supercluster mode:   --node-index I --weights w0,w1,.. --total N");
                println!("                       (this node mines only its proportional slice of [0,total))");
                println!("  build --features gpu to enable the GPU backend");
                std::process::exit(0);
            }
            other => return Err(format!("unknown arg: {other}")),
        }
    }
    Ok(Args { challenge, target, start, batches, batch_size, selftest, bench, server, wallet, submit, keep_looping, interval, node_index, weights, total })
}

/// One live round: fetch a fresh challenge, mine it (bounded by --batches),
/// and (if --submit) POST the solution. Returns Err on a fetch failure so the
/// caller can back off; a "no solution this round" is Ok (just keep going).
fn mine_round(srv: &str, wallet: &str, args: &Args) -> Result<(u64, bool), String> {
    let pc = http::fetch_challenge(srv, wallet)?;
    let bits = target_difficulty_bits(&pc.work.target);
    println!(
        "  challenge @ height {}: {} bits · reward {:.4} QUG · vdf_iters {}",
        pc.block_height, bits, pc.block_reward, pc.vdf_iterations
    );
    if !pc.server_notice.is_empty() {
        println!("  📢 node notice: {}", pc.server_notice);
    }
    if let Some(min) = pc.min_miner_version.as_ref() {
        let mine = env!("CARGO_PKG_VERSION");
        if !quillon_gpu_miner::version_at_least(mine, min) {
            println!("  ⚠ miner v{mine} < node-required v{min} — submissions may be rejected");
        }
    }

    let started = Instant::now();
    let mut nonce = 0u64;
    let mut found = None;
    for _ in 0..args.batches {
        let (sol, backend) = mine_batch_auto(&pc.work, nonce, args.batch_size);
        if let Some(s) = sol {
            found = Some((s, backend));
            break;
        }
        nonce = nonce.saturating_add(args.batch_size);
    }
    let secs = started.elapsed().as_secs_f64();
    match found {
        Some((s, backend)) => {
            let hps = if secs > 0.0 { (nonce + args.batch_size) as f64 / secs } else { 0.0 };
            println!(
                "✅ solution via {} nonce={} ({})",
                backend_label(backend), s.nonce, format_hashrate(hps)
            );
            if args.submit {
                match quillon_gpu_miner::submit_payload_json(wallet, &pc.work, s.nonce, &s.digest, hps / 1000.0) {
                    Some(payload) => match http::submit_solution(srv, &payload) {
                        Ok(resp) => println!("📨 node response: {}", &resp[..resp.len().min(300)]),
                        Err(e) => eprintln!("⚠ submit failed: {e}"),
                    },
                    None => eprintln!("⚠ bad wallet address, cannot build submission"),
                }
            } else {
                println!("  (re-run with --submit to POST it; note: PoW-only, mainnet needs VDF)");
            }
        }
        None => println!("⛏  no solution this round in {} hashes", args.batches * args.batch_size),
    }
    let hashes_searched = (nonce + args.batch_size).min(args.batches.saturating_mul(args.batch_size));
    Ok((hashes_searched, found.is_some()))
}

fn backend_label(b: Backend) -> &'static str {
    match b {
        Backend::Cpu => "CPU",
        Backend::Gpu => "GPU",
    }
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("arg error: {e}");
            std::process::exit(2);
        }
    };

    // ── Live mode: fetch a challenge from a node and mine it ─────────────
    if let Some(srv) = args.server.as_ref() {
        let wallet = match args.wallet.as_ref() {
            Some(w) => w.clone(),
            None => {
                eprintln!("--server requires --wallet <qnk...>");
                std::process::exit(2);
            }
        };
        let feature = if cfg!(feature = "gpu") { "gpu=ON" } else { "gpu=off" };
        let mode = if args.keep_looping { "continuous" } else { "one-shot" };
        println!("quillon-gpu-miner [{feature}] live ({mode}) → {srv}");

        let session_start = Instant::now();
        let mut stats = quillon_gpu_miner::SessionStats::default();
        let mut failures = 0u32;
        loop {
            match mine_round(srv, &wallet, &args) {
                Ok((hashes, found)) => {
                    failures = 0;
                    stats.record_round(hashes, found);
                    if args.keep_looping {
                        println!("  {}", stats.summary(session_start.elapsed().as_secs_f64()));
                    }
                }
                Err(e) => {
                    failures += 1;
                    let backoff = quillon_gpu_miner::poll_backoff_secs(failures, 2, 30);
                    eprintln!("⚠ {e} (failure #{failures}, backing off {backoff}s)");
                    if args.keep_looping {
                        std::thread::sleep(std::time::Duration::from_secs(backoff));
                        continue;
                    }
                    std::process::exit(1);
                }
            }
            if !args.keep_looping {
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(args.interval));
        }
        return;
    }

    // ── Benchmark mode: measure this box's raw hashrate and exit ─────────
    if let Some(n) = args.bench {
        let feature = if cfg!(feature = "gpu") { "gpu=ON" } else { "gpu=off" };
        let chal = args.challenge.unwrap_or([0x11u8; 32]);
        println!("quillon-gpu-miner [{feature}] benchmarking {n} hashes…");
        let hps = benchmark_hashrate(&chal, n);
        println!("📊 hashrate: {} ({} hashes)", format_hashrate(hps), n);
        // At this rate, how long for a 24-bit block, for operator intuition.
        let mut t24 = [0xffu8; 32];
        t24[0] = 0x00;
        t24[1] = 0x00;
        t24[2] = 0x00;
        if hps > 0.0 {
            println!("   → ~{:.1}s per 24-bit solution at this rate", eta_seconds(&t24, hps));
        }
        return;
    }

    let (challenge, target) = if args.selftest {
        let mut t = [0xffu8; 32];
        t[0] = 0x0f; // easy
        ([0x11u8; 32], t)
    } else {
        (
            args.challenge.unwrap_or_else(|| {
                eprintln!("no --challenge given; using all-0x11 demo challenge");
                [0x11u8; 32]
            }),
            args.target,
        )
    };

    let feature = if cfg!(feature = "gpu") { "gpu=ON" } else { "gpu=off" };
    println!("quillon-gpu-miner [{feature}] target={}", to_hex(&target));
    let bits = target_difficulty_bits(&target);
    println!(
        "  difficulty: {} bits · ~{:.0} hashes/solution (mean){}",
        bits,
        expected_hashes(&target),
        // Show an ETA estimate at a nominal 1 MH/s so the operator has a feel.
        if bits > 0 {
            format!(" · ~{:.1}s @ 1 MH/s", eta_seconds(&target, 1.0e6))
        } else {
            String::new()
        }
    );

    let work = Work::new(challenge, target);

    // ── Supercluster mode: mine only this node's assigned slice ──────────
    if let Some(idx) = args.node_index {
        if args.weights.is_empty() || args.total == 0 {
            eprintln!("supercluster mode needs --weights w0,w1,.. and --total N");
            std::process::exit(2);
        }
        match cluster::assign_range(args.total, &args.weights, idx) {
            Some((start, len)) => {
                println!(
                    "  cluster node {}/{}: slice [{}, {}) ({} nonces, weight {})",
                    idx,
                    args.weights.len(),
                    start,
                    start + len,
                    len,
                    args.weights.get(idx).copied().unwrap_or(0)
                );
                let (sol, backend) = mine_assigned(&work, args.total, &args.weights, idx, args.batch_size);
                match sol {
                    Some(s) => println!(
                        "✅ SOLUTION via {} nonce={} digest={}",
                        backend_label(backend), s.nonce, to_hex(&s.digest)
                    ),
                    None => println!("⛏  no solution in this node's slice (raise --total or lower difficulty)"),
                }
            }
            None => {
                eprintln!("--node-index {idx} out of range for {} weights", args.weights.len());
                std::process::exit(2);
            }
        }
        return;
    }

    let mut nonce = args.start;
    let mut total = 0u64;
    let started = Instant::now(); // elapsed() only — never `Instant - Duration` (panics on Windows)
    for batch in 0..args.batches {
        let (sol, backend) = mine_batch_auto(&work, nonce, args.batch_size);
        total += args.batch_size;
        if let Some(s) = sol {
            let secs = started.elapsed().as_secs_f64();
            let hps = if secs > 0.0 { total as f64 / secs } else { 0.0 };
            println!(
                "✅ SOLUTION via {} after {} hashes in {:.2}s ({}): nonce={} digest={}",
                backend_label(backend),
                total,
                secs,
                format_hashrate(hps),
                s.nonce,
                to_hex(&s.digest)
            );
            return;
        }
        if batch == 0 {
            println!("  mining via {} backend…", backend_label(backend));
        }
        nonce = nonce.saturating_add(args.batch_size);
    }
    let secs = started.elapsed().as_secs_f64();
    let hps = if secs > 0.0 { total as f64 / secs } else { 0.0 };
    println!(
        "⛏  no solution in {total} hashes ({} measured, {:.2}s) — raise --batches or lower difficulty",
        format_hashrate(hps),
        secs
    );
}
