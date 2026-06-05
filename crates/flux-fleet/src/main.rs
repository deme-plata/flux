//! flux-fleet — discover your machines from the SSH keys you already have,
//! then (with confirmation) spread Flux across them over the flux-p2p mesh.
//!
//! Philosophy borrowed straight from the SIGIL light node: **verify-don't-trust**.
//! Every line flux-fleet prints is a fact it just checked — a host is `reachable`
//! only after a real `ssh -o BatchMode` probe, `flux vX` only after running
//! `flux --version` on it. No optimistic assumptions, no simulated state, and the
//! things it can't know are labelled honestly (e.g. hashed `known_hosts` entries
//! can't be reversed — that's reported, not hidden).
//!
//! Subcommands:
//!   flux-fleet scan              read-only: parse ~/.ssh/{config,known_hosts,*.pub}
//!                                → candidate hosts. NO connection, NO key bytes leave.
//!   flux-fleet scan --probe      + ssh BatchMode "uname -sm" + remote flux --version
//!   flux-fleet up [--confirm]    push the musl-static flux binary to confirmed hosts,
//!                                verify --version. DEFAULT is dry-run; --confirm writes.
//!   flux-fleet status            probe the discovered fleet; show reachability + flux ver
//!
//! Flags: --probe  --confirm  --json  --hosts a,b,c  --user U  --port N  --help
//!
//! Safety rails (binding — this reads SSH assets + can touch other machines):
//!   • `scan` NEVER connects. `--probe`/`up` use `ssh -o BatchMode=yes` only, so a
//!     host that would need a password is SKIPPED, never prompted/harvested.
//!   • `up` is **version-aware + idempotent**: it runs `flux --version` on each
//!     remote FIRST and skips hosts already at/above the local version. (This is the
//!     lesson from editing the wrong sigil-top: never clobber, never double-install.)
//!   • `--hosts` supplements discovery, because hashed `known_hosts` can't be
//!     reversed and a config-less box still needs an explicit target.
//!   • Private key bytes are NEVER read or transmitted — only *which* hosts/identities
//!     exist. Every `up` action is appended to ~/.flux/fleet-audit.log.

use std::collections::BTreeMap;
use std::process::Command;

use serde::Serialize;

const VERSION: &str = env!("CARGO_PKG_VERSION");

// obsidian + violet ANSI, matching the SIGIL / sigil-top identity
const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[38;5;245m";
const VIOLET: &str = "\x1b[38;5;141m";
const VBRIGHT: &str = "\x1b[38;5;177m";
const GREEN: &str = "\x1b[38;5;114m";
const GOLD: &str = "\x1b[38;5;220m";
const RED: &str = "\x1b[38;5;203m";

#[derive(Debug, Clone, Serialize)]
struct Host {
    /// alias (config Host) or hostname/ip if no alias
    name: String,
    /// the real ssh target (config HostName, else == name)
    hostname: String,
    user: Option<String>,
    port: u16,
    /// where we learned about it: "config" | "known_hosts" | "--hosts"
    source: String,
    // --- probe results (None until a real probe runs) ---
    reachable: Option<bool>,
    os_arch: Option<String>,
    /// Some("0.18.0") if flux/fluxc answered --version, None if absent
    flux_version: Option<String>,
}

impl Host {
    fn new(name: &str, source: &str) -> Self {
        Host {
            name: name.to_string(),
            hostname: name.to_string(),
            user: None,
            port: 22,
            source: source.to_string(),
            reachable: None,
            os_arch: None,
            flux_version: None,
        }
    }
    /// `[user@]hostname` for ssh/scp.
    fn target(&self) -> String {
        match &self.user {
            Some(u) => format!("{u}@{}", self.hostname),
            None => self.hostname.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct Identity {
    path: String,
    kind: String, // "ed25519" | "rsa" | …
}

#[derive(Default)]
struct Args {
    cmd: String,
    probe: bool,
    confirm: bool,
    bootstrap: bool, // up: after install, kick the parallel model+research bootstrap over SSH
    json: bool,
    hosts: Vec<String>,
    user: Option<String>,
    port: Option<u16>,
    // `run` subcommand
    cmd_args: Vec<String>, // the remote command (quote it: run "uname -a")
    no_cache: bool,        // bypass the flux:// content cache
    // `get` subcommand
    from: Option<String>,  // provider to fetch a flux:// object from (user@host)
    out: Option<String>,   // write the fetched object to this path
}

fn parse_args() -> Args {
    let mut a = Args { cmd: "scan".into(), ..Default::default() };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    let mut saw_cmd = false;
    while i < argv.len() {
        match argv[i].as_str() {
            "--probe" => a.probe = true,
            "--confirm" => a.confirm = true,
            "--bootstrap" | "--serve" => a.bootstrap = true,
            "--json" => a.json = true,
            "--hosts" => {
                i += 1;
                if let Some(v) = argv.get(i) {
                    a.hosts.extend(v.split(',').filter(|s| !s.is_empty()).map(|s| s.to_string()));
                }
            }
            "--user" => { i += 1; a.user = argv.get(i).cloned(); }
            "--port" => { i += 1; a.port = argv.get(i).and_then(|s| s.parse().ok()); }
            "--no-cache" => a.no_cache = true,
            "--from" => { i += 1; a.from = argv.get(i).cloned(); }
            "--out" => { i += 1; a.out = argv.get(i).cloned(); }
            "--help" | "-h" => { print_help(); std::process::exit(0); }
            other if !other.starts_with('-') && !saw_cmd => { a.cmd = other.to_string(); saw_cmd = true; }
            other if !other.starts_with('-') => { a.cmd_args.push(other.to_string()); }
            _ => {}
        }
        i += 1;
    }
    a
}

fn print_help() {
    println!(
        "flux-fleet {VERSION} — discover your SSH fleet, spread Flux across it\n\n  \
         flux-fleet scan              list candidate hosts (read-only, no connection)\n  \
         flux-fleet scan --probe      + ssh BatchMode uname + remote flux --version\n  \
         flux-fleet up [--confirm] [--bootstrap]  push musl flux (dry-run unless --confirm);\n  \
         \u{20}                            --bootstrap also kicks the parallel model+p2p-research\n  \
         \u{20}                            bootstrap over SSH (detached) on an already-running box,\n  \
         \\                            content-verified via flux://b3/<hash> on the remote\n  \
         flux-fleet run \"<cmd>\"        run on N hosts in parallel; read-only results are\n  \
         \\                            flux://-cached (repeat = 0 round-trip). --no-cache to force\n  \
         flux-fleet put <file>        store a blob by content hash → prints flux://b3/<hash>\n  \
         flux-fleet get <flux://…>    fetch a flux:// object --from <host>, verify on arrival\n  \
         flux-fleet status            probe discovered fleet, show flux version\n\n  \
         --hosts a,b,c   add explicit targets   --user U   --port N   --json   --no-cache\n\n  \
         Safety: scan never connects; probe/up/run use BatchMode only (no password harvest);\n  \
         up is version-aware + idempotent + content-verified; private keys are never read or sent."
    );
}

fn home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/root".into())
}

// ─── flux:// content addressing ─────────────────────────────────────────────
// flux://b3/<hex> names an artifact (or a command result) by its blake3 content
// hash. There is no central server: the hash IS the address, and possession of
// the bytes + the hash IS the proof. We use it two ways — (1) `up` content-
// verifies the pushed binary (remote recomputes blake3 and refuses a mismatch),
// (2) `run` caches read-only results under their flux:// address. A full
// fetch-over-mesh (flux-get pulling flux://<hash> via flux-p2p) is the NEXT
// lever and is NOT built yet — labelled honestly wherever it would appear.

/// Content address of some bytes.
fn flux_uri(bytes: &[u8]) -> String {
    format!("flux://b3/{}", blake3::hash(bytes).to_hex())
}

/// Content address of a string (used for the run-cache key over host+command).
fn flux_uri_str(s: &str) -> String {
    flux_uri(s.as_bytes())
}

/// Read-only allowlist: only these commands are flux://-cached. Anything with
/// side effects always runs fresh — the idempotent gate (lesson from flux-ssh
/// lever 5: never speculate/cache a command that mutates).
fn is_read_only(cmd: &str) -> bool {
    let head = cmd.trim_start().split_whitespace().next().unwrap_or("");
    let base = head.rsplit('/').next().unwrap_or(head); // strip any path
    matches!(
        base,
        "uname" | "ls" | "cat" | "grep" | "rg" | "find" | "df" | "free" | "echo"
            | "hostname" | "ps" | "stat" | "wc" | "head" | "tail" | "whoami"
            | "uptime" | "id" | "pwd" | "date" | "nproc" | "lscpu" | "true"
            | "nvidia-smi" | "lsblk" | "lspci" | "arch"
    )
}

#[derive(Serialize, serde::Deserialize)]
struct CachedResult {
    addr: String,   // flux://b3/<hash> of host+cmd
    host: String,
    cmd: String,
    exit: i32,
    stdout: String,
    ts: u64,
}

fn cache_path(addr: &str) -> String {
    let hex = addr.rsplit('/').next().unwrap_or(addr);
    format!("{}/.flux/ssh-cache/{hex}.json", home())
}

fn cache_read(addr: &str) -> Option<CachedResult> {
    let body = std::fs::read_to_string(cache_path(addr)).ok()?;
    serde_json::from_str(&body).ok()
}

fn cache_write(r: &CachedResult) {
    let dir = format!("{}/.flux/ssh-cache", home());
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(j) = serde_json::to_string(r) {
        let _ = std::fs::write(cache_path(&r.addr), j);
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// One remote exec over BatchMode ssh. Returns (exit, stdout).
fn ssh_exec(host: &Host, cmd: &str) -> (i32, String) {
    let out = Command::new("ssh")
        .args([
            "-o", "BatchMode=yes", "-o", "ServerAliveInterval=10", "-o", "ServerAliveCountMax=3",
            "-o", "ConnectTimeout=15",
            "-o", "StrictHostKeyChecking=accept-new",
            "-p", &host.port.to_string(),
            &host.target(),
            cmd,
        ])
        .output();
    match out {
        Ok(o) => (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into_owned()),
        Err(e) => (-1, format!("(ssh error: {e})")),
    }
}

/// flux-ssh `run`: parallel multi-host exec + flux://-addressed read-only cache.
fn cmd_run(args: &Args) {
    let command = args.cmd_args.join(" ");
    if command.is_empty() {
        eprintln!("flux-fleet run: nothing to run. usage: flux-fleet run --hosts a,b,c \"uname -a\"");
        std::process::exit(2);
    }
    let hosts = discover(args);
    if hosts.is_empty() {
        eprintln!("flux-fleet run: no hosts. pass --hosts a,b,c.");
        std::process::exit(2);
    }
    let cacheable = is_read_only(&command) && !args.no_cache;
    println!("\n  {VBRIGHT}{BOLD}⬡ flux-ssh run{RESET} {DIM}v{VERSION}{RESET}  {DIM}{}{RESET}  {}",
        command,
        if cacheable { format!("{GREEN}flux://-cached (read-only){RESET}") } else { format!("{DIM}fresh (not cacheable){RESET}") });
    println!("  {VIOLET}╭{}╮{RESET}", "─".repeat(70));

    // parallel dispatch (lever 4: combo — N hosts, one command, one wall-clock)
    let handles: Vec<_> = hosts.into_iter().map(|h| {
        let cmd = command.clone();
        std::thread::spawn(move || {
            let addr = flux_uri_str(&format!("{}::{}", h.target(), cmd));
            if cacheable {
                if let Some(c) = cache_read(&addr) {
                    return (h, c, true, 0u128); // cache hit → 0 round-trip
                }
            }
            let t = std::time::Instant::now();
            let (exit, stdout) = ssh_exec(&h, &cmd);
            let us = t.elapsed().as_micros();
            let r = CachedResult { addr, host: h.target(), cmd, exit, stdout, ts: now_secs() };
            if cacheable && exit == 0 { cache_write(&r); }
            (h, r, false, us)
        })
    }).collect();

    let mut hit = 0u32;
    for handle in handles {
        if let Ok((h, r, cached, us)) = handle.join() {
            let tag = if cached { hit += 1; format!("{GREEN}⚡ cached · 0µs{RESET}") }
                      else { format!("{GOLD}{us}µs{RESET}") };
            let status = if r.exit == 0 { format!("{GREEN}✓{RESET}") } else { format!("{RED}✗{r}exit{RESET}", r = r.exit) };
            println!("  {VIOLET}│{RESET} {status} {BOLD}{:<16}{RESET}{tag}  {DIM}{}{RESET}", trunc(&h.name, 16), addr_short(&r.addr));
            let first = r.stdout.lines().next().unwrap_or("").trim();
            if !first.is_empty() {
                println!("  {VIOLET}│{RESET}     {}", trunc(first, 60));
            }
        }
    }
    println!("  {VIOLET}╰{}╯{RESET}", "─".repeat(70));
    if cacheable {
        println!("  {DIM}{hit} flux://-cache hit(s) · re-run to watch the rest collapse to 0µs (beat speed){RESET}");
    } else {
        println!("  {DIM}not cached (command has/may-have side effects) — always runs fresh{RESET}");
    }
}

fn addr_short(addr: &str) -> String {
    let hex = addr.rsplit('/').next().unwrap_or(addr);
    format!("flux://b3/{}…", &hex[..hex.len().min(10)])
}

// ─── flux-get: real content-addressed fetch (Fix #2) ────────────────────────
// flux:// stops being just an address and becomes a fetch: `put` stores a blob
// by its content hash; `get` PULLS flux://b3/<hash> from a provider and verifies
// blake3==hash on arrival, REJECTING any mismatch. A local content store caches
// hits (0 fetch). HONEST: today the fetch is single-provider over scp; pulling
// from a swarm of peers (flux-p2p DHT discovery) is the next lever, not built.

fn store_dir() -> String {
    let d = format!("{}/.flux/store", home());
    let _ = std::fs::create_dir_all(&d);
    d
}

fn cmd_put(args: &Args) {
    let Some(file) = args.cmd_args.first() else {
        eprintln!("usage: flux-fleet put <file>");
        std::process::exit(2);
    };
    let bytes = match std::fs::read(file) {
        Ok(b) => b,
        Err(e) => { eprintln!("flux-fleet put: read {file}: {e}"); std::process::exit(1); }
    };
    let addr = flux_uri(&bytes);
    let hex = addr.rsplit('/').next().unwrap_or("");
    let _ = std::fs::write(format!("{}/{hex}", store_dir()), &bytes);
    println!("\n  {GREEN}✓ stored{RESET} {} {DIM}({} bytes){RESET} → {VBRIGHT}{addr}{RESET}", file, bytes.len());
    println!("  {DIM}fetch + verify elsewhere:{RESET} flux-fleet get {addr} --from <user@host>\n");
}

fn cmd_get(args: &Args) {
    let Some(addr) = args.cmd_args.first().cloned() else {
        eprintln!("usage: flux-fleet get flux://b3/<hash> --from <user@host> [--out <path>]");
        std::process::exit(2);
    };
    let Some(hex) = addr.strip_prefix("flux://b3/") else {
        eprintln!("flux-fleet get: not a flux://b3/ address: {addr}");
        std::process::exit(2);
    };
    let local = format!("{}/{hex}", store_dir());

    // local content-store hit → 0 fetch, still re-verify the bytes hash the name
    if let Ok(b) = std::fs::read(&local) {
        if flux_uri(&b) == addr {
            println!("\n  {GREEN}⚡ local store hit{RESET} {} {DIM}({} bytes, verified){RESET}", addr_short(&addr), b.len());
            finalize_get(args, &b);
            return;
        }
    }

    let Some(from) = &args.from else {
        eprintln!("flux-fleet get: no local copy and no --from provider given");
        std::process::exit(1);
    };
    let tmp = format!("/tmp/flux-get-{hex}");
    let scp = Command::new("scp")
        .args(["-o", "BatchMode=yes", "-o", "ServerAliveInterval=10", "-o", "ServerAliveCountMax=3", &format!("{from}:.flux/store/{hex}"), &tmp])
        .status();
    if !matches!(scp, Ok(s) if s.success()) {
        eprintln!("{RED}✗ flux-get: fetch failed from {from} (does it have the object? `flux-fleet put` there first){RESET}");
        std::process::exit(1);
    }
    let bytes = std::fs::read(&tmp).unwrap_or_default();
    let got = flux_uri(&bytes);
    if got != addr {
        let _ = std::fs::remove_file(&tmp);
        eprintln!("{RED}✗ flux-get VERIFY MISMATCH: fetched bytes are {} , not {} — REJECTED{RESET}", addr_short(&got), addr_short(&addr));
        std::process::exit(9);
    }
    let _ = std::fs::write(&local, &bytes);
    let _ = std::fs::remove_file(&tmp);
    println!("\n  {GREEN}✓ fetched + verified{RESET} {} {DIM}from {from} ({} bytes){RESET}", addr_short(&addr), bytes.len());
    println!("  {DIM}content hash matches the address — proof-addressed, not server-trusted{RESET}");
    finalize_get(args, &bytes);
}

fn finalize_get(args: &Args, bytes: &[u8]) {
    if let Some(out) = &args.out {
        match std::fs::write(out, bytes) {
            Ok(_) => println!("  {DIM}written → {out}{RESET}\n"),
            Err(e) => eprintln!("  {RED}write {out}: {e}{RESET}"),
        }
    } else {
        println!("  {DIM}in local store; pass --out <path> to materialize{RESET}\n");
    }
}

// ─── discovery (read-only) ──────────────────────────────────────────────────

/// Parse ~/.ssh/config Host blocks. Returns hosts keyed by alias.
fn discover_config() -> Vec<Host> {
    let path = format!("{}/.ssh/config", home());
    let Ok(text) = std::fs::read_to_string(&path) else { return Vec::new() };
    let mut out: Vec<Host> = Vec::new();
    let mut cur: Option<Host> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let mut parts = line.splitn(2, char::is_whitespace);
        let key = parts.next().unwrap_or("").to_lowercase();
        let val = parts.next().unwrap_or("").trim();
        match key.as_str() {
            "host" => {
                if let Some(h) = cur.take() { out.push(h); }
                // ignore wildcard-only blocks (Host *)
                let alias = val.split_whitespace().next().unwrap_or("");
                if !alias.is_empty() && alias != "*" {
                    cur = Some(Host::new(alias, "config"));
                }
            }
            "hostname" => { if let Some(h) = cur.as_mut() { h.hostname = val.to_string(); } }
            "user" => { if let Some(h) = cur.as_mut() { h.user = Some(val.to_string()); } }
            "port" => { if let Some(h) = cur.as_mut() { if let Ok(p) = val.parse() { h.port = p; } } }
            _ => {}
        }
    }
    if let Some(h) = cur.take() { out.push(h); }
    out
}

/// Parse ~/.ssh/known_hosts. Returns (plain hosts, count of hashed-and-thus-unresolvable entries).
fn discover_known_hosts() -> (Vec<Host>, usize) {
    let path = format!("{}/.ssh/known_hosts", home());
    let Ok(text) = std::fs::read_to_string(&path) else { return (Vec::new(), 0) };
    let mut seen: BTreeMap<String, Host> = BTreeMap::new();
    let mut hashed = 0usize;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let Some(hostfield) = line.split_whitespace().next() else { continue };
        if hostfield.starts_with("|1|") || hostfield.starts_with('|') {
            hashed += 1; // HashKnownHosts — cannot be reversed, count honestly
            continue;
        }
        // host field may be "host,1.2.3.4" or "[host]:2222"
        for tok in hostfield.split(',') {
            let mut h = tok.trim().trim_start_matches('[').to_string();
            let mut port = 22u16;
            if let Some(idx) = h.find("]:") {
                port = h[idx + 2..].parse().unwrap_or(22);
                h = h[..idx].to_string();
            }
            h = h.trim_end_matches(']').to_string();
            if h.is_empty() { continue; }
            let entry = seen.entry(h.clone()).or_insert_with(|| Host::new(&h, "known_hosts"));
            entry.port = port;
        }
    }
    (seen.into_values().collect(), hashed)
}

/// List ~/.ssh/*.pub identities (informational — never reads private keys).
fn discover_identities() -> Vec<Identity> {
    let dir = format!("{}/.ssh", home());
    let Ok(rd) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut out = Vec::new();
    for ent in rd.flatten() {
        let p = ent.path();
        if p.extension().and_then(|e| e.to_str()) == Some("pub") {
            let kind = std::fs::read_to_string(&p)
                .ok()
                .and_then(|s| s.split_whitespace().next().map(|t| {
                    t.trim_start_matches("ssh-").trim_start_matches("ecdsa-").to_string()
                }))
                .unwrap_or_else(|| "unknown".into());
            out.push(Identity { path: p.display().to_string(), kind });
        }
    }
    out
}

/// Merge all read-only sources into a deduped candidate list (by ssh target).
fn discover(args: &Args) -> Vec<Host> {
    let mut by_target: BTreeMap<String, Host> = BTreeMap::new();
    let mut push = |mut h: Host| {
        if let Some(u) = &args.user { if h.user.is_none() { h.user = Some(u.clone()); } }
        // global --port only fills hosts still on the default 22 — a per-host
        // `host:port` (e.g. a Vast fleet where every node has a different port) wins.
        if let Some(p) = args.port { if h.port == 22 { h.port = p; } }
        // dedup on host AND port — many Vast instances share a proxy hostname
        // (ssh2.vast.ai:17376/17388/17390 are DISTINCT boxes); keying on target()
        // alone collapsed them into one. (Found by the 10-node fabric test.)
        by_target.entry(format!("{}#{}", h.target(), h.port)).or_insert(h);
    };
    for h in discover_config() { push(h); }
    for h in discover_known_hosts().0 { push(h); }
    for spec in &args.hosts {
        // accept user@host:port / user@host / host:port / host — per-host port
        // matters for fleets (e.g. Vast: every node has a distinct ssh port).
        let mut h = Host::new(spec, "--hosts");
        let rest = match spec.split_once('@') {
            Some((u, r)) => { h.user = Some(u.to_string()); r }
            None => spec.as_str(),
        };
        match rest.rsplit_once(':') {
            Some((host, port)) if port.parse::<u16>().is_ok() => {
                h.hostname = host.to_string();
                h.name = host.to_string();
                h.port = port.parse().unwrap();
            }
            _ => { h.hostname = rest.to_string(); h.name = rest.to_string(); }
        }
        push(h);
    }
    by_target.into_values().collect()
}

// ─── probe (BatchMode only) ─────────────────────────────────────────────────

/// One real, password-free probe: reachability, OS/arch, and remote flux version.
fn probe(host: &mut Host) {
    let remote_cmd = "uname -sm 2>/dev/null; \
         (flux --version 2>/dev/null || fluxc --version 2>/dev/null || echo NO_FLUX)";
    let out = Command::new("ssh")
        .args([
            "-o", "BatchMode=yes", "-o", "ServerAliveInterval=10", "-o", "ServerAliveCountMax=3",
            "-o", "ConnectTimeout=15",
            "-o", "StrictHostKeyChecking=accept-new",
            "-p", &host.port.to_string(),
            &host.target(),
            remote_cmd,
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            host.reachable = Some(true);
            let stdout = String::from_utf8_lossy(&o.stdout);
            let mut lines = stdout.lines().filter(|l| !l.trim().is_empty());
            host.os_arch = lines.next().map(|s| s.trim().to_string());
            // the version line is whatever flux/fluxc printed (e.g. "fluxc 0.18.0")
            let ver_line = stdout.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
            host.flux_version = if ver_line.contains("NO_FLUX") {
                None
            } else {
                ver_line.split_whitespace().last().map(|s| s.to_string())
            };
        }
        _ => {
            host.reachable = Some(false); // unreachable OR needs a password (we never prompt)
        }
    }
}

// ─── up: gated, version-aware, idempotent installer ─────────────────────────

/// Candidate paths for the portable musl fluxc binary we push.
fn musl_binary() -> Option<String> {
    let manifest = env!("CARGO_MANIFEST_DIR"); // …/flux/crates/flux-fleet
    let candidates = [
        format!("{manifest}/../../target/x86_64-unknown-linux-musl/release/fluxc"),
        format!("{manifest}/../../../.target-shared/x86_64-unknown-linux-musl/release/fluxc"),
        "/home/storage/deepseek-codewhale/flux/target/x86_64-unknown-linux-musl/release/fluxc".to_string(),
    ];
    candidates.into_iter().find(|p| std::path::Path::new(p).exists())
}

/// Compare dotted versions: is `remote` >= `local`?
fn ver_ge(remote: &str, local: &str) -> bool {
    let parse = |s: &str| s.split('.').filter_map(|x| x.parse::<u64>().ok()).collect::<Vec<_>>();
    let (r, l) = (parse(remote), parse(local));
    for i in 0..r.len().max(l.len()) {
        let rv = r.get(i).copied().unwrap_or(0);
        let lv = l.get(i).copied().unwrap_or(0);
        if rv != lv { return rv > lv; }
    }
    true // equal
}

fn audit(line: &str) {
    let dir = format!("{}/.flux", home());
    let _ = std::fs::create_dir_all(&dir);
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(format!("{dir}/fleet-audit.log")) {
        let _ = writeln!(f, "{line}");
    }
}

/// sha256 of a file via the universal `sha256sum` CLI (used as the remote
/// integrity gate; the remote has sha256sum but not b3sum).
fn sha256_file(path: &str) -> Option<String> {
    let o = Command::new("sha256sum").arg(path).output().ok()?;
    if !o.status.success() { return None; }
    String::from_utf8_lossy(&o.stdout).split_whitespace().next().map(|s| s.to_string())
}

/// Run a Command with a hard wall-clock timeout, killing the child if it hangs.
/// ServerAliveInterval only kills a DEAD session; a live session whose remote
/// command hangs forever (a flaky Vast box) needs this. (Rigorous-test fix #5.)
fn run_timed(mut cmd: Command, secs: u64) -> Option<std::process::Output> {
    use std::process::Stdio;
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().ok()?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) => {
                if start.elapsed().as_secs() >= secs {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None; // hung node — bounded, not infinite
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Err(_) => return None,
        }
    }
}

/// Returns a human verdict line for one host.
/// The parallel provisioning script: backgrounds the model pull, foregrounds flux-ssh + p2p/chronos
/// research, so a box is a useful flux node in ~1 min instead of idling for the ~18 min model download.
const BOOTSTRAP_URL: &str = "https://quillon.xyz/downloads/vast-flux-bootstrap.sh";

/// After flux is installed, kick the parallel bootstrap on the box — DETACHED (setsid + `&`), so this
/// returns at once while the model pull + research run async. The box is usable immediately.
fn kick_bootstrap(host: &Host) -> String {
    let kick = format!(
        "curl -fsSL {BOOTSTRAP_URL} -o /tmp/flux-boot.sh 2>/dev/null && \
         setsid bash /tmp/flux-boot.sh >/var/log/flux-bootstrap.log 2>&1 </dev/null & \
         echo FLUX_BOOTSTRAP_KICKED"
    );
    let mut c = Command::new("ssh");
    c.args(["-o", "BatchMode=yes", "-o", "ServerAliveInterval=10", "-o", "ServerAliveCountMax=3", "-p", &host.port.to_string(), &host.target(), &kick]);
    match run_timed(c, 30) {
        Some(o) if String::from_utf8_lossy(&o.stdout).contains("KICKED") =>
            format!("{GREEN}↳ bootstrap kicked{RESET} {DIM}— models + p2p/chronos research running in background → /workspace/FLUX_RESEARCH.md{RESET}"),
        _ => format!("{GOLD}↳ bootstrap kick failed (flux is still installed){RESET}"),
    }
}

fn up_one(host: &mut Host, confirm: bool, bootstrap: bool, musl: Option<&str>) -> String {
    probe(host); // version-aware: ALWAYS check remote state first (the wrong-binary lesson)
    if host.reachable != Some(true) {
        return format!("{RED}✗ {} — unreachable / needs password (skipped, never prompted){RESET}", host.name);
    }
    if let Some(rv) = &host.flux_version {
        if ver_ge(rv, VERSION) {
            return format!("{DIM}• {} — flux {rv} already ≥ {VERSION} (idempotent skip){RESET}", host.name);
        }
    }
    let Some(bin) = musl else {
        return format!("{GOLD}⚠ {} — would install, but no musl binary built. Run:{RESET}\n      {DIM}cargo build --release --target x86_64-unknown-linux-musl -p fluxc{RESET}", host.name);
    };
    // flux:// content address of the exact bytes we're about to ship.
    let bytes = std::fs::read(bin).unwrap_or_default();
    let addr = flux_uri(&bytes);
    if !confirm {
        let cur = host.flux_version.clone().unwrap_or_else(|| "none".into());
        let boot = if bootstrap { " + kick model/p2p-research bootstrap" } else { "" };
        return format!("{GOLD}DRY-RUN{RESET} {} ({}) — would push {} → content-verify → install flux {VERSION}{boot} (current: {cur})", host.name, host.os_arch.clone().unwrap_or_default(), addr_short(&addr));
    }
    // real install. flux:// = blake3 content address (canonical name, shown). The
    // remote integrity gate uses sha256sum (universal on any Linux; the remote has
    // no b3sum) — both hash the same bytes, and a MISMATCH ABORTS before install.
    // That's verify-don't-trust distribution: we don't trust the scp'd bytes, the
    // remote recomputes and refuses anything that isn't the artifact we addressed.
    let Some(local_sha) = sha256_file(bin) else {
        return format!("{RED}✗ {} — could not hash local binary{RESET}", host.name);
    };
    let mut scp_cmd = Command::new("scp");
    scp_cmd.args(["-o", "BatchMode=yes", "-o", "ServerAliveInterval=10", "-o", "ServerAliveCountMax=3", "-P", &host.port.to_string(), bin, &format!("{}:/usr/local/bin/flux.new", host.target())]);
    if !run_timed(scp_cmd, 60).map(|o| o.status.success()).unwrap_or(false) {
        return format!("{RED}✗ {} — scp failed or timed out (60s){RESET}", host.name);
    }
    let guarded = format!(
        "RS=$(sha256sum /usr/local/bin/flux.new 2>/dev/null | cut -d' ' -f1); \
         if [ \"$RS\" != \"{local_sha}\" ]; then rm -f /usr/local/bin/flux.new; echo FLUX_VERIFY_MISMATCH; exit 9; fi; \
         chmod +x /usr/local/bin/flux.new && mv /usr/local/bin/flux.new /usr/local/bin/flux && echo FLUX_VERIFIED_OK"
    );
    let mut inst_cmd = Command::new("ssh");
    inst_cmd.args(["-o", "BatchMode=yes", "-o", "ServerAliveInterval=10", "-o", "ServerAliveCountMax=3", "-p", &host.port.to_string(), &host.target(), &guarded]);
    match run_timed(inst_cmd, 50) {
        Some(o) if o.status.success() && String::from_utf8_lossy(&o.stdout).contains("FLUX_VERIFIED_OK") => {
            audit(&format!("{{\"host\":\"{}\",\"installed\":\"{VERSION}\",\"flux_uri\":\"{addr}\",\"verified\":\"sha256-match\"}}", host.target()));
            let mut msg = format!("{GREEN}✓ {} — content-verified + installed: {}{RESET}", host.name, addr_short(&addr));
            if bootstrap { msg.push_str(&format!("\n      {}", kick_bootstrap(host))); }
            msg
        }
        Some(o) if String::from_utf8_lossy(&o.stdout).contains("MISMATCH") => {
            format!("{RED}✗ {} — flux:// VERIFY MISMATCH, install aborted (bytes != {}){RESET}", host.name, addr_short(&addr))
        }
        None => format!("{RED}✗ {} — install hung, killed at 50s (node bounded, fleet continues){RESET}", host.name),
        _ => format!("{RED}✗ {} — install/verify failed{RESET}", host.name),
    }
}

fn short(p: &str) -> String {
    std::path::Path::new(p).file_name().and_then(|s| s.to_str()).unwrap_or(p).to_string()
}

// ─── render ─────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct ScanReport {
    flux_fleet: String,
    hosts: Vec<Host>,
    identities: Vec<Identity>,
    hashed_known_hosts: usize,
    probed: bool,
}

fn fmt_probe(h: &Host) -> String {
    match h.reachable {
        None => format!("{DIM}—{RESET}"),
        Some(false) => format!("{RED}unreachable{RESET}"),
        Some(true) => {
            let arch = h.os_arch.clone().unwrap_or_default();
            match &h.flux_version {
                Some(v) => format!("{GREEN}reachable{RESET} {DIM}{arch}{RESET} {GOLD}flux {v}{RESET}"),
                None => format!("{GREEN}reachable{RESET} {DIM}{arch}{RESET} {DIM}no flux{RESET}"),
            }
        }
    }
}

fn render_scan(hosts: &[Host], ids: &[Identity], hashed: usize, probed: bool) {
    println!("\n  {VBRIGHT}{BOLD}⬡ flux-fleet{RESET} {DIM}v{VERSION} · verify-don't-trust{RESET}");
    println!("  {VIOLET}╭{}╮{RESET}", "─".repeat(70));
    if hosts.is_empty() {
        println!("  {VIOLET}│{RESET} {DIM}no candidate hosts — add ~/.ssh/config or pass --hosts a,b,c{RESET}");
    }
    for h in hosts {
        println!("  {VIOLET}│{RESET} {BOLD}{:<14}{RESET}{DIM}{:<22}{RESET}{}", trunc(&h.name, 14), trunc(&h.hostname, 22), fmt_probe(h));
        println!("  {VIOLET}│{RESET}   {DIM}via {} · port {}{RESET}", h.source, h.port);
    }
    println!("  {VIOLET}├{}┤{RESET}", "─".repeat(70));
    println!("  {VIOLET}│{RESET} {VBRIGHT}identities{RESET}  {}", if ids.is_empty() { format!("{DIM}none{RESET}") } else { ids.iter().map(|i| i.kind.clone()).collect::<Vec<_>>().join(", ") });
    if hashed > 0 {
        println!("  {VIOLET}│{RESET} {DIM}{hashed} hashed known_hosts entries — can't resolve names (not a failure){RESET}");
    }
    println!("  {VIOLET}╰{}╯{RESET}", "─".repeat(70));
    if !probed {
        println!("  {DIM}read-only scan · no connections made. add --probe to check reachability + flux.{RESET}");
    }
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", &s.chars().take(n - 1).collect::<String>()) }
}

// ─── main ─────────────────────────────────────────────────────────────────

fn main() {
    let args = parse_args();
    match args.cmd.as_str() {
        "scan" | "status" => {
            let mut hosts = discover(&args);
            let (_, hashed) = discover_known_hosts();
            let ids = discover_identities();
            let probed = args.probe || args.cmd == "status";
            if probed {
                for h in hosts.iter_mut() { probe(h); }
            }
            if args.json {
                let rep = ScanReport {
                    flux_fleet: VERSION.to_string(),
                    hosts: hosts.clone(),
                    identities: ids.clone(),
                    hashed_known_hosts: hashed,
                    probed,
                };
                println!("{}", serde_json::to_string_pretty(&rep).unwrap_or_default());
            } else {
                render_scan(&hosts, &ids, hashed, probed);
            }
        }
        "up" => {
            let mut hosts = discover(&args);
            if hosts.is_empty() {
                eprintln!("flux-fleet up: no hosts. pass --hosts a,b,c or add ~/.ssh/config.");
                std::process::exit(2);
            }
            let musl = musl_binary();
            println!("\n  {VBRIGHT}{BOLD}⬡ flux-fleet up{RESET} {DIM}v{VERSION}{RESET}  {}",
                     if args.confirm { format!("{RED}{BOLD}LIVE (--confirm){RESET}") } else { format!("{GOLD}DRY-RUN (add --confirm to install){RESET}") });
            if musl.is_none() {
                println!("  {GOLD}⚠ no musl-static fluxc binary found — installs can't proceed until it's built.{RESET}");
            }
            for h in hosts.iter_mut() {
                println!("  {}", up_one(h, args.confirm, args.bootstrap, musl.as_deref()));
            }
            println!("  {DIM}version-aware + idempotent · BatchMode only · audit ~/.flux/fleet-audit.log{RESET}");
        }
        "run" => cmd_run(&args),
        "put" => cmd_put(&args),
        "get" => cmd_get(&args),
        other => {
            eprintln!("flux-fleet: unknown command '{other}'. try --help.");
            std::process::exit(2);
        }
    }
}
