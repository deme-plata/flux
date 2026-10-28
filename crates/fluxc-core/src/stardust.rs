//! stardust — the trail a build leaves inside a binary, read back WITHOUT running it.
//!
//! Why this exists (all measured on Epsilon, October 2026):
//! * 2026-10-26: sigil-top 10.7.4 was published as a **g2** build and auto-updated a g3
//!   server onto the dead chain. Nothing read the network id back out of the artifact
//!   before it went into the signed manifest.
//! * 2026-10-10: `systemctl cat` showed the base `ExecStart=` while a drop-in ran a
//!   different binary; a 7-minute release build went to the wrong target dir.
//! * 2026-10-22: a watchdog restart left systemd's `NRestarts` at 0 — MainPID and
//!   ActiveEnterTimestamp are the receipt, not the counter.
//! * `sigil-miner --help` starts mining and writes a wallet file: a binary must never be
//!   executed just to find out what it is.
//!
//! So `fluxc stardust <path | unit | pid | manifest-url>` scans the printable runs of the
//! file (what `strings` does) and reports the dust it finds: the dominant network id, the
//! flux-rev source stamp, the rustc that compiled it, product/version strings, BLAKE3 and
//! SHA-256. For a systemd unit it also compares the EFFECTIVE `ExecStart` binary with
//! `/proc/<MainPID>/exe` and says whether the file on disk is newer than the running
//! process (built, not restarted). For a release manifest URL it downloads the named
//! target, checks blake3 + size, and reads the dust — the 10.7.4 incident in one command.
//! The target is executed only with an explicit `--exec` (runs `<bin> network-id`).
//!
//! How the network is read, and why NOT by counting the id. Counting `sigil-gN` literals
//! was tried first and REFUTED the same hour (2026-10-28): the g2 build 10.7.4 and the g3
//! build 10.7.5 of sigil-top have practically identical counts (67/13/6/4 vs 66/13/6/4) —
//! the linker merges identical literals, so a count measures how many distinct strings
//! MENTION a network, not which one the binary was built for. What does flip with the build
//! is the set of gossip WIRE TOPICS sigil-net concatenates at compile time
//! (`/sigil/<short>/blocks`, `/txs`, `/release`, …): the g2 build carries only `/sigil/g2/*`,
//! the g3 build only `/sigil/g3/*`. A binary can mention another network; it only tunes to
//! the topics of its own. Verdict = the short id with the most DISTINCT known topics, at
//! least [`MIN_TOPICS`] of them, and at least [`TOPIC_MARGIN`]× the runner-up's topic
//! OCCURRENCES. Measured: the g2 build 10.7.4 = g2×25 / g0×3 / g3×0; the g3 build 10.7.5 =
//! g3×21 / g2×5 / g0×3 (the five g2 are a hardcoded self-check in sigil-net's source; the
//! three g0 are the embedded wallet page's legacy JavaScript). Neither can win.
//! Mentions are still reported, as information. Quillon has one network; `mainnet-genesis`
//! is reported by mention and labelled so.

use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// sigil-net's compile-time gossip topics (`concat!("/sigil/", SIGIL_NET_SHORT, "/…")`).
pub const WIRE_TOPICS: &[&str] = &["blocks", "peer-heights", "tip-proofs", "txs", "release", "finality-votes"];
/// A network is called only with at least this many DISTINCT wire topics present…
pub const MIN_TOPICS: usize = 3;
/// …and at least this many times the runner-up's topic occurrences.
pub const TOPIC_MARGIN: usize = 2;
/// Shortest printable run worth looking at.
pub const MIN_RUN: usize = 6;
/// Refuse to download artifacts above this (a sigil-top is ~47 MB).
pub const MAX_ARTIFACT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct Opts {
    /// Run `<bin> network-id` (5 s timeout) for the authoritative network. OFF by default.
    pub exec: bool,
    /// Manifest target to pick (default linux-x64).
    pub triple: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct UnitFacts {
    pub id: String,
    pub active_state: String,
    pub main_pid: u32,
    pub active_enter: String,
    /// The binary named by the EFFECTIVE ExecStart (drop-ins applied).
    pub exec_start_path: Option<String>,
    /// What `/proc/<MainPID>/exe` actually points at.
    pub running_exe: Option<String>,
    /// The running file was replaced/removed on disk (" (deleted)" suffix).
    pub running_exe_deleted: bool,
    /// The `systemctl cat` lie check: does the running exe equal the ExecStart binary?
    pub exe_matches_exec_start: Option<bool>,
    pub process_start_unix: Option<u64>,
    /// Built but not restarted: the file's mtime is after the process started.
    pub file_newer_than_process: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ManifestFacts {
    pub url: String,
    pub product: Option<String>,
    pub version: Option<String>,
    pub target: String,
    pub artifact_url: Option<String>,
    pub blake3_expected: Option<String>,
    pub size_expected: Option<u64>,
    pub blake3_ok: Option<bool>,
    pub size_ok: Option<bool>,
    pub flux_rev_manifest: Option<String>,
    pub from_cache: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Dust {
    pub target: String,
    pub kind: String,
    pub file: Option<String>,
    pub size_bytes: Option<u64>,
    pub mtime_unix: Option<u64>,
    pub blake3: Option<String>,
    pub sha256: Option<String>,
    pub rustc: Option<String>,
    pub flux_rev: Option<String>,
    pub versions: Vec<String>,
    /// Every network id MENTIONED, with its count, highest first — information, not a verdict.
    pub mentions: Vec<(String, usize)>,
    /// Compile-time wire topics per short network id (`g3` → ["blocks", "txs", …]).
    pub wire_topics: Vec<(String, Vec<String>)>,
    /// Occurrences of known wire topics per short id (`g3` → 21).
    pub topic_hits: Vec<(String, usize)>,
    /// The network this binary was built for, when the wire topics say so.
    pub network: Option<String>,
    /// "wire-topics" | "mention" (one id, no topics — e.g. a Quillon binary) | "none"
    pub network_basis: String,
    pub network_ambiguous: bool,
    /// Output of `<bin> network-id`, only with `--exec`.
    pub network_exec: Option<String>,
    pub unit: Option<UnitFacts>,
    pub manifest: Option<ManifestFacts>,
    pub warnings: Vec<String>,
    pub ok: bool,
    pub elapsed_ms: u64,
}

/// What a byte scan finds. Pure; unit-tested.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scan {
    pub rustc: Option<String>,
    pub flux_rev: Option<String>,
    pub versions: Vec<String>,
    pub mentions: Vec<(String, usize)>,
    pub wire_topics: Vec<(String, Vec<String>)>,
    pub topic_hits: Vec<(String, usize)>,
}

// ───────────────────────────── entry ─────────────────────────────

/// Read the dust off `target`: `http(s)://…` = manifest URL, digits = pid,
/// an existing path = file, anything else = systemd unit name.
pub fn read(target: &str, opts: &Opts) -> Dust {
    let t0 = Instant::now();
    let mut d = if target.starts_with("http://") || target.starts_with("https://") {
        read_url(target, opts)
    } else if !target.is_empty() && target.bytes().all(|b| b.is_ascii_digit()) {
        read_pid(target.parse().unwrap_or(0), opts)
    } else if Path::new(target).exists() {
        read_path(Path::new(target), opts)
    } else {
        read_unit(target, opts)
    };
    d.target = target.to_string();
    d.elapsed_ms = t0.elapsed().as_millis() as u64;
    d
}

pub fn read_path(p: &Path, opts: &Opts) -> Dust {
    let mut d = Dust { kind: "path".into(), ..Default::default() };
    fill_file(&mut d, p, opts);
    d
}

pub fn read_pid(pid: u32, opts: &Opts) -> Dust {
    let mut d = Dust { kind: "pid".into(), ..Default::default() };
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok();
    let mut uf = UnitFacts { id: format!("pid {pid}"), main_pid: pid, ..Default::default() };
    match exe {
        Some(e) => {
            let (clean, deleted) = split_deleted(&e);
            uf.running_exe = Some(e.to_string_lossy().into_owned());
            uf.running_exe_deleted = deleted;
            uf.process_start_unix = process_start_unix(pid);
            fill_file(&mut d, &clean, opts);
            uf.file_newer_than_process = match (d.mtime_unix, uf.process_start_unix) {
                (Some(m), Some(s)) => Some(m > s),
                _ => None,
            };
            if deleted {
                d.warnings.push("the running executable was replaced or removed on disk — the dust above is from the CURRENT file, not the running process".into());
            }
        }
        None => {
            d.warnings.push(format!("/proc/{pid}/exe not readable (no such process, or not root)"));
        }
    }
    d.unit = Some(uf);
    d.ok = d.file.is_some();
    d
}

pub fn read_unit(unit: &str, opts: &Opts) -> Dust {
    let mut d = Dust { kind: "unit".into(), ..Default::default() };
    let name = if unit.contains('.') { unit.to_string() } else { format!("{unit}.service") };
    let out = Command::new("systemctl")
        .args(["show", &name, "-p", "Id,ActiveState,MainPID,ActiveEnterTimestamp,ExecStart"])
        .output();
    let text = match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        Ok(o) => {
            d.warnings.push(format!("systemctl show {name}: {}", String::from_utf8_lossy(&o.stderr).trim()));
            return d;
        }
        Err(e) => {
            d.warnings.push(format!("systemctl not runnable: {e}"));
            return d;
        }
    };
    let mut uf = parse_systemctl_show(&text);
    if uf.id.is_empty() { uf.id = name.clone(); }
    if uf.main_pid == 0 {
        d.warnings.push(format!("{} is not running (ActiveState={})", uf.id, uf.active_state));
        // Still read the ExecStart binary's dust — that is what WOULD run.
        if let Some(p) = uf.exec_start_path.clone() {
            fill_file(&mut d, Path::new(&p), opts);
        }
        d.unit = Some(uf);
        d.ok = d.file.is_some();
        return d;
    }
    let exe = std::fs::read_link(format!("/proc/{}/exe", uf.main_pid)).ok();
    let (clean, deleted) = match &exe {
        Some(e) => split_deleted(e),
        None => (PathBuf::new(), false),
    };
    uf.running_exe = exe.as_ref().map(|e| e.to_string_lossy().into_owned());
    uf.running_exe_deleted = deleted;
    uf.process_start_unix = process_start_unix(uf.main_pid);
    if let Some(es) = &uf.exec_start_path {
        let es_canon = std::fs::canonicalize(es).unwrap_or_else(|_| PathBuf::from(es));
        if !clean.as_os_str().is_empty() {
            let m = es_canon == clean;
            uf.exe_matches_exec_start = Some(m);
            if !m {
                d.warnings.push(format!(
                    "ExecStart names {} but the process runs {} — a drop-in or a stale start; trust /proc, not `systemctl cat`",
                    es_canon.display(), clean.display()));
            }
        }
    }
    let scan_target = if !clean.as_os_str().is_empty() { clean.clone() }
        else { uf.exec_start_path.clone().map(PathBuf::from).unwrap_or_default() };
    if !scan_target.as_os_str().is_empty() {
        fill_file(&mut d, &scan_target, opts);
        uf.file_newer_than_process = match (d.mtime_unix, uf.process_start_unix) {
            (Some(m), Some(s)) => Some(m > s),
            _ => None,
        };
        if uf.file_newer_than_process == Some(true) {
            d.warnings.push("the binary on disk is NEWER than the running process — built, not restarted (the fix is in the file, not running)".into());
        }
        if deleted {
            d.warnings.push("the running executable was replaced on disk — the dust above is from the CURRENT file, not the running process".into());
        }
    }
    d.unit = Some(uf);
    d.ok = d.file.is_some();
    d
}

pub fn read_url(url: &str, opts: &Opts) -> Dust {
    let mut d = Dust { kind: "url".into(), ..Default::default() };
    let triple = if opts.triple.is_empty() { "linux-x64".to_string() } else { opts.triple.clone() };
    let mut mf = ManifestFacts { url: url.to_string(), target: triple.clone(), ..Default::default() };
    let body = match curl(url, None, 30) {
        Ok(b) => b,
        Err(e) => { d.warnings.push(format!("fetch manifest: {e}")); d.manifest = Some(mf); return d; }
    };
    let m: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            let ct = String::from_utf8_lossy(&body[..body.len().min(80)]).to_string();
            d.warnings.push(format!("manifest is not JSON ({e}); first bytes: {ct:?} — a 200 with HTML is a missing file, not a manifest"));
            d.manifest = Some(mf);
            return d;
        }
    };
    mf.product = m.get("product").and_then(|v| v.as_str()).map(str::to_string);
    mf.version = m.get("version").and_then(|v| v.as_str()).map(str::to_string);
    mf.flux_rev_manifest = m.get("flux_rev").and_then(|v| v.as_str()).map(str::to_string);
    let Some((aurl, b3, size)) = pick_manifest_target(&m, &triple) else {
        d.warnings.push(format!("manifest has no target {triple:?} and no top-level url/blake3_hex"));
        d.manifest = Some(mf);
        return d;
    };
    mf.artifact_url = Some(aurl.clone());
    mf.blake3_expected = b3.clone();
    mf.size_expected = size;
    if let Some(s) = size { if s > MAX_ARTIFACT_BYTES {
        d.warnings.push(format!("artifact declares {s} bytes > {MAX_ARTIFACT_BYTES} cap — not downloading"));
        d.manifest = Some(mf); return d;
    } }
    // Cache by expected blake3 (or a hash of the url) so a re-read is free.
    let key = b3.clone().unwrap_or_else(|| blake3::hash(aurl.as_bytes()).to_hex().to_string());
    let dir = cache_dir().join("stardust");
    let _ = std::fs::create_dir_all(&dir);
    let dest = dir.join(&key);
    let mut from_cache = false;
    if dest.exists() {
        if let (Some(exp), Ok(bytes)) = (&b3, std::fs::read(&dest)) {
            if blake3::hash(&bytes).to_hex().as_str() == exp.as_str() { from_cache = true; }
        }
    }
    if !from_cache {
        let tmp = dir.join(format!("{key}.part-{}", std::process::id()));
        match curl(&aurl, Some(&tmp), 900) {
            Ok(_) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
                }
                if let Err(e) = std::fs::rename(&tmp, &dest) { d.warnings.push(format!("rename: {e}")); }
            }
            Err(e) => { let _ = std::fs::remove_file(&tmp); d.warnings.push(format!("download artifact: {e}")); d.manifest = Some(mf); return d; }
        }
    }
    mf.from_cache = from_cache;
    fill_file(&mut d, &dest, opts);
    if let (Some(exp), Some(got)) = (&mf.blake3_expected, &d.blake3) {
        let ok = exp.eq_ignore_ascii_case(got);
        mf.blake3_ok = Some(ok);
        if !ok { d.warnings.push("blake3 of the downloaded artifact ≠ manifest blake3_hex".into()); }
    }
    if let (Some(exp), Some(got)) = (mf.size_expected, d.size_bytes) {
        let ok = exp == got;
        mf.size_ok = Some(ok);
        if !ok { d.warnings.push(format!("size {got} ≠ manifest size_bytes {exp}")); }
    }
    // The manifest's `flux_rev` is only comparable when it is a real flux-rev id (≥ 16 hex);
    // sigil-top's release script writes the git commit there (8 hex) — not the same thing.
    if let (Some(mr), Some(dr)) = (&mf.flux_rev_manifest, &d.flux_rev) {
        let comparable = mr.len() >= 16 && mr.bytes().all(|c| c.is_ascii_hexdigit());
        let dr_short = dr.trim_start_matches("full:");
        if comparable && !dr_short.starts_with(mr.as_str()) {
            d.warnings.push(format!("manifest flux_rev {mr} is not the binary's flux-rev {dr} — republished artifact?"));
        }
    }
    d.ok = d.file.is_some() && mf.blake3_ok != Some(false) && mf.size_ok != Some(false);
    d.manifest = Some(mf);
    d
}

// ───────────────────────────── file ─────────────────────────────

fn fill_file(d: &mut Dust, p: &Path, opts: &Opts) {
    let canon = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    d.file = Some(canon.to_string_lossy().into_owned());
    let bytes = match std::fs::read(&canon) {
        Ok(b) => b,
        Err(e) => { d.warnings.push(format!("read {}: {e}", canon.display())); return; }
    };
    if let Ok(md) = std::fs::metadata(&canon) {
        d.size_bytes = Some(md.len());
        d.mtime_unix = md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|x| x.as_secs());
    }
    let (b3, s2) = hashes(&bytes);
    d.blake3 = Some(b3);
    d.sha256 = Some(s2);
    let scan = scan_bytes(&bytes);
    d.rustc = scan.rustc;
    d.flux_rev = scan.flux_rev;
    d.versions = scan.versions;
    let (net, basis, amb) = network_verdict(&scan.wire_topics, &scan.topic_hits, &scan.mentions);
    d.mentions = scan.mentions;
    d.wire_topics = scan.wire_topics;
    d.topic_hits = scan.topic_hits;
    d.network = net;
    d.network_basis = basis;
    d.network_ambiguous = amb;
    if amb {
        d.warnings.push(format!("network ambiguous — wire-topic occurrences {:?} too close; use --exec for `network-id`", d.topic_hits));
    }
    if d.rustc.is_none() {
        d.warnings.push("no `rustc version` string — not a Rust binary, or stripped".into());
    }
    if opts.exec {
        match exec_network_id(&canon) {
            Ok(Some(line)) => {
                // sigil-top prints `header=sigil-g3 net=sigil-g3 g3_rules=true`; take the net= token when present.
                let net = line.split_whitespace().find_map(|t| t.strip_prefix("net=")).unwrap_or(line.as_str()).to_string();
                if let Some(n) = &d.network {
                    if &net != n { d.warnings.push(format!("`network-id` says {net} but the wire topics said {n} — trust network-id")); }
                }
                d.network_exec = Some(line);
            }
            Ok(None) => d.warnings.push("`network-id` printed nothing within 5 s".into()),
            Err(e) => d.warnings.push(format!("--exec: {e}")),
        }
    }
    d.ok = true;
}

pub fn hashes(bytes: &[u8]) -> (String, String) {
    use sha2::Digest;
    let b3 = blake3::hash(bytes).to_hex().to_string();
    let s2 = hex::encode(sha2::Sha256::digest(bytes));
    (b3, s2)
}

/// Run `<bin> network-id` with a 5 s timeout. Only called on explicit request.
fn exec_network_id(bin: &Path) -> Result<Option<String>, String> {
    let mut child = Command::new(bin).arg("network-id")
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).stdin(std::process::Stdio::null())
        .spawn().map_err(|e| format!("cannot run {}: {e} (is the execute bit set?)", bin.display()))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => { let _ = child.kill(); let _ = child.wait(); return Err("network-id did not exit within 5 s (killed)".into()); }
        }
    }
    let mut out = String::new();
    use std::io::Read;
    if let Some(mut so) = child.stdout.take() { let _ = so.read_to_string(&mut out); }
    Ok(out.lines().next().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()))
}

// ───────────────────────────── pure parsers ─────────────────────────────

/// Printable ASCII runs of at least `min` bytes — what `strings -n <min>` prints.
pub fn printable_runs(bytes: &[u8], min: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, &b) in bytes.iter().enumerate() {
        let printable = (0x20..0x7f).contains(&b);
        match (printable, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                if i - s >= min { if let Ok(st) = std::str::from_utf8(&bytes[s..i]) { out.push(st); } }
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start { if bytes.len() - s >= min { if let Ok(st) = std::str::from_utf8(&bytes[s..]) { out.push(st); } } }
    out
}

/// Scan a binary's bytes for the dust we know how to read.
pub fn scan_bytes(bytes: &[u8]) -> Scan {
    let mut scan = Scan::default();
    let mut nets: std::collections::BTreeMap<String, usize> = Default::default();
    let mut topics: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> = Default::default();
    let mut hits: std::collections::BTreeMap<String, usize> = Default::default();
    let mut versions: std::collections::BTreeSet<String> = Default::default();
    for run in printable_runs(bytes, MIN_RUN) {
        if scan.rustc.is_none() {
            if let Some(i) = run.find("rustc version ") {
                let tail = &run[i..];
                let end = tail.find(')').map(|e| e + 1).unwrap_or(tail.len().min(60));
                scan.rustc = Some(tail[..end].to_string());
            }
        }
        if scan.flux_rev.is_none() {
            if let Some(i) = run.find("full:") {
                let hexrun: String = run[i + 5..].chars().take_while(|c| c.is_ascii_hexdigit()).take(64).collect();
                if hexrun.len() >= 16 { scan.flux_rev = Some(format!("full:{hexrun}")); }
            }
        }
        count_networks(run, &mut nets);
        collect_wire_topics(run, &mut topics, &mut hits);
        for v in version_tokens(run) { versions.insert(v); }
    }
    let mut n: Vec<(String, usize)> = nets.into_iter().collect();
    n.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    scan.mentions = n;
    let mut t: Vec<(String, Vec<String>)> = topics.into_iter().map(|(k, v)| (k, v.into_iter().collect())).collect();
    t.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
    scan.wire_topics = t;
    let mut h: Vec<(String, usize)> = hits.into_iter().collect();
    h.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    scan.topic_hits = h;
    scan.versions = versions.into_iter().take(12).collect();
    scan
}

/// Count SIGIL network-id MENTIONS — `sigil-` + a letter + a digit (g2, g3, r1, m1 …), then a
/// boundary — and Quillon's `mainnet-genesis`, in one printable run. The letter+digit shape
/// keeps crate names (`sigil-tx`, `sigil-vm`, `sigil-pq`) out of the count.
pub fn count_networks(run: &str, nets: &mut std::collections::BTreeMap<String, usize>) {
    let b = run.as_bytes();
    let mut i = 0;
    while let Some(off) = run[i..].find("sigil-") {
        let s = i + off;
        let e = s + 8;
        if e <= b.len()
            && b[s + 6].is_ascii_lowercase()
            && b[s + 7].is_ascii_digit()
            && (e == b.len() || !(b[e].is_ascii_alphanumeric() || b[e] == b'-' || b[e] == b'_'))
        {
            *nets.entry(run[s..e].to_string()).or_insert(0) += 1;
        }
        i = s + 6;
    }
    let mut j = 0;
    while let Some(off) = run[j..].find("mainnet-genesis") {
        *nets.entry("mainnet-genesis".to_string()).or_insert(0) += 1;
        j += off + 15;
    }
}

/// `/sigil/<short>/<topic>` where topic is one of [`WIRE_TOPICS`] (prefix match: Rust string
/// literals have no terminator, so the next literal in rodata is glued on — "/sigil/g3/txstokio").
pub fn collect_wire_topics(run: &str, topics: &mut std::collections::BTreeMap<String, std::collections::BTreeSet<String>>, hits: &mut std::collections::BTreeMap<String, usize>) {
    let mut i = 0;
    while let Some(off) = run[i..].find("/sigil/") {
        let s = i + off + 7;
        if let Some(rest) = run.get(s..) {
            let b = rest.as_bytes();
            if b.len() > 3 && b[0].is_ascii_alphanumeric() && b[1].is_ascii_alphanumeric() && b[2] == b'/' {
                let short = &rest[..2];
                let tail = &rest[3..];
                if let Some(t) = WIRE_TOPICS.iter().find(|t| tail.starts_with(**t)) {
                    topics.entry(short.to_string()).or_default().insert((*t).to_string());
                    *hits.entry(short.to_string()).or_insert(0) += 1;
                }
            }
        }
        i = s;
    }
}

/// The verdict: (network, basis, ambiguous). Wire topics decide: the id with the most
/// occurrences wins when it has ≥ MIN_TOPICS distinct topics and ≥ TOPIC_MARGIN× the
/// runner-up's occurrences; a thinner margin is AMBIGUOUS. Without a qualifying id, a single
/// mentioned network (Quillon's `mainnet-genesis`) is reported by mention; several mentions
/// with no wire topics just mean "not built for one network" (fluxc, a wallet page) — no verdict.
pub fn network_verdict(topics: &[(String, Vec<String>)], hits: &[(String, usize)], mentions: &[(String, usize)]) -> (Option<String>, String, bool) {
    let distinct = |id: &str| topics.iter().find(|(k, _)| k == id).map(|(_, v)| v.len()).unwrap_or(0);
    let mut ranked: Vec<&(String, usize)> = hits.iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    if let Some((best, best_hits)) = ranked.first().map(|r| (&r.0, r.1)) {
        if distinct(best) >= MIN_TOPICS {
            let runner = ranked.get(1).map(|r| r.1).unwrap_or(0);
            if best_hits >= TOPIC_MARGIN * runner.max(1) || runner == 0 {
                return (Some(format!("sigil-{best}")), "wire-topics".into(), false);
            }
            return (None, "wire-topics".into(), true);
        }
    }
    match mentions {
        [] => (None, "none".into(), false),
        [(only, _)] => (Some(only.clone()), "mention".into(), false),
        _ => (None, "none".into(), false),
    }
}

/// `<product>/<major>.<minor>.<patch>` tokens, e.g. `sigil-top/10.7.3`.
pub fn version_tokens(run: &str) -> Vec<String> {
    let mut out = Vec::new();
    for tok in run.split(|c: char| !(c.is_ascii_alphanumeric() || c == '/' || c == '.' || c == '-')) {
        let Some((name, ver)) = tok.split_once('/') else { continue };
        if name.is_empty() || !name.bytes().next().map(|c| c.is_ascii_lowercase()).unwrap_or(false) { continue; }
        if !name.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-') { continue; }
        let parts: Vec<&str> = ver.split('.').collect();
        if parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.len() <= 5 && p.bytes().all(|c| c.is_ascii_digit())) {
            out.push(format!("{name}/{ver}"));
        }
    }
    out
}

/// `systemctl show -p Id,ActiveState,MainPID,ActiveEnterTimestamp,ExecStart` → facts.
pub fn parse_systemctl_show(text: &str) -> UnitFacts {
    let mut uf = UnitFacts::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        match k.trim() {
            "Id" => uf.id = v.trim().to_string(),
            "ActiveState" => uf.active_state = v.trim().to_string(),
            "MainPID" => uf.main_pid = v.trim().parse().unwrap_or(0),
            "ActiveEnterTimestamp" => uf.active_enter = v.trim().to_string(),
            "ExecStart" => uf.exec_start_path = parse_exec_start_path(v),
            _ => {}
        }
    }
    uf
}

/// `{ path=/opt/x/bin ; argv[]=… }` → `/opt/x/bin`.
pub fn parse_exec_start_path(v: &str) -> Option<String> {
    let i = v.find("path=")? + 5;
    let rest = &v[i..];
    let end = rest.find(" ;").unwrap_or(rest.len());
    let p = rest[..end].trim();
    if p.is_empty() { None } else { Some(p.to_string()) }
}

/// The manifest's artifact for `triple`: `targets.<triple>.{url,blake3_hex,size_bytes}`,
/// falling back to the top-level fields (older manifests).
pub fn pick_manifest_target(m: &Value, triple: &str) -> Option<(String, Option<String>, Option<u64>)> {
    let t = m.get("targets").and_then(|t| t.get(triple)).or_else(|| m.get("builds").and_then(|t| t.get(triple)));
    let node = t.unwrap_or(m);
    let url = node.get("url").and_then(|v| v.as_str())?.to_string();
    let b3 = node.get("blake3_hex").or_else(|| node.get("blake3")).and_then(|v| v.as_str()).map(str::to_string);
    let size = node.get("size_bytes").or_else(|| node.get("size")).and_then(|v| v.as_u64());
    Some((url, b3, size))
}

fn split_deleted(e: &Path) -> (PathBuf, bool) {
    let s = e.to_string_lossy();
    match s.strip_suffix(" (deleted)") {
        Some(c) => (PathBuf::from(c), true),
        None => (e.to_path_buf(), false),
    }
}

/// Process start as unix seconds: /proc/stat btime + /proc/<pid>/stat field 22 / CLK_TCK.
pub fn process_start_unix(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let btime: u64 = stat.lines().find_map(|l| l.strip_prefix("btime "))?.trim().parse().ok()?;
    let ps = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after = &ps[ps.rfind(')')? + 1..];
    let fields: Vec<&str> = after.split_whitespace().collect();
    // field 22 (starttime) overall; after the ')' the state is field 3 → index 19.
    let ticks: u64 = fields.get(19)?.parse().ok()?;
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    let hz = if hz <= 0 { 100 } else { hz as u64 };
    Some(btime + ticks / hz)
}

fn cache_dir() -> PathBuf {
    if let Ok(d) = std::env::var("FLUX_CACHE_DIR") { return PathBuf::from(d); }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    PathBuf::from(home).join(".flux").join("cache")
}

/// GET via curl (present on every box here; no HTTP client dep in fluxc-core).
fn curl(url: &str, to: Option<&Path>, max_secs: u64) -> Result<Vec<u8>, String> {
    let mut c = Command::new("curl");
    c.args(["-fsSL", "--max-time", &max_secs.to_string(), url]);
    if let Some(p) = to { c.arg("-o").arg(p); }
    let o = c.output().map_err(|e| format!("curl: {e}"))?;
    if !o.status.success() {
        return Err(format!("HTTP fetch failed ({}): {}", o.status, String::from_utf8_lossy(&o.stderr).trim()));
    }
    Ok(o.stdout)
}

// ───────────────────────────── render ─────────────────────────────

pub fn to_json(d: &Dust) -> Value {
    serde_json::to_value(d).unwrap_or_else(|_| json!({"error": "serialize"}))
}

fn fmt_unix(t: u64) -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|x| x.as_secs()).unwrap_or(0);
    let ago = now.saturating_sub(t);
    if ago < 3600 { format!("{t} ({}m ago)", ago / 60) }
    else if ago < 86400 { format!("{t} ({}h ago)", ago / 3600) }
    else { format!("{t} ({}d ago)", ago / 86400) }
}

pub fn render(d: &Dust) -> String {
    let mut s = String::new();
    let head = match (&d.network, d.network_ambiguous) {
        (Some(n), _) => format!("✦ stardust  {}  →  network {n}  (by {})", d.target, d.network_basis),
        (None, true) => format!("✦ stardust  {}  →  network AMBIGUOUS", d.target),
        (None, false) => format!("✦ stardust  {}  →  not built for one network (no wire-topic verdict)", d.target),
    };
    s.push_str(&head); s.push('\n');
    if let Some(u) = &d.unit {
        s.push_str(&format!("  unit        {}  {}  MainPID {}  since {}\n", u.id, u.active_state, u.main_pid, u.active_enter));
        if let Some(es) = &u.exec_start_path { s.push_str(&format!("  ExecStart   {es}\n")); }
        if let Some(re) = &u.running_exe {
            let tag = match u.exe_matches_exec_start { Some(true) => "= ExecStart ✓", Some(false) => "≠ ExecStart ✗", None => "" };
            s.push_str(&format!("  /proc exe   {re}  {tag}\n"));
        }
        if let Some(ps) = u.process_start_unix { s.push_str(&format!("  proc start  {}\n", fmt_unix(ps))); }
        if u.file_newer_than_process == Some(true) { s.push_str("  ⚠ file newer than process — built, not restarted\n"); }
    }
    if let Some(f) = &d.file { s.push_str(&format!("  file        {f}\n")); }
    if let (Some(sz), Some(mt)) = (d.size_bytes, d.mtime_unix) { s.push_str(&format!("  size/mtime  {sz} B · {}\n", fmt_unix(mt))); }
    if let Some(h) = &d.blake3 { s.push_str(&format!("  blake3      {h}\n")); }
    if let Some(h) = &d.sha256 { s.push_str(&format!("  sha256      {h}\n")); }
    if !d.wire_topics.is_empty() {
        let list: Vec<String> = d.wire_topics.iter().map(|(n, t)| {
            let h = d.topic_hits.iter().find(|(k, _)| k == n).map(|(_, c)| *c).unwrap_or(0);
            format!("/sigil/{n}/ {} topics ×{h}", t.len())
        }).collect();
        s.push_str(&format!("  wire topics {}   (compile-time gossip topics = the network it tunes to; ≥{MIN_TOPICS} distinct, ≥{TOPIC_MARGIN}× runner-up)\n", list.join("  ")));
    }
    if !d.mentions.is_empty() {
        let list: Vec<String> = d.mentions.iter().map(|(n, c)| format!("{n}×{c}")).collect();
        s.push_str(&format!("  mentions    {}   (literal counts — information only, NOT the verdict)\n", list.join("  ")));
    }
    if let Some(e) = &d.network_exec { s.push_str(&format!("  network-id  {e}   (executed, --exec)\n")); }
    if let Some(r) = &d.flux_rev { s.push_str(&format!("  flux-rev    {r}\n")); }
    if !d.versions.is_empty() { s.push_str(&format!("  versions    {}\n", d.versions.join("  "))); }
    if let Some(r) = &d.rustc { s.push_str(&format!("  rustc       {r}\n")); }
    if let Some(m) = &d.manifest {
        s.push_str(&format!("  manifest    {} {}  target {}  {}\n",
            m.product.clone().unwrap_or_default(), m.version.clone().unwrap_or_default(), m.target,
            if m.from_cache { "(artifact from cache)" } else { "(artifact downloaded)" }));
        if let Some(a) = &m.artifact_url { s.push_str(&format!("  artifact    {a}\n")); }
        s.push_str(&format!("  blake3 ok   {}   size ok {}   manifest flux_rev {}\n",
            m.blake3_ok.map(|b| if b { "✓" } else { "✗" }).unwrap_or("?"),
            m.size_ok.map(|b| if b { "✓" } else { "✗" }).unwrap_or("?"),
            m.flux_rev_manifest.clone().unwrap_or_else(|| "-".into())));
    }
    for w in &d.warnings { s.push_str(&format!("  ⚠ {w}\n")); }
    s.push_str(&format!("  {} · {} ms · read from the bytes, nothing executed; `--exec` runs `network-id`\n",
        if d.ok { "ok" } else { "INCOMPLETE" }, d.elapsed_ms));
    s
}

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printable_runs_split_on_binary_bytes() {
        let bytes = b"\x00\x01rustc version 1.93.1 (01f6ddf75 2026-02-11)\x00ab\x00sigil-g3\xff";
        let runs = printable_runs(bytes, MIN_RUN);
        assert_eq!(runs, vec!["rustc version 1.93.1 (01f6ddf75 2026-02-11)", "sigil-g3"]);
    }

    #[test]
    fn scan_finds_rustc_fluxrev_versions_mentions_and_topics() {
        // The shape of the REAL g3 sigil-top 10.7.5 (measured 2026-10-28): it MENTIONS sigil-g2
        // 66 times and sigil-g3 only 4, yet its wire topics are all /sigil/g3/*. The verdict
        // must follow the topics, never the counts.
        let mut bytes = Vec::new();
        for _ in 0..66 { bytes.extend_from_slice(b"\x00sigil-g2/chain\x00"); }
        for _ in 0..4 { bytes.extend_from_slice(b"\x00sigil-g3\x00"); }
        bytes.extend_from_slice(b"\x00sigil-g3-db\x00sigil-rpcd\x00");                 // boundary: not ids
        bytes.extend_from_slice(b"\x00/sigil/g3/blocks\x00/sigil/g3/txstokio\x00/sigil/g3/release\x00/sigil/g3/peer-hblake\x00");
        bytes.extend_from_slice(b"\x00/sigil/g0/blocks gossipsub topic\x00/sigil/g0/balance\x00");      // legacy wallet-page JS
        bytes.extend_from_slice(b"\x00rustc version 1.93.1 (01f6ddf75 2026-02-11)\x00");
        bytes.extend_from_slice(b"\x00full:51404b79aa00bb11cc22dd33ee44ff55\x00");
        bytes.extend_from_slice(b"\x00sigil-top/10.7.3 kgauge\x00mainnet-genesis\x00");
        let s = scan_bytes(&bytes);
        assert_eq!(s.rustc.as_deref(), Some("rustc version 1.93.1 (01f6ddf75 2026-02-11)"));
        assert_eq!(s.flux_rev.as_deref(), Some("full:51404b79aa00bb11cc22dd33ee44ff55"));
        assert_eq!(s.versions, vec!["sigil-top/10.7.3".to_string()]);
        assert_eq!(s.mentions[0], ("sigil-g2".to_string(), 66));
        assert!(s.mentions.iter().any(|(n, c)| n == "mainnet-genesis" && *c == 1));
        assert!(!s.mentions.iter().any(|(n, _)| n == "sigil-rp" || n == "sigil-g3-db"));
        assert_eq!(s.wire_topics[0].0, "g3");
        assert_eq!(s.wire_topics[0].1, vec!["blocks", "release", "txs"]);   // peer-hblake is not a known topic
        assert_eq!(s.wire_topics[1], ("g0".to_string(), vec!["blocks".to_string()]));
        assert_eq!(s.topic_hits, vec![("g3".to_string(), 3), ("g0".to_string(), 1)]);
        let (net, basis, amb) = network_verdict(&s.wire_topics, &s.topic_hits, &s.mentions);
        assert_eq!(net.as_deref(), Some("sigil-g3"));
        assert_eq!(basis, "wire-topics");
        assert!(!amb);
    }

    #[test]
    fn mentions_are_letter_digit_ids_only() {
        let mut n = Default::default();
        count_networks("sigil-tx sigil-vm sigil-pq sigil-g3 sigil-r1 sigil-m1 sigil-33 mainnet-genesis", &mut n);
        let keys: Vec<&String> = n.keys().collect();
        assert_eq!(keys, vec!["mainnet-genesis", "sigil-g3", "sigil-m1", "sigil-r1"]);
    }

    #[test]
    fn network_verdict_rules_measured_shapes() {
        let g = |s: &str, t: &[&str]| (s.to_string(), t.iter().map(|x| x.to_string()).collect::<Vec<_>>());
        let h = |p: &[(&str, usize)]| p.iter().map(|(k, v)| (k.to_string(), *v)).collect::<Vec<_>>();
        let six = ["blocks", "peer-heights", "tip-proofs", "txs", "release", "finality-votes"];
        let five = &six[..5];
        // The g3 build 10.7.5 as measured: g3×21 (6 topics), g2×5 (5 topics, a hardcoded self-check), g0×3 (1 topic).
        let (n, b, a) = network_verdict(&[g("g3", &six), g("g2", five), g("g0", &["blocks"])], &h(&[("g3", 21), ("g2", 5), ("g0", 3)]), &[]);
        assert_eq!((n.as_deref(), b.as_str(), a), (Some("sigil-g3"), "wire-topics", false));
        // The g2 build 10.7.4 as measured: g2×25, g0×3, no g3 at all.
        let (n, b, a) = network_verdict(&[g("g2", &six), g("g0", &["blocks"])], &h(&[("g2", 25), ("g0", 3)]), &[]);
        assert_eq!((n.as_deref(), b.as_str(), a), (Some("sigil-g2"), "wire-topics", false));
        // The g3 sigil-node: g3×6, g0×1.
        let (n, _, a) = network_verdict(&[g("g3", &six), g("g0", &["blocks"])], &h(&[("g3", 6), ("g0", 1)]), &[]);
        assert_eq!((n.as_deref(), a), (Some("sigil-g3"), false));
        // A single id with no runner-up wins outright.
        let (n, _, a) = network_verdict(&[g("r1", &six)], &h(&[("r1", 6)]), &[]);
        assert_eq!((n.as_deref(), a), (Some("sigil-r1"), false));
        // Too thin a margin (21 vs 15) with both qualifying = AMBIGUOUS, never a guess.
        let (n, b, a) = network_verdict(&[g("g2", &six), g("g3", &six)], &h(&[("g2", 21), ("g3", 15)]), &[]);
        assert_eq!((n, b.as_str(), a), (None, "wire-topics", true));
        // Fewer than MIN_TOPICS distinct → no topic verdict; one mentioned network = by mention (Quillon).
        let (n, b, a) = network_verdict(&[g("g0", &["blocks"])], &h(&[("g0", 3)]), &[("mainnet-genesis".to_string(), 7)]);
        assert_eq!((n.as_deref(), b.as_str(), a), (Some("mainnet-genesis"), "mention", false));
        // fluxc as measured: one g3 topic, several mentions → not built for one network, no verdict, not "ambiguous".
        let (n, b, a) = network_verdict(&[g("g3", &["blocks"])], &h(&[("g3", 1)]), &[("mainnet-genesis".to_string(), 6), ("sigil-g2".to_string(), 5), ("sigil-g3".to_string(), 1)]);
        assert_eq!((n, b.as_str(), a), (None, "none", false));
        assert_eq!(network_verdict(&[], &[], &[]), (None, "none".to_string(), false));
    }

    #[test]
    fn wire_topics_need_the_exact_shape() {
        let mut t = Default::default();
        let mut h = Default::default();
        collect_wire_topics("/sigil/g3/finality-votesopening /sigil/g3/court /sigil/gx3/blocks /sigil/g2/ /sigil/g3/finality-votes", &mut t, &mut h);
        assert_eq!(t.get("g3").map(|s| s.iter().cloned().collect::<Vec<_>>()), Some(vec!["finality-votes".to_string()]));
        assert_eq!(h.get("g3"), Some(&2));
        assert!(t.get("g2").is_none());
    }

    #[test]
    fn exec_start_path_from_systemctl_show() {
        let v = "{ path=/opt/orobit/shared/q-narwhalknight/q-api-server-v10.11.114f ; argv[]=/opt/x --port 8080 ; ignore_errors=no }";
        assert_eq!(parse_exec_start_path(v).as_deref(), Some("/opt/orobit/shared/q-narwhalknight/q-api-server-v10.11.114f"));
        assert_eq!(parse_exec_start_path("garbage"), None);
        let text = "MainPID=911356\nExecStart={ path=/home/storage/sigil-g3-bin/sigil-top ; argv[]=/home/storage/sigil-g3-bin/sigil-top ; ignore_errors=no }\nId=sigil-node.service\nActiveState=active\nActiveEnterTimestamp=Fri 2026-10-23 12:38:40 CEST\n";
        let uf = parse_systemctl_show(text);
        assert_eq!(uf.id, "sigil-node.service");
        assert_eq!(uf.main_pid, 911356);
        assert_eq!(uf.active_state, "active");
        assert_eq!(uf.exec_start_path.as_deref(), Some("/home/storage/sigil-g3-bin/sigil-top"));
        assert_eq!(uf.active_enter, "Fri 2026-10-23 12:38:40 CEST");
    }

    #[test]
    fn manifest_target_pick_prefers_targets_then_top_level() {
        let m = json!({
            "product": "sigil-top", "version": "10.7.3",
            "url": "https://x/top-level", "blake3_hex": "aa", "size_bytes": 1,
            "targets": {"linux-x64": {"url": "https://x/linux", "blake3_hex": "bb", "size_bytes": 2}}
        });
        assert_eq!(pick_manifest_target(&m, "linux-x64"), Some(("https://x/linux".into(), Some("bb".into()), Some(2))));
        assert_eq!(pick_manifest_target(&m, "windows-x64"), Some(("https://x/top-level".into(), Some("aa".into()), Some(1))));
        assert_eq!(pick_manifest_target(&json!({"product":"p"}), "linux-x64"), None);
    }

    #[test]
    fn version_tokens_are_strict() {
        assert_eq!(version_tokens("ua sigil-top/10.7.3 kgauge"), vec!["sigil-top/10.7.3"]);
        assert!(version_tokens("http://a/b/c").is_empty());
        assert!(version_tokens("sigil-top/10.7").is_empty());
        assert!(version_tokens("X/1.2.3").is_empty());
    }

    #[test]
    fn deleted_suffix_is_split() {
        let (p, del) = split_deleted(Path::new("/opt/x/bin (deleted)"));
        assert_eq!(p, PathBuf::from("/opt/x/bin"));
        assert!(del);
        let (p2, del2) = split_deleted(Path::new("/opt/x/bin"));
        assert_eq!(p2, PathBuf::from("/opt/x/bin"));
        assert!(!del2);
    }

    #[test]
    fn hashes_are_hex_of_the_right_length() {
        let (b3, s2) = hashes(b"hello");
        assert_eq!(b3.len(), 64);
        assert_eq!(s2, "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    }

    #[test]
    fn read_dispatches_on_target_shape_without_touching_the_network() {
        // A path that exists → kind path; our own test binary is a Rust binary with a rustc string.
        let exe = std::env::current_exe().unwrap();
        let d = read(&exe.to_string_lossy(), &Opts::default());
        assert_eq!(d.kind, "path");
        assert!(d.ok);
        assert!(d.rustc.is_some(), "a Rust test binary carries its rustc version string");
        assert!(d.blake3.is_some() && d.sha256.is_some());
        // A pid → kind pid (our own).
        let me = read(&std::process::id().to_string(), &Opts::default());
        assert_eq!(me.kind, "pid");
        assert_eq!(me.unit.as_ref().map(|u| u.main_pid), Some(std::process::id()));
        assert_eq!(me.unit.as_ref().and_then(|u| u.file_newer_than_process), Some(false));
    }
}
