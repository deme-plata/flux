// flux-report CLI: drives `flux_report::render_report` with file I/O and an
// optional pdflatex step. Designed to be run from anywhere — workspace root
// is auto-detected via fluxc-core's `workspace_root()` (so MCP-style cwds
// like /home/storage/claude-code work too).
//
// Usage:
//   flux-report --sources DIR [--output BASE] [--period "May 2026"] [--pdf]
//
// Defaults:
//   --sources  ./beta-docs/          (rsync target from Beta's docs/)
//   --output   project-report-YYYY-MM
//   --pdf      not set; emit .tex only

use flux_report::{render_report, ReportOptions};
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let opts = match parse_argv() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("flux-report: {e}");
            std::process::exit(2);
        }
    };

    let want_pdf = std::env::args().any(|a| a == "--pdf");
    let out_tex = PathBuf::from(format!("{}.tex", opts.base_name));
    let report = render_report(&opts);

    if let Err(e) = std::fs::write(&out_tex, &report.tex) {
        eprintln!("flux-report: cannot write {}: {e}", out_tex.display());
        std::process::exit(1);
    }
    println!(
        "✓ Rendered {} bytes → {} (Flux v{}, {} sources, {} swarm completions)",
        report.tex.len(),
        out_tex.display(),
        report.state.workspace_version,
        report.sources.len(),
        report.state.swarm.completed.len()
    );

    if want_pdf {
        run_pdflatex(&out_tex);
    }
}

fn parse_argv() -> Result<ReportOptions, String> {
    let mut args = std::env::args().skip(1).peekable();
    let mut sources: Option<PathBuf> = None;
    let mut base: Option<String> = None;
    let mut period: Option<String> = None;
    let mut title: Option<String> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--sources" => {
                sources = args.next().map(PathBuf::from);
            }
            "--output" => {
                base = args.next();
            }
            "--period" => {
                period = args.next();
            }
            "--title" => {
                title = args.next();
            }
            "--pdf" => { /* handled in main */ }
            "-h" | "--help" => {
                println!("flux-report — generate a Q-NarwhalKnight LaTeX project report");
                println!();
                println!("Usage:");
                println!(
                    "  flux-report --sources DIR [--output BASE] [--period \"May 2026\"] [--pdf]"
                );
                println!();
                println!("Defaults: --sources ./beta-docs/  --output project-report-YYYY-MM");
                std::process::exit(0);
            }
            other => return Err(format!("unknown arg `{other}` — try --help")),
        }
    }
    let sources = sources.unwrap_or_else(|| PathBuf::from("./beta-docs"));
    let workspace_root = fluxc_core::version::workspace_root();
    let now = chrono::Utc::now();
    let base = base.unwrap_or_else(|| format!("project-report-{}", now.format("%Y-%m")));

    let mut o = ReportOptions::default_for(workspace_root, sources, &base);
    if let Some(p) = period {
        o.period = p;
    }
    if let Some(t) = title {
        o.title = t;
    }
    Ok(o)
}

fn run_pdflatex(tex: &PathBuf) {
    // `parent()` of a bare relative path returns `Some("")`, which Command
    // treats as a missing directory (ENOENT). Coerce empty to "." so the
    // child process inherits our cwd.
    let cwd = match tex.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let basename = tex.file_name().and_then(|n| n.to_str()).unwrap_or("output.tex");
    // Two passes — first builds aux files, second resolves cross-refs +
    // hyperref bookmarks. -interaction=batchmode keeps output quiet but
    // still surfaces errors via exit code.
    for pass in 1..=2 {
        let status = Command::new("pdflatex")
            .arg("-interaction=batchmode")
            .arg("-halt-on-error")
            .arg(basename)
            .current_dir(&cwd)
            .status();
        match status {
            Ok(s) if s.success() => {
                println!("  ✓ pdflatex pass {pass} ok");
            }
            Ok(s) => {
                eprintln!("  ✗ pdflatex pass {pass} failed (status {s})");
                eprintln!("    check `{}.log` for details", basename.trim_end_matches(".tex"));
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("  ✗ could not invoke pdflatex: {e}");
                std::process::exit(1);
            }
        }
    }
    println!("✓ PDF emitted next to {}", tex.display());
}
