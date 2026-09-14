//! `fluxc deploy-check <unit>` — which binary does a systemd unit actually RUN, versus
//! which binary does its effective `ExecStart=` NAME, versus what is on disk at that path?
//!
//! Why this exists (2026-09-10, SIGIL deploy): `systemctl cat sigil-node` showed the base
//! unit's `ExecStart=/home/storage/deepseek-codewhale/sigil/target/release/sigil-node` while a
//! drop-in had long since repointed it at `/home/storage/rocky-mintprof-target/release/sigil-node`.
//! A 7-minute release build went to the first path and a restart would have "deployed" nothing.
//! The retro (docs/FLUX_SIGIL_RETRO_2026-09-13.md) asked for one check that reads the two
//! sources that cannot lie — `systemctl show -p ExecStart` (drop-ins applied) and
//! `/proc/<MainPID>/exe` (what the kernel loaded) — and compares them by path AND by content.
//!
//! Three failure shapes, each seen live on this box:
//!   * `PathMismatch`   ExecStart names X, the process runs Y   → your build targets the wrong dir
//!   * `StaleOnDisk`    same path, different bytes              → built but never restarted
//!   * `Deleted`        the running exe is `(deleted)`          → the file was replaced/removed under it
//!                                                                (fluxc serve pid 785435 sat like this for 2 days)
//! `Match` is the only state in which "restart = deploy this binary" is a true sentence.
//!
//! Local or remote (`--host`): the remote path runs the same three commands over ssh and parses
//! the same text, so both hosts of a producer/follower pair are checked by one code path.

use std::path::{Path, PathBuf};
use std::process::Command;

/// One unit's binary identity, as measured (never as configured).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitIdentity {
    pub unit: String,
    pub host: Option<String>,
    pub active_state: String,
    /// Effective `ExecStart=` path (drop-ins applied), as systemd resolves it.
    pub exec_start: Option<PathBuf>,
    pub main_pid: u32,
    /// `readlink /proc/<MainPID>/exe` with any ` (deleted)` suffix stripped.
    pub running_exe: Option<PathBuf>,
    pub running_deleted: bool,
    /// sha256 of the file at `exec_start` (what a restart would load).
    pub exec_sha256: Option<String>,
    /// sha256 of `/proc/<MainPID>/exe` (what is loaded now; readable even when deleted).
    pub running_sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// ExecStart path == running exe path, and the bytes on disk == the bytes in memory.
    Match,
    /// The unit is not running (MainPID 0). A restart would load `exec_start`.
    NotRunning,
    /// The running exe has been deleted/replaced underneath the process.
    Deleted,
    /// ExecStart names one file, the process runs another. Build target is probably wrong.
    PathMismatch,
    /// Same path, different content: a new binary is on disk but the old one is running.
    StaleOnDisk,
    /// Could not determine one side (no ExecStart parsed, unreadable exe, …).
    Unknown,
}

impl Verdict {
    /// Exit status for the CLI: 0 only on `Match`.
    pub fn exit_code(self) -> i32 {
        match self {
            Verdict::Match => 0,
            Verdict::Unknown => 3,
            _ => 2,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Match => "MATCH",
            Verdict::NotRunning => "NOT-RUNNING",
            Verdict::Deleted => "DELETED-EXE",
            Verdict::PathMismatch => "PATH-MISMATCH",
            Verdict::StaleOnDisk => "STALE-ON-DISK",
            Verdict::Unknown => "UNKNOWN",
        }
    }
}

impl UnitIdentity {
    pub fn verdict(&self) -> Verdict {
        if self.main_pid == 0 {
            return Verdict::NotRunning;
        }
        if self.running_deleted {
            return Verdict::Deleted;
        }
        let (Some(es), Some(re)) = (&self.exec_start, &self.running_exe) else {
            return Verdict::Unknown;
        };
        if es != re {
            return Verdict::PathMismatch;
        }
        match (&self.exec_sha256, &self.running_sha256) {
            (Some(a), Some(b)) if a == b => Verdict::Match,
            (Some(_), Some(_)) => Verdict::StaleOnDisk,
            _ => Verdict::Unknown,
        }
    }

    /// Does the RUNNING binary satisfy an operator expectation — a sha256 hex or a path?
    pub fn satisfies(&self, expect: &str) -> bool {
        let e = expect.trim();
        if e.len() == 64 && e.bytes().all(|b| b.is_ascii_hexdigit()) {
            return self.running_sha256.as_deref().map(|s| s.eq_ignore_ascii_case(e)).unwrap_or(false);
        }
        self.running_exe.as_deref() == Some(Path::new(e))
    }
}

/// Unit names as systemd accepts them, restricted to the charset we are willing to put in a
/// remote shell string (SEC-001 — same discipline as `safe_cmd_charset` in fluxc-mcp).
pub fn safe_unit(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'@' | b'.' | b'_' | b'-'))
}

/// Hostname/IP charset only — never interpolate anything else into `ssh root@<host>`.
pub fn safe_host(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 253
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':'))
}

/// `ExecStart={ path=/x/y ; argv[]=/x/y start ; ignore_errors=no ; … }` → `/x/y`.
/// Older systemd prints `ExecStart=/x/y start` — handled too.
pub fn parse_exec_start(line: &str) -> Option<PathBuf> {
    let v = line.strip_prefix("ExecStart=")?.trim();
    if let Some(rest) = v.strip_prefix('{') {
        let rest = rest.trim_start();
        let rest = rest.strip_prefix("path=")?;
        let end = rest.find(|c: char| c == ';' || c.is_whitespace()).unwrap_or(rest.len());
        let p = rest[..end].trim();
        return if p.is_empty() { None } else { Some(PathBuf::from(p)) };
    }
    let first = v.split_whitespace().next()?;
    // systemd allows a leading `-`/`+`/`!`/`@` prefix on the path
    let first = first.trim_start_matches(['-', '+', '!', '@']);
    if first.is_empty() { None } else { Some(PathBuf::from(first)) }
}

/// The shell snippet both the local and remote paths run. Prints, one per line:
/// the raw `systemctl show` properties, then `EXE=`, `RUNSHA=`, `EXECSHA=` when derivable.
/// Written so that every stage degrades to an absent line rather than a failed script.
pub fn probe_script(unit: &str) -> String {
    format!(
        "U={u}; systemctl show \"$U\" -p ExecStart -p MainPID -p ActiveState 2>/dev/null; \
         P=$(systemctl show \"$U\" -p MainPID --value 2>/dev/null); \
         if [ -n \"$P\" ] && [ \"$P\" != 0 ]; then \
           echo \"EXE=$(readlink /proc/$P/exe 2>/dev/null)\"; \
           sha256sum /proc/$P/exe 2>/dev/null | awk '{{print \"RUNSHA=\"$1}}'; \
         fi; \
         E=$(systemctl show \"$U\" -p ExecStart --value 2>/dev/null | sed -n 's/^{{ *path=\\([^ ;]*\\).*/\\1/p'); \
         [ -z \"$E\" ] && E=$(systemctl show \"$U\" -p ExecStart --value 2>/dev/null | awk '{{print $1}}' | sed 's/^[-+!@]*//'); \
         if [ -n \"$E\" ] && [ -f \"$E\" ]; then sha256sum \"$E\" 2>/dev/null | awk '{{print \"EXECSHA=\"$1}}'; fi",
        u = unit
    )
}

/// Parse the probe output (pure — testable on captured text).
pub fn parse_probe(unit: &str, host: Option<&str>, out: &str) -> UnitIdentity {
    let mut id = UnitIdentity {
        unit: unit.to_string(),
        host: host.map(String::from),
        active_state: String::from("unknown"),
        exec_start: None,
        main_pid: 0,
        running_exe: None,
        running_deleted: false,
        exec_sha256: None,
        running_sha256: None,
    };
    for line in out.lines() {
        let line = line.trim_end();
        if line.starts_with("ExecStart=") {
            id.exec_start = parse_exec_start(line);
        } else if let Some(v) = line.strip_prefix("MainPID=") {
            id.main_pid = v.trim().parse().unwrap_or(0);
        } else if let Some(v) = line.strip_prefix("ActiveState=") {
            id.active_state = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("EXE=") {
            let v = v.trim();
            if let Some(stripped) = v.strip_suffix(" (deleted)") {
                id.running_deleted = true;
                id.running_exe = Some(PathBuf::from(stripped));
            } else if !v.is_empty() {
                id.running_exe = Some(PathBuf::from(v));
            }
        } else if let Some(v) = line.strip_prefix("RUNSHA=") {
            id.running_sha256 = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("EXECSHA=") {
            id.exec_sha256 = Some(v.trim().to_string());
        }
    }
    id
}

/// Inspect a unit on this box.
pub fn inspect_local(unit: &str) -> Result<UnitIdentity, String> {
    if !safe_unit(unit) {
        return Err(format!("unit {unit:?} rejected (systemd unit charset only)"));
    }
    let out = Command::new("bash").arg("-c").arg(probe_script(unit)).output()
        .map_err(|e| format!("bash: {e}"))?;
    Ok(parse_probe(unit, None, &String::from_utf8_lossy(&out.stdout)))
}

/// Inspect a unit on another box over ssh (key auth, 10 s connect timeout).
pub fn inspect_remote(host: &str, unit: &str) -> Result<UnitIdentity, String> {
    if !safe_unit(unit) {
        return Err(format!("unit {unit:?} rejected (systemd unit charset only)"));
    }
    if !safe_host(host) {
        return Err(format!("host {host:?} rejected (hostname/IP chars only) [SEC-001]"));
    }
    let out = Command::new("ssh")
        .arg("-o").arg("BatchMode=yes")
        .arg("-o").arg("StrictHostKeyChecking=no")
        .arg("-o").arg("ConnectTimeout=10")
        .arg(format!("root@{host}"))
        .arg(probe_script(unit))
        .output()
        .map_err(|e| format!("ssh: {e}"))?;
    if !out.status.success() && out.stdout.is_empty() {
        return Err(format!("ssh root@{host}: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(parse_probe(unit, Some(host), &String::from_utf8_lossy(&out.stdout)))
}

fn short(s: &Option<String>) -> String {
    s.as_deref().map(|h| h.chars().take(16).collect()).unwrap_or_else(|| "—".into())
}

/// Human rendering: one block per unit, verdict first.
pub fn render(id: &UnitIdentity) -> String {
    let v = id.verdict();
    let where_ = id.host.as_deref().map(|h| format!("@{h}")).unwrap_or_else(|| "(local)".into());
    let mut s = format!("⬡ deploy-check {} {}  →  {}\n", id.unit, where_, v.label());
    s.push_str(&format!("  ActiveState  {}\n", id.active_state));
    s.push_str(&format!("  ExecStart    {}  sha256 {}\n",
        id.exec_start.as_deref().map(|p| p.display().to_string()).unwrap_or_else(|| "—".into()),
        short(&id.exec_sha256)));
    s.push_str(&format!("  running exe  {}{}  sha256 {}  (pid {})\n",
        id.running_exe.as_deref().map(|p| p.display().to_string()).unwrap_or_else(|| "—".into()),
        if id.running_deleted { " (deleted)" } else { "" },
        short(&id.running_sha256), id.main_pid));
    let note = match v {
        Verdict::Match => "restart = deploy exactly this binary",
        Verdict::NotRunning => "unit is down; a start would load ExecStart",
        Verdict::Deleted => "the file under the process is gone/replaced — restart to load what is on disk, and check ExecStart still points at a real file",
        Verdict::PathMismatch => "ExecStart and the process disagree — a build into the ExecStart dir is what a restart picks up; the running path is a lie",
        Verdict::StaleOnDisk => "a newer binary sits at ExecStart; the old one is still running — this is the 'built, not restarted' state",
        Verdict::Unknown => "one side could not be read (is the unit real? is /proc readable?)",
    };
    s.push_str(&format!("  → {note}\n"));
    s
}

/// JSON rendering for MCP/automation.
pub fn to_json(id: &UnitIdentity) -> serde_json::Value {
    serde_json::json!({
        "unit": id.unit,
        "host": id.host,
        "verdict": id.verdict().label(),
        "active_state": id.active_state,
        "exec_start": id.exec_start.as_deref().map(|p| p.display().to_string()),
        "exec_sha256": id.exec_sha256,
        "main_pid": id.main_pid,
        "running_exe": id.running_exe.as_deref().map(|p| p.display().to_string()),
        "running_deleted": id.running_deleted,
        "running_sha256": id.running_sha256,
    })
}

/// CLI entry: `fluxc deploy-check <unit> [--host H] [--expect <sha256|path>] [--json]`.
/// Returns the process exit code.
pub fn run(args: &[String]) -> i32 {
    let unit = match args.first() {
        Some(u) if !u.starts_with("--") => u.clone(),
        _ => {
            eprintln!("usage: fluxc deploy-check <unit> [--host HOST] [--expect <sha256|path>] [--json]");
            return 3;
        }
    };
    let opt = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let host = opt("--host");
    let expect = opt("--expect");
    let want_json = args.iter().any(|a| a == "--json");
    let id = match host.as_deref() {
        Some(h) => inspect_remote(h, &unit),
        None => inspect_local(&unit),
    };
    let id = match id {
        Ok(i) => i,
        Err(e) => { eprintln!("deploy-check: {e}"); return 3; }
    };
    let mut code = id.verdict().exit_code();
    let expect_ok = expect.as_deref().map(|e| id.satisfies(e));
    if want_json {
        let mut j = to_json(&id);
        if let Some(ok) = expect_ok { j["expect_ok"] = serde_json::Value::Bool(ok); }
        println!("{}", serde_json::to_string_pretty(&j).unwrap_or_default());
    } else {
        print!("{}", render(&id));
        if let Some(ok) = expect_ok {
            println!("  expect       {}  →  {}", expect.as_deref().unwrap_or(""), if ok { "OK" } else { "NOT SATISFIED" });
        }
    }
    if expect_ok == Some(false) { code = 2; }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOW_DROPIN: &str = "ExecStart={ path=/home/storage/rocky-mintprof-target/release/sigil-node ; argv[]=/home/storage/rocky-mintprof-target/release/sigil-node start ; ignore_errors=no ; start_time=[Sun 2026-09-13 23:35:12 CEST] ; stop_time=[n/a] ; pid=3316453 ; code=(null) ; status=0/0 }\nMainPID=3316453\nActiveState=active\n";

    #[test]
    fn exec_start_parses_new_and_old_systemd_forms() {
        assert_eq!(parse_exec_start("ExecStart={ path=/a/b ; argv[]=/a/b start ; ignore_errors=no }"),
            Some(PathBuf::from("/a/b")));
        assert_eq!(parse_exec_start("ExecStart=/opt/x/bin --flag"), Some(PathBuf::from("/opt/x/bin")));
        assert_eq!(parse_exec_start("ExecStart=-/opt/x/bin"), Some(PathBuf::from("/opt/x/bin")));
        assert_eq!(parse_exec_start("ExecStart="), None);
        assert_eq!(parse_exec_start("ExecStart={ argv[]=/a }"), None);
    }

    #[test]
    fn match_when_path_and_bytes_agree() {
        let out = format!("{SHOW_DROPIN}EXE=/home/storage/rocky-mintprof-target/release/sigil-node\nRUNSHA={0}\nEXECSHA={0}\n", "ab".repeat(32));
        let id = parse_probe("sigil-node", None, &out);
        assert_eq!(id.main_pid, 3316453);
        assert_eq!(id.verdict(), Verdict::Match);
        assert_eq!(id.verdict().exit_code(), 0);
        assert!(id.satisfies(&"AB".repeat(32)));
        assert!(id.satisfies("/home/storage/rocky-mintprof-target/release/sigil-node"));
        assert!(!id.satisfies("/home/storage/deepseek-codewhale/sigil/target/release/sigil-node"));
    }

    #[test]
    fn the_2026_09_10_shape_is_a_path_mismatch() {
        // `systemctl cat` said target/release; the process ran rocky-mintprof-target.
        let out = "ExecStart={ path=/home/storage/deepseek-codewhale/sigil/target/release/sigil-node ; argv[]=… }\nMainPID=42\nActiveState=active\nEXE=/home/storage/rocky-mintprof-target/release/sigil-node\nRUNSHA=11\nEXECSHA=22\n";
        let id = parse_probe("sigil-node", Some("10.77.0.5"), out);
        assert_eq!(id.verdict(), Verdict::PathMismatch);
        assert_eq!(id.verdict().exit_code(), 2);
        assert_eq!(id.host.as_deref(), Some("10.77.0.5"));
    }

    #[test]
    fn built_but_not_restarted_is_stale_on_disk() {
        let out = format!("{SHOW_DROPIN}EXE=/home/storage/rocky-mintprof-target/release/sigil-node\nRUNSHA=old\nEXECSHA=new\n");
        assert_eq!(parse_probe("sigil-node", None, &out).verdict(), Verdict::StaleOnDisk);
    }

    #[test]
    fn deleted_exe_is_flagged_even_when_sha_matches() {
        // fluxc serve pid 785435 (2026-09-12..14): exe replaced under the process.
        let out = "ExecStart={ path=/x/fluxc ; argv[]=/x/fluxc serve }\nMainPID=785435\nActiveState=active\nEXE=/x/fluxc (deleted)\nRUNSHA=aa\nEXECSHA=aa\n";
        let id = parse_probe("fluxc-serve", None, out);
        assert!(id.running_deleted);
        assert_eq!(id.running_exe, Some(PathBuf::from("/x/fluxc")));
        assert_eq!(id.verdict(), Verdict::Deleted);
    }

    #[test]
    fn not_running_and_unknown() {
        let out = "ExecStart={ path=/x/y ; argv[]=/x/y }\nMainPID=0\nActiveState=inactive\nEXECSHA=aa\n";
        assert_eq!(parse_probe("y", None, out).verdict(), Verdict::NotRunning);
        let out = "MainPID=7\nActiveState=active\nEXE=/x/y\n";
        assert_eq!(parse_probe("y", None, out).verdict(), Verdict::Unknown);
        assert_eq!(Verdict::Unknown.exit_code(), 3);
    }

    #[test]
    fn charsets_refuse_shell_metacharacters() {
        assert!(safe_unit("sigil-node"));
        assert!(safe_unit("wg-quick@sigilwg0.service"));
        assert!(!safe_unit("sigil-node; rm -rf /"));
        assert!(!safe_unit(""));
        assert!(safe_host("10.77.0.5"));
        assert!(safe_host("happysrv.local"));
        assert!(!safe_host("h`id`"));
        assert!(!safe_host("a b"));
    }

    #[test]
    fn probe_script_embeds_only_the_validated_unit() {
        let s = probe_script("sigil-node");
        assert!(s.starts_with("U=sigil-node;"));
        assert!(s.contains("readlink /proc/$P/exe"));
        assert!(s.contains("EXECSHA="));
    }

    #[test]
    fn local_probe_on_a_missing_unit_reports_not_running_not_a_crash() {
        // systemctl show on an unknown unit exits 0 with MainPID=0 / inactive; bash always exists.
        if Command::new("systemctl").arg("--version").output().is_err() { return; }
        let id = inspect_local("fluxc-deploy-check-does-not-exist-9f3a").expect("probe runs");
        assert_eq!(id.verdict(), Verdict::NotRunning);
    }
}
