//! stardust — the trail a build leaves inside a binary, read back WITHOUT running it, and (since
//! v2) written there on purpose by the rustc wrapper.
//!
//! Why this exists (all measured on Epsilon, October 2026):
//! * 2026-10-26: sigil-top 10.7.4 was published as a **g2** build and auto-updated a g3 server
//!   onto the dead chain. Nothing read the network id back out of the artifact before it went
//!   into the signed manifest.
//! * 2026-10-10: `systemctl cat` showed the base `ExecStart=` while a drop-in ran a different
//!   binary; a 7-minute release build went to the wrong target dir.
//! * `sigil-miner --help` starts mining and writes a wallet file: a binary must never be
//!   executed just to find out what it is.
//!
//! THREE KINDS OF ANSWER, kept apart (review of 2026-10-28, point 1):
//! * DECLARED — the writer's stamp: what the build was INVOKED with (`SIGIL_NETWORK=…`), the git
//!   head and dirtiness, opt level, target, fluxc version. It is metadata the wrapper wrote at
//!   the time of the LAST LINK of this file; it proves the environment, not the behaviour.
//! * EVIDENCE — what the bytes show: sigil-net's compile-time gossip wire topics
//!   (`/sigil/<short>/blocks`, `/txs`, …). A HEURISTIC: it depends on the networking crate
//!   building its topic names with `concat!` from the network id.
//! * The VERDICT — only when declared and evidence AGREE, or when exactly one of them exists and
//!   says so plainly. A disagreement is reported as a CONFLICT with `ok = false` and no network,
//!   so a release check that requires a network FAILS on it.
//!
//! Why NOT count the network-id literals: tried first and refuted the same hour. The g2 build
//! 10.7.4 and the g3 build 10.7.5 have practically identical counts (66 vs 67 `sigil-g2`, 4 vs 6
//! `sigil-g3`) because the linker merges identical literals. Counts measure how many distinct
//! strings MENTION a network. Mentions are still reported, labelled information.
//!
//! THE WRITER. fluxc is the RUSTC_WRAPPER for every build here, so it sees the final link of
//! every binary. For a unit that links an ELF executable (bin or `--test`, Linux target,
//! gcc-flavour linker) it writes a tiny relocatable object carrying ONE allocated note section,
//! `.note.stardust` (owner "STARDUST", type 1, body = the JSON stamp), next to the unit's output
//! dir and passes `-C link-arg=<that .o>`. The linker keeps the section (measured with rustc's
//! default `--gc-sections` under mold, GNU ld and a musl static-pie link) and the GNU build-id
//! is left UNTOUCHED — v1 of the writer replaced the build-id with the stamp; the review pointed
//! out that the build-id identifies the linked FILE and two different binaries could then share
//! one id, so v2 stopped doing that. Legacy v1 stamps (JSON in the build-id) are still read.
//!
//! FRESHNESS CAVEAT (review point 2): the wrapper only runs when cargo decides to relink. For
//! SIGIL the network variable is cargo-tracked (`cargo:rerun-if-env-changed=SIGIL_NETWORK` in
//! sigil-header's build.rs), so a changed network always relinks and the stamp follows; the git
//! head and dirtiness are facts "as of the last link" and may lag a tree that changed without
//! triggering a relink. The stamp carries no timestamp, so links stay deterministic.

use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// sigil-net's compile-time gossip topics (`concat!("/sigil/", SIGIL_NET_SHORT, "/…")`).
pub const WIRE_TOPICS: &[&str] = &["blocks", "peer-heights", "tip-proofs", "txs", "release", "finality-votes"];
/// A network is called by evidence only with at least this many DISTINCT wire topics present…
pub const MIN_TOPICS: usize = 3;
/// …and at least this many times the runner-up's topic occurrences.
pub const TOPIC_MARGIN: usize = 2;
/// Shortest printable run worth looking at.
pub const MIN_RUN: usize = 6;
/// Refuse to download artifacts above this (a sigil-top is ~47 MB).
pub const MAX_ARTIFACT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Env names the writer records into the stamp (comma list; `FLUX_STARDUST_ENV` overrides).
pub const STAMP_ENV_DEFAULT: &str = "SIGIL_NETWORK,SIGIL_NET_ID,Q_NETWORK";
/// The stamp schema this writer emits.
pub const STAMP_SCHEMA: u64 = 2;
/// The stardust note: owner name (NUL-terminated as in the file) and type.
pub const NOTE_OWNER_STARDUST: &[u8] = b"STARDUST\0";
pub const NT_STARDUST: u32 = 1;
/// The GNU build-id note (left untouched by the v2 writer; carried the v1 stamp).
const NOTE_OWNER_GNU: &[u8] = b"GNU\0";
const NT_GNU_BUILD_ID: u32 = 3;

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
    /// The GNU build-id (hex) — the linker's identity of the linked file, untouched by the v2 writer.
    pub build_id: Option<String>,
    pub rustc: Option<String>,
    pub flux_rev: Option<String>,
    pub versions: Vec<String>,
    /// Every network id MENTIONED, with its count, highest first — information, not a verdict.
    pub mentions: Vec<(String, usize)>,
    /// Compile-time wire topics per short network id (`g3` → ["blocks", "txs", …]).
    pub wire_topics: Vec<(String, Vec<String>)>,
    /// Occurrences of known wire topics per short id (`g3` → 21).
    pub topic_hits: Vec<(String, usize)>,
    /// The writer's stamp (DECLARED metadata) and where it was found.
    pub stamp: Option<Value>,
    /// "note.stardust" | "build-id (legacy v1)"
    pub stamp_source: Option<String>,
    /// DECLARED: `env.SIGIL_NETWORK` from the stamp.
    pub declared_network: Option<String>,
    /// EVIDENCE: the wire-topic (or single-mention) reading of the bytes.
    pub evidence_network: Option<String>,
    /// "wire-topics" | "mention" | "none"
    pub evidence_basis: String,
    /// Declared and evidence disagree. `ok` is false when this is set.
    pub conflict: bool,
    /// The verdict — only when declared and evidence agree, or exactly one speaks.
    pub network: Option<String>,
    /// "stamp+evidence agree" | "declared-only" | "wire-topics (heuristic)" | "mention (heuristic)" | "conflict" | "none"
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
    d.ok = d.ok && d.file.is_some();
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
        if let Some(p) = uf.exec_start_path.clone() {
            fill_file(&mut d, Path::new(&p), opts);
        }
        d.unit = Some(uf);
        d.ok = d.ok && d.file.is_some();
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
    d.ok = d.ok && d.file.is_some();
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
    d.ok = d.ok && d.file.is_some() && mf.blake3_ok != Some(false) && mf.size_ok != Some(false);
    d.manifest = Some(mf);
    d
}

// ───────────────────────────── file ─────────────────────────────

fn fill_file(d: &mut Dust, p: &Path, opts: &Opts) {
    let canon = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    d.file = Some(canon.to_string_lossy().into_owned());
    let bytes = match std::fs::read(&canon) {
        Ok(b) => b,
        Err(e) => { d.warnings.push(format!("read {}: {e}", canon.display())); d.ok = false; return; }
    };
    if let Ok(md) = std::fs::metadata(&canon) {
        d.size_bytes = Some(md.len());
        d.mtime_unix = md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|x| x.as_secs());
    }
    let (b3, s2) = hashes(&bytes);
    d.blake3 = Some(b3);
    d.sha256 = Some(s2);

    // EVIDENCE from the bytes — with the writer's own stamp text blanked out first, or the stamp's
    // `"SIGIL_NETWORK":"sigil-r3"` would count as a MENTION of its own network and "agree" with
    // itself (measured 2026-10-28 on a hello binary: declared-only read as stamp+evidence agree).
    let mut masked: Option<Vec<u8>> = None;
    for r in [elf_note_range(&bytes, NOTE_OWNER_STARDUST, NT_STARDUST), elf_note_range(&bytes, NOTE_OWNER_GNU, NT_GNU_BUILD_ID)].into_iter().flatten() {
        let m = masked.get_or_insert_with(|| bytes.clone());
        for b in &mut m[r] { *b = 0; }
    }
    let scan = scan_bytes(masked.as_deref().unwrap_or(&bytes));
    d.rustc = scan.rustc;
    d.flux_rev = scan.flux_rev;
    d.versions = scan.versions;
    let (ev_net, ev_basis, amb) = network_verdict(&scan.wire_topics, &scan.topic_hits, &scan.mentions);
    d.mentions = scan.mentions;
    d.wire_topics = scan.wire_topics;
    d.topic_hits = scan.topic_hits;
    d.evidence_network = ev_net.clone();
    d.evidence_basis = ev_basis.clone();
    d.network_ambiguous = amb;
    if amb {
        d.warnings.push(format!("evidence ambiguous — wire-topic occurrences {:?} too close; use --exec for `network-id`", d.topic_hits));
    }

    // DECLARED from the stamp: .note.stardust first, then a legacy v1 stamp in the build-id.
    let gnu = elf_note(&bytes, NOTE_OWNER_GNU, NT_GNU_BUILD_ID);
    if let Some(desc) = elf_note(&bytes, NOTE_OWNER_STARDUST, NT_STARDUST).and_then(|x| decode_stamp(&x).map(|s| (s, "note.stardust"))) {
        d.stamp = Some(desc.0); d.stamp_source = Some(desc.1.into());
    } else if let Some(s) = gnu.as_ref().and_then(|g| decode_stamp(g)) {
        d.stamp = Some(s); d.stamp_source = Some("build-id (legacy v1)".into());
        d.warnings.push("stamp lives in the GNU build-id (writer v1) — this binary has no linker file identity of its own".into());
    }
    if let Some(g) = &gnu { if decode_stamp(g).is_none() { d.build_id = Some(hex::encode(g)); } }
    if let Some(st) = &d.stamp {
        match st.get("env").and_then(|e| e.get("SIGIL_NETWORK")) {
            Some(Value::String(n)) => d.declared_network = Some(n.clone()),
            Some(Value::Null) if st.get("pkg").and_then(|v| v.as_str()).map(|p| p.starts_with("sigil")).unwrap_or(false) => {
                d.warnings.push("stamp: built with SIGIL_NETWORK UNSET — sigil-header's build default (sigil-g2) applied".into());
            }
            _ => {}
        }
    }

    // THE VERDICT: agreement, or one voice; a disagreement is a conflict and fails the read.
    let (net, basis, conflict) = resolve_network(d.declared_network.as_deref(), ev_net.as_deref(), &ev_basis);
    d.network = net;
    d.network_basis = basis;
    d.conflict = conflict;
    if conflict {
        d.warnings.push(format!("CONFLICT: the stamp declares {} but the bytes' wire topics say {} — no network reported; find out which build input lied before trusting this file",
            d.declared_network.clone().unwrap_or_default(), d.evidence_network.clone().unwrap_or_default()));
    }
    if d.rustc.is_none() {
        d.warnings.push("no `rustc version` string — not a Rust binary, or stripped".into());
    }
    if opts.exec {
        match exec_network_id(&canon) {
            Ok(Some(line)) => {
                let net = line.split_whitespace().find_map(|t| t.strip_prefix("net=")).unwrap_or(line.as_str()).to_string();
                if let Some(n) = &d.network {
                    if &net != n { d.warnings.push(format!("`network-id` says {net} but the verdict was {n} — trust network-id")); }
                }
                d.network_exec = Some(line);
            }
            Ok(None) => d.warnings.push("`network-id` printed nothing within 5 s".into()),
            Err(e) => d.warnings.push(format!("--exec: {e}")),
        }
    }
    d.ok = !conflict;
}

/// The verdict rule, pure. Returns (network, basis, conflict).
pub fn resolve_network(declared: Option<&str>, evidence: Option<&str>, evidence_basis: &str) -> (Option<String>, String, bool) {
    match (declared, evidence) {
        (Some(d), Some(e)) if d == e => (Some(d.to_string()), "stamp+evidence agree".into(), false),
        (Some(_), Some(_)) => (None, "conflict".into(), true),
        (Some(d), None) => (Some(d.to_string()), "declared-only (stamp; no network code found in the bytes)".into(), false),
        (None, Some(e)) => (Some(e.to_string()), format!("{evidence_basis} (heuristic)"), false),
        (None, None) => (None, "none".into(), false),
    }
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

/// The EVIDENCE verdict: (network, basis, ambiguous). Wire topics decide: the id with the most
/// occurrences wins when it has ≥ MIN_TOPICS distinct topics and ≥ TOPIC_MARGIN× the runner-up's
/// occurrences; a thinner margin is AMBIGUOUS. Without a qualifying id, a single mentioned
/// network (Quillon's `mainnet-genesis`) is reported by mention; several mentions with no wire
/// topics mean "not built for one network" — no evidence verdict.
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
    let ticks: u64 = fields.get(19)?.parse().ok()?;
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    let hz = if hz <= 0 { 100 } else { hz as u64 };
    Some(btime + ticks / hz)
}

pub fn cache_dir() -> PathBuf {
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

// ───────────────────────────── the writer ─────────────────────────────

/// Does this rustc invocation link an ELF executable we can stamp? Pure. A bin or `--test`
/// unit whose `--emit` includes link (or is unset), targeting Linux (host or `*-linux-*`),
/// with a compiler-driver linker flavour (the extra object is passed as a plain positional
/// argument, which every driver and direct linker accepts, but we still skip direct `ld`/`lld`
/// flavours and non-ELF targets because they were not tested).
pub fn links_elf_executable(args: &[String]) -> bool {
    let (mut is_bin, mut is_test, mut emit_link, mut flavor_ok) = (false, false, true, true);
    let mut target: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let next = args.get(i + 1).map(|s| s.as_str()).unwrap_or("");
        let has_bin = |v: &str| v.split(',').any(|t| t == "bin");
        if a == "--crate-type" { if has_bin(next) { is_bin = true; } i += 1; }
        else if let Some(v) = a.strip_prefix("--crate-type=") { if has_bin(v) { is_bin = true; } }
        else if a == "--test" { is_test = true; }
        else if a == "--emit" { emit_link = next.split(',').any(|e| e == "link"); i += 1; }
        else if let Some(v) = a.strip_prefix("--emit=") { emit_link = v.split(',').any(|e| e == "link"); }
        else if a == "--target" { target = Some(next.to_string()); i += 1; }
        else if let Some(v) = a.strip_prefix("--target=") { target = Some(v.to_string()); }
        else if a == "-C" || a.starts_with("-C") {
            let c = if a == "-C" { i += 1; next } else { &a[2..] };
            if let Some(v) = c.strip_prefix("linker=") {
                let base = v.rsplit('/').next().unwrap_or(v);
                flavor_ok = base.contains("cc") || base.contains("clang") || base.contains("gcc");
            } else if let Some(v) = c.strip_prefix("linker-flavor=") {
                // rustc flavours: gcc, gnu-cc, gnu-lld-cc (drivers) vs ld, ld.lld, msvc, em… (direct)
                flavor_ok = v.contains("gcc") || v.ends_with("-cc");
            }
        }
        i += 1;
    }
    let elf = match &target {
        None => cfg!(target_os = "linux"),
        Some(t) => t.contains("-linux-"),
    };
    (is_bin || is_test) && emit_link && elf && flavor_ok
}

/// Git facts of the repository containing `start`, read from `.git` directly (no subprocess for
/// HEAD; worktrees' `gitdir:` files, `commondir` and packed-refs handled). Dirtiness runs
/// `git status --porcelain --untracked-files=no` with a 2 s timeout (12 ms warm on the flux tree,
/// measured) unless FLUX_STARDUST_GIT_STATUS=0; `None` = unknown, never "clean".
pub fn git_head_full(start: &Path) -> Option<String> {
    let mut dir = Some(start);
    let dot = loop {
        let d = dir?;
        let cand = d.join(".git");
        if cand.exists() { break cand; }
        dir = d.parent();
    };
    let gitdir = if dot.is_dir() { dot.clone() } else {
        let txt = std::fs::read_to_string(&dot).ok()?;
        let p = txt.trim().strip_prefix("gitdir:")?.trim();
        let p = PathBuf::from(p);
        if p.is_absolute() { p } else { dot.parent()?.join(p) }
    };
    let head = std::fs::read_to_string(gitdir.join("HEAD")).ok()?;
    let head = head.trim();
    let sha = match head.strip_prefix("ref:") {
        None => head.to_string(),
        Some(r) => {
            let r = r.trim();
            let common = std::fs::read_to_string(gitdir.join("commondir")).ok()
                .map(|c| { let c = c.trim(); let p = PathBuf::from(c); if p.is_absolute() { p } else { gitdir.join(p) } })
                .unwrap_or_else(|| gitdir.clone());
            let loose = [gitdir.join(r), common.join(r)].iter().find_map(|p| std::fs::read_to_string(p).ok().map(|s| s.trim().to_string()));
            match loose {
                Some(x) => x,
                None => {
                    let packed = std::fs::read_to_string(common.join("packed-refs")).ok()?;
                    packed.lines().find_map(|l| { let mut it = l.split_whitespace(); let sha = it.next()?; let name = it.next()?; (name == r).then(|| sha.to_string()) })?
                }
            }
        }
    };
    if sha.len() >= 40 && sha.bytes().all(|c| c.is_ascii_hexdigit()) { Some(sha[..40].to_string()) } else { None }
}

/// Short form kept for callers that want 12 hex.
pub fn git_head_short(start: &Path) -> Option<String> { git_head_full(start).map(|s| s[..12].to_string()) }

/// `Some(true)` = tracked files modified, `Some(false)` = clean, `None` = could not tell.
pub fn git_dirty(start: &Path) -> Option<bool> {
    if matches!(std::env::var("FLUX_STARDUST_GIT_STATUS").ok().as_deref(), Some("0") | Some("off")) { return None; }
    let mut child = Command::new("git").args(["status", "--porcelain", "--untracked-files=no"])
        .current_dir(start).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).stdin(std::process::Stdio::null())
        .spawn().ok()?;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(st)) => { if !st.success() { return None; } break; }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => { let _ = child.kill(); let _ = child.wait(); return None; }
        }
    }
    let mut out = String::new();
    use std::io::Read;
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    Some(out.lines().any(|l| !l.trim().is_empty()))
}

/// What the wrapper knows at link time, as the stamp (schema 2). `env` is injected for tests.
pub fn build_stamp(args: &[String], env: &dyn Fn(&str) -> Option<String>) -> Value {
    let after = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    let mut opt = "0".to_string();
    let mut target = "host".to_string();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let next = args.get(i + 1).map(|s| s.as_str()).unwrap_or("");
        if a == "--target" { target = next.to_string(); i += 1; }
        else if let Some(v) = a.strip_prefix("--target=") { target = v.to_string(); }
        else if a == "-C" || a.starts_with("-C") {
            let c = if a == "-C" { i += 1; next } else { &a[2..] };
            if let Some(v) = c.strip_prefix("opt-level=") { opt = v.to_string(); }
        }
        i += 1;
    }
    let names = env("FLUX_STARDUST_ENV").filter(|s| !s.trim().is_empty()).unwrap_or_else(|| STAMP_ENV_DEFAULT.to_string());
    let mut envs = serde_json::Map::new();
    for n in names.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        envs.insert(n.to_string(), env(n).map(Value::String).unwrap_or(Value::Null));
    }
    let manifest_dir = env("CARGO_MANIFEST_DIR").map(PathBuf::from);
    let git_head = manifest_dir.as_ref().and_then(|d| git_head_full(d));
    let git_dirty = if git_head.is_some() { manifest_dir.as_ref().and_then(|d| git_dirty(d)) } else { None };
    let mut v = json!({
        "stardust": STAMP_SCHEMA,
        "crate": env("CARGO_CRATE_NAME").or_else(|| after("--crate-name")),
        "pkg": env("CARGO_PKG_NAME"),
        "version": env("CARGO_PKG_VERSION"),
        "bin": env("CARGO_BIN_NAME"),
        "test": args.iter().any(|a| a == "--test"),
        "env": Value::Object(envs),
        "git_head": git_head,
        "git_dirty": git_dirty,
        "opt": opt,
        "target": target,
        "fluxc": env!("CARGO_PKG_VERSION"),
    });
    // The builder's hostname is left out by default (review point 6): it is discoverable from DNS
    // anyway and has no place in a user-downloaded artifact. FLUX_STARDUST_HOST=1 records it.
    if env("FLUX_STARDUST_HOST").as_deref() == Some("1") {
        let host = std::fs::read_to_string("/etc/hostname").ok().map(|h| h.trim().to_string()).filter(|h| !h.is_empty()).or_else(|| env("HOSTNAME"));
        v["host"] = host.map(Value::String).unwrap_or(Value::Null);
    }
    v
}

fn pad4(mut v: Vec<u8>, with: u8) -> Vec<u8> { while v.len() % 4 != 0 { v.push(with); } v }

/// A minimal ELF64 little-endian RELOCATABLE object (x86-64 machine id; the linker does not
/// check it for a data-only object) carrying ONE allocated SHT_NOTE section `.note.stardust`
/// whose body is `stamp` (JSON, space-padded to 4 bytes). Three section headers: null,
/// `.note.stardust`, `.shstrtab`. No symbols, no relocations.
pub fn make_note_object(stamp: &Value) -> Vec<u8> {
    let desc = pad4(stamp.to_string().into_bytes(), b' ');
    let mut note = Vec::new();
    note.extend_from_slice(&(NOTE_OWNER_STARDUST.len() as u32).to_le_bytes());
    note.extend_from_slice(&(desc.len() as u32).to_le_bytes());
    note.extend_from_slice(&NT_STARDUST.to_le_bytes());
    note.extend_from_slice(NOTE_OWNER_STARDUST);
    let note = pad4(note, 0);
    let mut note = note;
    note.extend_from_slice(&desc);
    let strs = b"\0.note.stardust\0.shstrtab\0".to_vec();
    let note_off = 64usize;
    let strs_off = note_off + note.len();
    let shoff = (strs_off + strs.len() + 7) / 8 * 8;
    let mut f = vec![0u8; shoff + 3 * 64];
    f[0..4].copy_from_slice(b"\x7fELF"); f[4] = 2; f[5] = 1; f[6] = 1;
    f[16..18].copy_from_slice(&1u16.to_le_bytes());        // ET_REL
    f[18..20].copy_from_slice(&0x3eu16.to_le_bytes());     // EM_X86_64
    f[20..24].copy_from_slice(&1u32.to_le_bytes());
    f[0x28..0x30].copy_from_slice(&(shoff as u64).to_le_bytes());
    f[0x34..0x36].copy_from_slice(&64u16.to_le_bytes());   // e_ehsize
    f[0x3a..0x3c].copy_from_slice(&64u16.to_le_bytes());   // e_shentsize
    f[0x3c..0x3e].copy_from_slice(&3u16.to_le_bytes());    // e_shnum
    f[0x3e..0x40].copy_from_slice(&2u16.to_le_bytes());    // e_shstrndx
    f[note_off..note_off + note.len()].copy_from_slice(&note);
    f[strs_off..strs_off + strs.len()].copy_from_slice(&strs);
    let mut sh = |i: usize, name: u32, ty: u32, flags: u64, off: usize, size: usize, align: u64| {
        let b = shoff + i * 64;
        f[b..b + 4].copy_from_slice(&name.to_le_bytes());
        f[b + 4..b + 8].copy_from_slice(&ty.to_le_bytes());
        f[b + 8..b + 16].copy_from_slice(&flags.to_le_bytes());
        f[b + 0x18..b + 0x20].copy_from_slice(&(off as u64).to_le_bytes());
        f[b + 0x20..b + 0x28].copy_from_slice(&(size as u64).to_le_bytes());
        f[b + 0x30..b + 0x38].copy_from_slice(&align.to_le_bytes());
    };
    sh(1, 1, 7, 2, note_off, note.len(), 4);   // SHT_NOTE, SHF_ALLOC
    sh(2, 16, 3, 0, strs_off, strs.len(), 1);  // SHT_STRTAB
    f
}

/// Where the note object goes: `<--out-dir>/.stardust/<crate>-<blake3[..16]>.o` (the unit's own
/// output dir, so it lives in the target tree, never on the root partition).
pub fn stamp_object_path(args: &[String], obj: &[u8]) -> PathBuf {
    let out_dir = args.iter().position(|a| a == "--out-dir").and_then(|i| args.get(i + 1)).cloned()
        .or_else(|| args.iter().find_map(|a| a.strip_prefix("--out-dir=").map(str::to_string)))
        .or_else(|| std::env::var("CARGO_TARGET_DIR").ok())
        .unwrap_or_else(|| "target".into());
    let crate_name = args.iter().position(|a| a == "--crate-name").and_then(|i| args.get(i + 1)).cloned().unwrap_or_else(|| "unit".into());
    let h = blake3::hash(obj).to_hex();
    PathBuf::from(out_dir).join(".stardust").join(format!("{crate_name}-{}.o", &h[..16]))
}

/// The wrapper's one call: extra rustc args for this unit, or None when it does not link an
/// ELF executable, the writer is off (`FLUX_STARDUST=0`), or the object could not be written
/// (a stamp must never fail a build).
pub fn link_stamp_args(args: &[String]) -> Option<Vec<String>> {
    if matches!(std::env::var("FLUX_STARDUST").ok().as_deref().map(str::trim), Some("0") | Some("off") | Some("false")) {
        return None;
    }
    if !links_elf_executable(args) { return None; }
    let stamp = build_stamp(args, &|k| std::env::var(k).ok());
    let obj = make_note_object(&stamp);
    let path = stamp_object_path(args, &obj);
    if std::fs::read(&path).map(|cur| cur == obj).unwrap_or(false) {
        return Some(vec!["-C".into(), format!("link-arg={}", path.display())]);
    }
    std::fs::create_dir_all(path.parent()?).ok()?;
    let tmp = path.with_extension(format!("o.tmp-{}", std::process::id()));
    std::fs::write(&tmp, &obj).ok()?;
    std::fs::rename(&tmp, &path).ok()?;
    Some(vec!["-C".into(), format!("link-arg={}", path.display())])
}

// ───────────────────────────── the ELF reader ─────────────────────────────

fn u16le(b: &[u8], o: usize) -> Option<u16> { b.get(o..o.checked_add(2)?).map(|x| u16::from_le_bytes([x[0], x[1]])) }
fn u32le(b: &[u8], o: usize) -> Option<u32> { b.get(o..o.checked_add(4)?).map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]])) }
fn u64le(b: &[u8], o: usize) -> Option<u64> { b.get(o..o.checked_add(8)?).map(|x| u64::from_le_bytes([x[0], x[1], x[2], x[3], x[4], x[5], x[6], x[7]])) }
/// A bounded byte range inside `b`, or None (checked arithmetic — fuzzed inputs must never panic).
fn span(b: &[u8], off: u64, len: u64) -> Option<&[u8]> {
    let o = usize::try_from(off).ok()?;
    let l = usize::try_from(len).ok()?;
    b.get(o..o.checked_add(l)?)
}

/// The body of the first note with (owner, type) in an ELF64 little-endian file: section headers
/// (every SHT_NOTE section) first, then the PT_NOTE program headers (a binary with its section
/// table stripped). Every offset and length is bounds-checked; bad input returns None.
pub fn elf_note(bytes: &[u8], owner: &[u8], ty: u32) -> Option<Vec<u8>> {
    elf_note_range(bytes, owner, ty).map(|r| bytes[r].to_vec())
}

/// Where that note's body sits in the file (so a scan can blank it out).
pub fn elf_note_range(bytes: &[u8], owner: &[u8], ty: u32) -> Option<std::ops::Range<usize>> {
    if bytes.len() < 64 || &bytes[0..4] != b"\x7fELF" || bytes[4] != 2 || bytes[5] != 1 { return None; }
    // Section headers.
    let shoff = u64le(bytes, 0x28)?;
    let shentsize = u16le(bytes, 0x3a)? as u64;
    let shnum = u16le(bytes, 0x3c)? as u64;
    if shentsize >= 64 && shnum > 0 && shoff > 0 {
        for i in 0..shnum {
            let Some(h) = shoff.checked_add(i.checked_mul(shentsize)?).and_then(|o| span(bytes, o, 64)) else { break };
            if u32le(h, 4)? != 7 { continue; } // SHT_NOTE
            let off = u64le(h, 0x18)?;
            let size = u64le(h, 0x20)?;
            if let Some(sec) = span(bytes, off, size) {
                if let Some(r) = parse_note_range(sec, owner, ty) { let base = off as usize; return Some(base + r.start..base + r.end); }
            }
        }
    }
    // Program headers (PT_NOTE = 4).
    let phoff = u64le(bytes, 0x20)?;
    let phentsize = u16le(bytes, 0x36)? as u64;
    let phnum = u16le(bytes, 0x38)? as u64;
    if phentsize >= 56 && phnum > 0 && phoff > 0 {
        for i in 0..phnum {
            let Some(h) = phoff.checked_add(i.checked_mul(phentsize)?).and_then(|o| span(bytes, o, 56)) else { break };
            if u32le(h, 0)? != 4 { continue; }
            let off = u64le(h, 8)?;
            let size = u64le(h, 32)?;
            if let Some(sec) = span(bytes, off, size) {
                if let Some(r) = parse_note_range(sec, owner, ty) { let base = off as usize; return Some(base + r.start..base + r.end); }
            }
        }
    }
    None
}

/// The GNU build-id body, if any.
pub fn elf_build_id(bytes: &[u8]) -> Option<Vec<u8>> { elf_note(bytes, NOTE_OWNER_GNU, NT_GNU_BUILD_ID) }

/// Walk a note section for (owner, type) and return that note's body. Bounded.
pub fn parse_note_desc(sec: &[u8], owner: &[u8], ty: u32) -> Option<Vec<u8>> {
    parse_note_range(sec, owner, ty).map(|r| sec[r].to_vec())
}

/// Same walk, returning the body's range inside `sec`.
pub fn parse_note_range(sec: &[u8], owner: &[u8], ty: u32) -> Option<std::ops::Range<usize>> {
    let mut o: usize = 0;
    let mut guard = 0u32;
    while o.checked_add(12)? <= sec.len() && guard < 10_000 {
        guard += 1;
        let namesz = u32le(sec, o)? as usize;
        let descsz = u32le(sec, o + 4)? as usize;
        let t = u32le(sec, o + 8)?;
        let name_start = o + 12;
        let desc_start = name_start.checked_add(namesz.checked_add(3)? & !3)?;
        let desc_end = desc_start.checked_add(descsz)?;
        if desc_end > sec.len() { return None; }
        if t == ty && sec.get(name_start..name_start + namesz) == Some(owner) {
            return Some(desc_start..desc_end);
        }
        o = desc_start.checked_add(descsz.checked_add(3)? & !3)?;
    }
    None
}

/// A note body is a stardust stamp when it is UTF-8 JSON with `"stardust": <n>`.
pub fn decode_stamp(desc: &[u8]) -> Option<Value> {
    let txt = std::str::from_utf8(desc).ok()?.trim();
    if !txt.starts_with('{') { return None; }
    let v: Value = serde_json::from_str(txt).ok()?;
    v.get("stardust").and_then(|x| x.as_u64())?;
    Some(v)
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
    let head = if d.conflict {
        format!("✦ stardust  {}  →  CONFLICT: declared {} vs evidence {} — no network", d.target,
            d.declared_network.clone().unwrap_or_default(), d.evidence_network.clone().unwrap_or_default())
    } else {
        match (&d.network, d.network_ambiguous) {
            (Some(n), _) => format!("✦ stardust  {}  →  network {n}  ({})", d.target, d.network_basis),
            (None, true) => format!("✦ stardust  {}  →  evidence AMBIGUOUS, no stamp", d.target),
            (None, false) => format!("✦ stardust  {}  →  not built for one network (no stamp, no wire-topic verdict)", d.target),
        }
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
    if let Some(b) = &d.build_id { s.push_str(&format!("  build-id    {b}   (the linker's identity of this file, untouched)\n")); }
    if let Some(st) = &d.stamp {
        let g = |k: &str| st.get(k).map(|v| match v { Value::String(x) => x.clone(), Value::Null => "-".into(), o => o.to_string() }).unwrap_or_else(|| "-".into());
        let envs = st.get("env").and_then(|e| e.as_object()).map(|o| o.iter().map(|(k, v)| format!("{k}={}", match v { Value::String(x) => x.as_str(), _ => "unset" })).collect::<Vec<_>>().join(" ")).unwrap_or_default();
        let dirty = match st.get("git_dirty") { Some(Value::Bool(true)) => "+dirty", Some(Value::Bool(false)) => "clean", _ => "dirty?" };
        let ghead = g("git_head");
        let kind = if st.get("test").and_then(|v| v.as_bool()).unwrap_or(false) { "test-bin".to_string() } else { format!("bin {}", g("bin")) };
        s.push_str(&format!("  declared    {} {} {} · {} · git {} {} · opt {} · target {} · fluxc {}   (stamp v{} in {}; as of this file's LAST LINK)\n",
            g("pkg"), g("version"), kind,
            envs, if ghead.len() > 12 { &ghead[..12] } else { &ghead }, dirty, g("opt"), g("target"), g("fluxc"),
            st.get("stardust").and_then(|v| v.as_u64()).unwrap_or(0), d.stamp_source.clone().unwrap_or_default()));
    }
    if !d.wire_topics.is_empty() {
        let list: Vec<String> = d.wire_topics.iter().map(|(n, t)| {
            let h = d.topic_hits.iter().find(|(k, _)| k == n).map(|(_, c)| *c).unwrap_or(0);
            format!("/sigil/{n}/ {} topics ×{h}", t.len())
        }).collect();
        s.push_str(&format!("  evidence    {}   → {}   (HEURISTIC: compile-time gossip topics; ≥{MIN_TOPICS} distinct, ≥{TOPIC_MARGIN}× runner-up)\n",
            list.join("  "), d.evidence_network.clone().unwrap_or_else(|| "none".into())));
    }
    if !d.mentions.is_empty() {
        let list: Vec<String> = d.mentions.iter().map(|(n, c)| format!("{n}×{c}")).collect();
        s.push_str(&format!("  mentions    {}   (literal counts — information only)\n", list.join("  ")));
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
        if d.ok { "ok" } else { "NOT OK" }, d.elapsed_ms));
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
        let mut bytes = Vec::new();
        for _ in 0..66 { bytes.extend_from_slice(b"\x00sigil-g2/chain\x00"); }
        for _ in 0..4 { bytes.extend_from_slice(b"\x00sigil-g3\x00"); }
        bytes.extend_from_slice(b"\x00sigil-g3-db\x00sigil-rpcd\x00");
        bytes.extend_from_slice(b"\x00/sigil/g3/blocks\x00/sigil/g3/txstokio\x00/sigil/g3/release\x00/sigil/g3/peer-hblake\x00");
        bytes.extend_from_slice(b"\x00/sigil/g0/blocks gossipsub topic\x00/sigil/g0/balance\x00");
        bytes.extend_from_slice(b"\x00rustc version 1.93.1 (01f6ddf75 2026-02-11)\x00");
        bytes.extend_from_slice(b"\x00full:51404b79aa00bb11cc22dd33ee44ff55\x00");
        bytes.extend_from_slice(b"\x00sigil-top/10.7.3 kgauge\x00mainnet-genesis\x00");
        let s = scan_bytes(&bytes);
        assert_eq!(s.rustc.as_deref(), Some("rustc version 1.93.1 (01f6ddf75 2026-02-11)"));
        assert_eq!(s.flux_rev.as_deref(), Some("full:51404b79aa00bb11cc22dd33ee44ff55"));
        assert_eq!(s.versions, vec!["sigil-top/10.7.3".to_string()]);
        assert_eq!(s.mentions[0], ("sigil-g2".to_string(), 66));
        assert!(s.mentions.iter().any(|(n, c)| n == "mainnet-genesis" && *c == 1));
        assert_eq!(s.wire_topics[0].0, "g3");
        assert_eq!(s.wire_topics[0].1, vec!["blocks", "release", "txs"]);
        assert_eq!(s.topic_hits, vec![("g3".to_string(), 3), ("g0".to_string(), 1)]);
        let (net, basis, amb) = network_verdict(&s.wire_topics, &s.topic_hits, &s.mentions);
        assert_eq!((net.as_deref(), basis.as_str(), amb), (Some("sigil-g3"), "wire-topics", false));
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
        let (n, b, a) = network_verdict(&[g("g3", &six), g("g2", five), g("g0", &["blocks"])], &h(&[("g3", 21), ("g2", 5), ("g0", 3)]), &[]);
        assert_eq!((n.as_deref(), b.as_str(), a), (Some("sigil-g3"), "wire-topics", false));
        let (n, b, a) = network_verdict(&[g("g2", &six), g("g0", &["blocks"])], &h(&[("g2", 25), ("g0", 3)]), &[]);
        assert_eq!((n.as_deref(), b.as_str(), a), (Some("sigil-g2"), "wire-topics", false));
        let (n, b, a) = network_verdict(&[g("g2", &six), g("g3", &six)], &h(&[("g2", 21), ("g3", 15)]), &[]);
        assert_eq!((n, b.as_str(), a), (None, "wire-topics", true));
        let (n, b, a) = network_verdict(&[g("g0", &["blocks"])], &h(&[("g0", 3)]), &[("mainnet-genesis".to_string(), 7)]);
        assert_eq!((n.as_deref(), b.as_str(), a), (Some("mainnet-genesis"), "mention", false));
        let (n, b, a) = network_verdict(&[g("g3", &["blocks"])], &h(&[("g3", 1)]), &[("mainnet-genesis".to_string(), 6), ("sigil-g2".to_string(), 5)]);
        assert_eq!((n, b.as_str(), a), (None, "none", false));
        assert_eq!(network_verdict(&[], &[], &[]), (None, "none".to_string(), false));
    }

    #[test]
    fn resolve_network_keeps_declared_and_evidence_apart() {
        assert_eq!(resolve_network(Some("sigil-g3"), Some("sigil-g3"), "wire-topics"), (Some("sigil-g3".into()), "stamp+evidence agree".into(), false));
        let (n, b, c) = resolve_network(Some("sigil-r3"), Some("sigil-g3"), "wire-topics");
        assert_eq!((n, b.as_str(), c), (None, "conflict", true));
        let (n, b, c) = resolve_network(Some("sigil-r3"), None, "none");
        assert_eq!(n.as_deref(), Some("sigil-r3")); assert!(b.starts_with("declared-only")); assert!(!c);
        let (n, b, c) = resolve_network(None, Some("sigil-g2"), "wire-topics");
        assert_eq!((n.as_deref(), b.as_str(), c), (Some("sigil-g2"), "wire-topics (heuristic)", false));
        assert_eq!(resolve_network(None, None, "none"), (None, "none".into(), false));
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
        assert_eq!(uf.exec_start_path.as_deref(), Some("/home/storage/sigil-g3-bin/sigil-top"));
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

    /// A minimal ELF64 LE EXECUTABLE-shaped file with one .note.gnu.build-id SECTION carrying `desc`.
    fn fake_elf_sections(desc: &[u8]) -> Vec<u8> {
        let mut note = Vec::new();
        note.extend_from_slice(&4u32.to_le_bytes());
        note.extend_from_slice(&(desc.len() as u32).to_le_bytes());
        note.extend_from_slice(&3u32.to_le_bytes());
        note.extend_from_slice(b"GNU\0");
        note.extend_from_slice(desc);
        while note.len() % 4 != 0 { note.push(0); }
        let strs = b"\0.note.gnu.build-id\0.shstrtab\0".to_vec();
        let note_off = 64usize;
        let strs_off = note_off + note.len();
        let mut shoff = strs_off + strs.len();
        while shoff % 8 != 0 { shoff += 1; }
        let mut f = vec![0u8; shoff + 3 * 64];
        f[0..4].copy_from_slice(b"\x7fELF"); f[4] = 2; f[5] = 1; f[6] = 1;
        f[16..18].copy_from_slice(&2u16.to_le_bytes()); f[18..20].copy_from_slice(&0x3eu16.to_le_bytes());
        f[0x28..0x30].copy_from_slice(&(shoff as u64).to_le_bytes());
        f[0x3a..0x3c].copy_from_slice(&64u16.to_le_bytes());
        f[0x3c..0x3e].copy_from_slice(&3u16.to_le_bytes());
        f[0x3e..0x40].copy_from_slice(&2u16.to_le_bytes());
        f[note_off..note_off + note.len()].copy_from_slice(&note);
        f[strs_off..strs_off + strs.len()].copy_from_slice(&strs);
        let mut sh = |i: usize, name: u32, ty: u32, off: usize, size: usize| {
            let b = shoff + i * 64;
            f[b..b + 4].copy_from_slice(&name.to_le_bytes());
            f[b + 4..b + 8].copy_from_slice(&ty.to_le_bytes());
            f[b + 0x18..b + 0x20].copy_from_slice(&(off as u64).to_le_bytes());
            f[b + 0x20..b + 0x28].copy_from_slice(&(size as u64).to_le_bytes());
        };
        sh(1, 1, 7, note_off, note.len());
        sh(2, 20, 3, strs_off, strs.len());
        f
    }

    /// The same note reachable ONLY through a PT_NOTE program header (section table stripped).
    fn fake_elf_phdr_only(desc: &[u8]) -> Vec<u8> {
        let mut note = Vec::new();
        note.extend_from_slice(&(NOTE_OWNER_STARDUST.len() as u32).to_le_bytes());
        note.extend_from_slice(&(desc.len() as u32).to_le_bytes());
        note.extend_from_slice(&NT_STARDUST.to_le_bytes());
        note.extend_from_slice(NOTE_OWNER_STARDUST);
        while note.len() % 4 != 0 { note.push(0); }
        note.extend_from_slice(desc);
        let phoff = 64usize;
        let note_off = phoff + 56;
        let mut f = vec![0u8; note_off + note.len()];
        f[0..4].copy_from_slice(b"\x7fELF"); f[4] = 2; f[5] = 1; f[6] = 1;
        f[16..18].copy_from_slice(&2u16.to_le_bytes()); f[18..20].copy_from_slice(&0x3eu16.to_le_bytes());
        f[0x20..0x28].copy_from_slice(&(phoff as u64).to_le_bytes());
        f[0x36..0x38].copy_from_slice(&56u16.to_le_bytes());
        f[0x38..0x3a].copy_from_slice(&1u16.to_le_bytes());
        // e_shnum = 0, e_shoff = 0 → no section table
        f[phoff..phoff + 4].copy_from_slice(&4u32.to_le_bytes());                 // PT_NOTE
        f[phoff + 8..phoff + 16].copy_from_slice(&(note_off as u64).to_le_bytes()); // p_offset
        f[phoff + 32..phoff + 40].copy_from_slice(&(note.len() as u64).to_le_bytes()); // p_filesz
        f[note_off..].copy_from_slice(&note);
        f
    }

    #[test]
    fn note_object_round_trips_through_the_reader() {
        let stamp = json!({"stardust": 2, "pkg": "sigil-top", "env": {"SIGIL_NETWORK": "sigil-g3"}, "test": false});
        let obj = make_note_object(&stamp);
        // The object IS an ELF64 file with a .note.stardust section: the reader must find it.
        let body = elf_note(&obj, NOTE_OWNER_STARDUST, NT_STARDUST).expect("stardust note in the object");
        assert_eq!(body.len() % 4, 0);
        assert_eq!(decode_stamp(&body).unwrap(), stamp);
        assert!(elf_build_id(&obj).is_none(), "the object carries no GNU build-id");
        // Legacy v1 stamp in a build-id section, and a plain sha1 build-id.
        let legacy = b"{\"stardust\":1,\"pkg\":\"x\"}   ";
        let f = fake_elf_sections(legacy);
        assert_eq!(elf_build_id(&f).as_deref(), Some(&legacy[..]));
        assert_eq!(elf_build_id(&fake_elf_sections(&[7u8; 20])).map(|d| d.len()), Some(20));
        assert!(decode_stamp(&[7u8; 20]).is_none());
        // Program-header-only path.
        let padded = pad4(b"{\"stardust\":2,\"pkg\":\"ph\"}".to_vec(), b' ');
        let ph = fake_elf_phdr_only(&padded);
        assert_eq!(decode_stamp(&elf_note(&ph, NOTE_OWNER_STARDUST, NT_STARDUST).unwrap()).unwrap()["pkg"], "ph");
    }

    #[test]
    fn reader_never_panics_on_garbage() {
        // Deterministic xorshift noise, truncations of a real object, and hostile headers.
        let mut x: u64 = 0x9E3779B97F4A7C15;
        let mut rnd = || { x ^= x << 13; x ^= x >> 7; x ^= x << 17; x };
        for _ in 0..300 {
            let len = (rnd() % 600) as usize;
            let bytes: Vec<u8> = (0..len).map(|_| rnd() as u8).collect();
            let _ = elf_note(&bytes, NOTE_OWNER_STARDUST, NT_STARDUST);
            let _ = elf_build_id(&bytes);
            let _ = decode_stamp(&bytes);
            let _ = parse_note_desc(&bytes, NOTE_OWNER_GNU, 3);
        }
        let obj = make_note_object(&json!({"stardust": 2}));
        for cut in 0..obj.len() { let _ = elf_note(&obj[..cut], NOTE_OWNER_STARDUST, NT_STARDUST); }
        // Valid header with absurd offsets/sizes.
        let mut evil = obj.clone();
        evil[0x28..0x30].copy_from_slice(&u64::MAX.to_le_bytes());
        evil[0x3c..0x3e].copy_from_slice(&u16::MAX.to_le_bytes());
        evil[0x20..0x28].copy_from_slice(&(u64::MAX - 7).to_le_bytes());
        evil[0x36..0x38].copy_from_slice(&u16::MAX.to_le_bytes());
        evil[0x38..0x3a].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(elf_note(&evil, NOTE_OWNER_STARDUST, NT_STARDUST).is_none());
        // A note header claiming a gigantic name.
        let mut sec = vec![0u8; 24];
        sec[0..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_note_desc(&sec, NOTE_OWNER_GNU, 3).is_none());
    }

    #[test]
    fn writer_gates_on_elf_executable_links() {
        let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(links_elf_executable(&v(&["--crate-name", "a", "src/main.rs", "--crate-type", "bin", "--emit=dep-info,link", "-C", "opt-level=3"])));
        assert!(links_elf_executable(&v(&["--crate-name", "a", "src/lib.rs", "--test", "--emit=dep-info,link"])));
        assert!(links_elf_executable(&v(&["src/main.rs", "--crate-type=bin", "--target", "x86_64-unknown-linux-musl"])));
        assert!(!links_elf_executable(&v(&["src/lib.rs", "--crate-type", "lib", "--emit=dep-info,metadata,link"])));
        assert!(!links_elf_executable(&v(&["src/main.rs", "--crate-type", "bin", "--emit=dep-info,metadata"])));
        assert!(!links_elf_executable(&v(&["src/main.rs", "--crate-type", "bin", "--target", "x86_64-pc-windows-gnu"])));
        assert!(!links_elf_executable(&v(&["src/main.rs", "--crate-type", "bin", "--target", "wasm32-unknown-unknown"])));
        assert!(!links_elf_executable(&v(&["src/main.rs", "--crate-type", "bin", "-C", "linker=rust-lld"])));
        assert!(!links_elf_executable(&v(&["src/main.rs", "--crate-type", "bin", "-C", "linker-flavor=ld.lld"])));
        assert!(links_elf_executable(&v(&["src/main.rs", "--crate-type", "bin", "-C", "linker-flavor=gnu-lld-cc"])));
        assert!(links_elf_executable(&v(&["src/main.rs", "--crate-type", "bin", "-C", "linker=/home/storage/tools/zig/zcc"])));
    }

    #[test]
    fn stamp_is_declared_metadata_without_host_by_default() {
        let fake_env = |k: &str| -> Option<String> {
            match k {
                "CARGO_CRATE_NAME" => Some("sigil_top".into()),
                "CARGO_PKG_NAME" => Some("sigil-top".into()),
                "CARGO_PKG_VERSION" => Some("10.7.5".into()),
                "CARGO_BIN_NAME" => Some("sigil-top".into()),
                "SIGIL_NETWORK" => Some("sigil-g3".into()),
                _ => None,
            }
        };
        let args: Vec<String> = ["--crate-name", "sigil_top", "src/main.rs", "--crate-type", "bin", "-C", "opt-level=3", "--target", "x86_64-unknown-linux-gnu", "--out-dir", "/tmp/x/deps"].iter().map(|s| s.to_string()).collect();
        let st = build_stamp(&args, &fake_env);
        assert_eq!(st["stardust"], STAMP_SCHEMA);
        assert_eq!(st["env"]["SIGIL_NETWORK"], "sigil-g3");
        assert!(st["env"]["SIGIL_NET_ID"].is_null());
        assert_eq!(st["opt"], "3");
        assert_eq!(st["target"], "x86_64-unknown-linux-gnu");
        assert_eq!(st["bin"], "sigil-top");
        assert_eq!(st["test"], false);
        assert!(st.get("host").is_none(), "hostname is not recorded unless FLUX_STARDUST_HOST=1");
        assert!(st["git_head"].is_null(), "no CARGO_MANIFEST_DIR → no git facts");
        let obj = make_note_object(&st);
        let p = stamp_object_path(&args, &obj);
        assert!(p.starts_with("/tmp/x/deps/.stardust"), "{p:?}");
        assert!(p.file_name().unwrap().to_string_lossy().starts_with("sigil_top-"));
    }

    #[test]
    fn stamp_text_is_not_evidence_of_itself() {
        // A binary whose ONLY mention of a network is inside its own stamp must read declared-only.
        let st = json!({"stardust": 2, "pkg": "hello", "env": {"SIGIL_NETWORK": "sigil-r3"}});
        let obj = make_note_object(&st);
        let r = elf_note_range(&obj, NOTE_OWNER_STARDUST, NT_STARDUST).unwrap();
        assert_eq!(decode_stamp(&obj[r.clone()]).unwrap(), st);
        let mut masked = obj.clone();
        for b in &mut masked[r] { *b = 0; }
        let scan = scan_bytes(&masked);
        assert!(scan.mentions.is_empty(), "the stamp's own text leaked into the mentions: {:?}", scan.mentions);
        let unmasked = scan_bytes(&obj);
        assert_eq!(unmasked.mentions, vec![("sigil-r3".to_string(), 1)], "without masking the stamp would vote for itself");
    }

    #[test]
    fn git_facts_read_the_flux_tree() {
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        let h = git_head_full(here).expect("the flux tree has a git HEAD");
        assert_eq!(h.len(), 40);
        assert!(h.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(git_head_short(here).unwrap().len(), 12);
        assert!(git_head_full(Path::new("/proc")).is_none());
        // dirtiness is a bool here (a git tree), never asserted either way — it depends on the box
        let _ = git_dirty(here);
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
        let exe = std::env::current_exe().unwrap();
        let d = read(&exe.to_string_lossy(), &Opts::default());
        assert_eq!(d.kind, "path");
        assert!(d.rustc.is_some(), "a Rust test binary carries its rustc version string");
        assert!(d.blake3.is_some() && d.sha256.is_some());
        // Linked through the fluxc wrapper → a stamp naming this crate (v2 note or legacy build-id).
        if let Some(st) = &d.stamp {
            assert_eq!(st["crate"], "fluxc_core");
            assert_eq!(st["test"], true);
            if st["stardust"] == STAMP_SCHEMA {
                assert!(d.build_id.is_some(), "v2 leaves the GNU build-id in place: {:?}", d.stamp_source);
            }
        }
        let me = read(&std::process::id().to_string(), &Opts::default());
        assert_eq!(me.kind, "pid");
        assert_eq!(me.unit.as_ref().map(|u| u.main_pid), Some(std::process::id()));
        assert_eq!(me.unit.as_ref().and_then(|u| u.file_newer_than_process), Some(false));
    }
}
