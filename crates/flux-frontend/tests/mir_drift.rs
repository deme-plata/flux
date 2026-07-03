// flux-frontend/tests/mir_drift.rs — FIP-0001 keep-A-open #2: the MIR-dialect drift contract,
// as a Rust test.
//
// Runs the PINNED rustc (flux_driver::RUSTC_VERSION) with --emit=mir over every mir-corpus/*.rs
// that has a committed *.mir.expected baseline, normalizes both sides exactly like
// mir-corpus/check.sh does (strip trailing whitespace, drop "// MIR for" banner lines, collapse
// absolute toolchain paths), and fails on ANY textual drift. rustc's --emit=mir dialect is the
// single contracted input of flux_frontend::mir::parse_mir — drift here means the frontend
// contract may be silently broken, so it must fail `fluxc test -p flux-frontend` / flux_combo
// locally, not just the .github/workflows/mir-diff.yml CI job.
//
// Skips cleanly (loud eprintln, pass) when no rustc is on PATH — e.g. a bare CI sandbox.
// When a rustc IS present it must be the pinned one; a mismatched toolchain is a hard failure
// (its MIR proves nothing about the contract).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("mir-corpus")
}

/// check.sh `normalize`: strip trailing whitespace, drop `// MIR for` banner lines,
/// collapse absolute rustc/toolchain paths to `<path>`.
fn normalize(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        let line = line.trim_end();
        if line.starts_with("// MIR for") {
            continue;
        }
        out.push_str(&normalize_paths(line));
        out.push('\n');
    }
    out
}

/// Rust twin of check.sh's `s#/[^ ]*/(rustc|toolchains)/[^ ]*#<path>#g`: any nonspace run
/// starting at a '/' whose tail contains '/rustc/' or '/toolchains/' collapses to `<path>`.
fn normalize_paths(line: &str) -> String {
    line.split(' ')
        .map(|tok| {
            if let Some(slash) = tok.find('/') {
                let tail = &tok[slash..];
                if tail.contains("/rustc/") || tail.contains("/toolchains/") {
                    return format!("{}<path>", &tok[..slash]);
                }
            }
            tok.to_string()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn mir_dialect_matches_pinned_baselines() {
    // Toolchain guard. stdin is force-nulled: on shared build boxes a concurrent feed-filter
    // process can race onto an inherited stdin pipe and poison the child (the epsilon
    // jq-on-stdin probe bug) — same defense as fluxc's wrapper probe passthrough.
    let ver_out = match Command::new("rustc")
        .arg("-V")
        .stdin(Stdio::null())
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => {
            eprintln!(
                "mir_drift: SKIP — no rustc on PATH (the contract needs the pinned {})",
                flux_driver::RUSTC_VERSION
            );
            return;
        }
    };
    let ver = String::from_utf8_lossy(&ver_out.stdout).to_string();
    assert!(
        ver.contains(flux_driver::RUSTC_VERSION),
        "MIR-drift contract must run against the PINNED rustc {} (flux_driver::RUSTC_VERSION); \
         found: {}. A different toolchain's MIR proves nothing about parse_mir's contract.",
        flux_driver::RUSTC_VERSION,
        ver.trim()
    );

    let corpus = corpus_dir();
    assert!(
        corpus.is_dir(),
        "mir-corpus/ not found at {} — the drift contract has no baselines",
        corpus.display()
    );
    let tmp = std::env::temp_dir().join(format!("flux-mir-drift-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("create tmp dir");

    let mut entries: Vec<PathBuf> = std::fs::read_dir(&corpus)
        .expect("read mir-corpus")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    entries.sort();

    let mut checked = 0usize;
    let mut drifts: Vec<String> = Vec::new();
    for src in entries {
        if src.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let base = src.file_stem().unwrap().to_string_lossy().to_string();
        let expected_path = corpus.join(format!("{}.mir.expected", base));
        if !expected_path.exists() {
            // A corpus sample without a committed baseline isn't part of the contract (yet).
            continue;
        }
        let mir_out = tmp.join(format!("{}.mir", base));
        let run = Command::new("rustc")
            .args(["--crate-type", "lib", "--emit=mir", "-o"])
            .arg(&mir_out)
            .arg(&src)
            .stdin(Stdio::null())
            .output()
            .expect("spawn pinned rustc");
        assert!(
            run.status.success(),
            "pinned rustc failed on corpus sample {}:\n{}",
            src.display(),
            String::from_utf8_lossy(&run.stderr)
        );
        let got = normalize(&std::fs::read_to_string(&mir_out).expect("read emitted MIR"));
        let want = normalize(&std::fs::read_to_string(&expected_path).expect("read baseline"));
        if got != want {
            drifts.push(format!(
                "=== {} drifted ===\n--- expected ({})\n{}\n--- got (rustc {})\n{}",
                base,
                expected_path.display(),
                want,
                flux_driver::RUSTC_VERSION,
                got
            ));
        }
        checked += 1;
        let _ = std::fs::remove_file(&mir_out);
    }
    let _ = std::fs::remove_dir_all(&tmp);

    assert!(
        checked >= 5,
        "mir-corpus should carry >=5 baselined samples, found {} — contract coverage collapsed",
        checked
    );
    assert!(
        drifts.is_empty(),
        "MIR DIALECT DRIFT vs pinned rustc {} — parse_mir's input contract may be broken.\n\
         Regenerate intentionally with mir-corpus/check.sh --update on the pinned toolchain.\n{}",
        flux_driver::RUSTC_VERSION,
        drifts.join("\n")
    );
    eprintln!("mir_drift: OK — {} corpus samples match the pinned {} dialect", checked, flux_driver::RUSTC_VERSION);
}

#[test]
fn normalization_matches_check_sh() {
    // The Rust normalize must agree with check.sh's sed for the cases the corpus can produce.
    assert_eq!(normalize("x = 1;   \n// MIR for `f`\ny = 2;"), "x = 1;\ny = 2;\n");
    assert_eq!(
        normalize_paths("at /home/u/.rustup/toolchains/1.93.1/lib/std.rs:1"),
        "at <path>"
    );
    assert_eq!(normalize_paths("no paths here"), "no paths here");
}
