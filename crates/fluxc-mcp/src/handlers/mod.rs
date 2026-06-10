use std::collections::HashMap;
use serde_json::Value;

/// A single tool handler: takes arguments JSON, returns result string.
pub type ToolFn = fn(&Value) -> String;

/// Tool definition registered with the MCP server.
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

/// Registry of all MCP tools. Each handler module calls `register()`.
pub struct ToolRegistry {
    tools: Vec<ToolDef>,
    handlers: HashMap<String, ToolFn>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        ToolRegistry {
            tools: Vec::new(),
            handlers: HashMap::new(),
        }
    }

    /// Register a tool: its schema (for tools/list) and its handler function.
    pub fn register(&mut self, def: ToolDef, handler: ToolFn) {
        self.handlers.insert(def.name.to_string(), handler);
        self.tools.push(def);
    }

    /// Build the full tools/list schema response.
    pub fn tools_schema(&self) -> Vec<Value> {
        self.tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "inputSchema": t.input_schema,
                })
            })
            .collect()
    }

    /// Dispatch a tool call by name.
    pub fn execute(&self, name: &str, args: &Value) -> Option<String> {
        self.handlers.get(name).map(|h| h(args))
    }
}

/// Workspace root, robustly resolved from the running fluxc binary's location
/// (not cwd). Use this anywhere we'd otherwise pass `"."` to cargo subprocesses
/// or to `quantum_architect::analyze_workspace`.
///
/// Why this exists: the MCP server is spawned by an IDE / agent runner whose
/// cwd is *its* cwd (typically the user's home or claude-code workspace), not
/// the Flux source tree. Without this, every cargo build fails with `could not
/// find Cargo.toml`, every `analyze_workspace` returns 0 crates, every
/// `flux_iterate` falsely reports failure.
pub fn ws() -> std::path::PathBuf {
    fluxc_core::version::workspace_root()
}

/// Convenience wrapper around `analyze_workspace` anchored at `ws()`.
pub fn analyze_ws() -> fluxc_core::quantum_architect::QuantumArchitecture {
    fluxc_core::quantum_architect::analyze_workspace(&ws().to_string_lossy())
}


/// Workspace `target/debug/fluxc` with cargo on PATH (flux-dev: never flux-cargo-wrapper).
pub fn fluxc_cmd() -> std::process::Command {
    let fluxc = ws().join("target/debug/fluxc");
    let mut cmd = std::process::Command::new(fluxc);
    cmd.current_dir(ws());
    let path = std::env::var("PATH").unwrap_or_default();
    if !path.contains("/root/.cargo/bin") {
        cmd.env("PATH", format!("/root/.cargo/bin:{path}"));
    }
    cmd
}

/// Convenience wrapper: a `cargo` Command pre-anchored at the workspace root.
/// All cargo subprocesses in handler modules should construct via this.
pub fn cargo_cmd() -> std::process::Command {
    let mut cmd = std::process::Command::new("cargo");
    cmd.current_dir(ws());
    let path = std::env::var("PATH").unwrap_or_default();
    if !path.contains("/root/.cargo/bin") {
        cmd.env("PATH", format!("/root/.cargo/bin:{path}"));
    }
    cmd
}

/// Parse `cargo test` output into aggregated (passed, failed) counts.
///
/// `cargo test -p X` runs one or more test binaries (lib, integration,
/// doctest); each emits its own `test result: ok. N passed; M failed; …`
/// summary line. We sum across all such lines instead of counting per-test
/// `... ok` / `FAILED` lines, which would (a) miss anything under `--quiet`
/// (no per-test output) and (b) double-count failures (the "FAILED." in the
/// summary itself was matching).
///
/// Both stdout and stderr are scanned because cargo's summary location has
/// drifted between versions.
pub fn parse_test_counts(stdout: &str, stderr: &str) -> (usize, usize) {
    let mut passed = 0;
    let mut failed = 0;
    for stream in [stdout, stderr] {
        for line in stream.lines() {
            // Match the canonical summary line — robust to both
            //   "test result: ok. N passed; M failed; …"
            //   "test result: FAILED. N passed; M failed; …"
            if let Some(after) = line.find("test result:") {
                let tail = &line[after + "test result:".len()..];
                let toks: Vec<&str> = tail.split_whitespace().collect();
                for window in toks.windows(2) {
                    if window[1].starts_with("passed") {
                        if let Ok(n) = window[0].parse::<usize>() { passed += n; }
                    } else if window[1].starts_with("failed") {
                        if let Ok(n) = window[0].parse::<usize>() { failed += n; }
                    }
                }
            }
        }
    }
    (passed, failed)
}

#[cfg(test)]
mod parse_test_counts_tests {
    use super::parse_test_counts;

    #[test]
    fn single_binary_ok() {
        let stdout = "running 5 tests\n\
                      test foo ... ok\n\
                      test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n";
        assert_eq!(parse_test_counts(stdout, ""), (5, 0));
    }

    #[test]
    fn two_binaries_summed() {
        let stdout = "test result: ok. 10 passed; 0 failed; 0 ignored; …\n\
                      test result: ok. 7 passed; 2 failed; 0 ignored; …\n";
        assert_eq!(parse_test_counts(stdout, ""), (17, 2));
    }

    #[test]
    fn doesnt_double_count_failed_keyword() {
        // The "FAILED." after "test result:" used to be matched by the
        // per-line `contains("FAILED")` counter, inflating the failed count.
        let stdout = "test foo ... FAILED\n\
                      failures:\n    foo\n\
                      test result: FAILED. 3 passed; 1 failed; 0 ignored; …\n";
        assert_eq!(parse_test_counts(stdout, ""), (3, 1));
    }

    #[test]
    fn quiet_with_no_per_test_lines() {
        // --quiet emits dots/Fs + the final summary. We only care about the
        // summary, so we still get the right counts.
        let stdout = ".....F\n\
                      test result: FAILED. 5 passed; 1 failed; 0 ignored; …\n";
        assert_eq!(parse_test_counts(stdout, ""), (5, 1));
    }
}

// ── Shell-safety helpers (SEC-001/002/009/012 — docs/SECURITY_AUDIT_2026-06-10.md) ──
//
// Any handler that builds a REMOTE shell string (ssh `host` `cmd`) MUST pass
// every interpolated value through one of these. MCP tool args are
// agent-controlled input (incl. non-Claude siblings) — treat them as hostile.

/// POSIX single-quote `s` for safe interpolation into a shell command string.
/// (Same pattern as `fluxc-core::distributed::shell_quote`.)
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// True when `s` is a plausible ssh host (hostname / IPv4 / IPv6): blocks both
/// shell metacharacters and ssh option-injection (leading `-`).
pub fn safe_host(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '-' | '_'))
}

/// True when every char is in the conservative command-line-safe set
/// (alphanumeric, space, `/ . _ - =`). Permits "bash /path/to/script.sh"
/// while rejecting every shell metacharacter (`; | & $ > < ` ( ) { } * ? ! \n`).
pub fn safe_cmd_charset(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '/' | '.' | '_' | '-' | '='))
}

/// True when `s` is safe to embed as a URL path fragment in a shell string:
/// alphanumeric plus `/ . _ - ? = &`. No spaces, no quotes, no metacharacters.
pub fn safe_url_path(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '?' | '=' | '&'))
}

#[cfg(test)]
mod shell_safety_tests {
    use super::*;

    #[test]
    fn quote_neutralizes_single_quotes() {
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
        assert_eq!(shell_quote("x; rm -rf /"), "'x; rm -rf /'");
    }

    #[test]
    fn hosts() {
        assert!(safe_host("5.79.79.158"));
        assert!(safe_host("delta.internal"));
        assert!(safe_host("fe80::1"));
        assert!(!safe_host("-oProxyCommand=evil"));
        assert!(!safe_host("host;whoami"));
        assert!(!safe_host("host $(id)"));
        assert!(!safe_host(""));
    }

    #[test]
    fn cmd_charset() {
        assert!(safe_cmd_charset("bash /home/orobit/sigil-data/launch-delta.sh"));
        assert!(safe_cmd_charset("/usr/local/bin/sigil-node start --port=9501"));
        assert!(!safe_cmd_charset("bash x.sh; rm -rf /"));
        assert!(!safe_cmd_charset("bash x.sh && curl evil"));
        assert!(!safe_cmd_charset("$(reboot)"));
        assert!(!safe_cmd_charset("a`b`"));
        assert!(!safe_cmd_charset("a > /etc/passwd"));
    }

    #[test]
    fn url_paths() {
        assert!(safe_url_path("/posts/cb2d0a6d-4352-49a1-8e07-cd988341398a/comments"));
        assert!(safe_url_path("/verify"));
        assert!(safe_url_path("/agents/status?full=1&n=2"));
        assert!(!safe_url_path("/posts$(whoami)"));
        assert!(!safe_url_path("/x;curl evil"));
        assert!(!safe_url_path("/x'y"));
        assert!(!safe_url_path("/x y"));
    }
}

// ── Handler modules ──
pub mod build;
pub mod test_combo;
pub mod stats;
pub mod predict;
pub mod webhook;
pub mod session;
pub mod ops;
pub mod chronos;
pub mod frontend;
pub mod nodeswarm;
pub mod gateway;
pub mod bank;
pub mod sigil_cosmos;
pub mod agora_stargate;
pub mod dao_vm_dex;
pub mod zerox;
pub mod crosscompile;
pub mod sigil_combos;
pub mod sigil_ops;
pub mod flux_error;
pub mod molt;
pub mod wallet_xray;
pub mod platform_security;
pub mod platform_webhook;
pub mod aether;
pub mod fleet;
pub mod compile_error;
pub mod flux_legacy;
pub mod cortex;
pub mod swarm_compile;
