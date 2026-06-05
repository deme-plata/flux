// quillonos-claude — installable stub for an in-browser Claude client.
//
// Proves that arbitrary 3rd-party-named modules (here, "claude") ship
// through the QuillonOS pipeline cleanly: fluxc build → fluxc os-stage
// → manifest.json append → browser sha256-verifies → boot adds to
// STATE.verified → terminal command `claude` invokes the WASI binary
// via the existing runWasm path.
//
// v0.1 stub: prints what the real binary will do once Slice γ lands
// the HTTPS-fetch WASI shim. Once γ is in, this same crate grows the
// reqwest-style code to POST to api.anthropic.com/v1/messages.
//
// API key handling (when wired): read from QUILLONOS_ANTHROPIC_API_KEY
// env var the host shim sets (citizen pastes it once into a tiny UMG /
// stores it in OPFS at ~/.config/quillonos/claude.json).

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let kernel  = env::var("QUILLONOS_KERNEL").unwrap_or_else(|_| "wasi-preview1".into());
    let wallet  = env::var("QUILLONOS_AGENT_WALLET").unwrap_or_else(|_| "(none)".into());

    if args.iter().any(|a| a == "-V" || a == "--version") {
        println!("quillonos-claude {} (stub)", env!("CARGO_PKG_VERSION"));
        println!("kernel  {kernel}");
        println!("status  Anthropic API integration pending Slice \u{03b3} (q-wallet-web / fetch shim)");
        return ExitCode::SUCCESS;
    }

    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("claude — talk to Anthropic via QuillonOS (stub)");
        println!();
        println!("usage:");
        println!("  claude \"your prompt here\"");
        println!("  claude --version");
        println!("  claude --help");
        println!();
        println!("status:");
        println!("  v0.1 ships this as a stub — the network fetch path through WASI is");
        println!("  not yet wired (Slice γ). When γ lands, this same binary grows the");
        println!("  POST to api.anthropic.com/v1/messages with the QUILLONOS_ANTHROPIC_API_KEY");
        println!("  env var the host shim threads from your OPFS config.");
        return ExitCode::SUCCESS;
    }

    if args.is_empty() {
        eprintln!("claude: no prompt (try `claude \"hello\"` or `claude --help`)");
        return ExitCode::from(2);
    }

    let prompt = args.join(" ");
    println!("┌─ claude (QuillonOS stub) ───────────────────────────────");
    println!("│ kernel   {kernel}");
    println!("│ citizen  {}", short(&wallet));
    println!("│ model    claude-opus-4-7  (planned)");
    println!("│ prompt   {prompt}");
    println!("├──────────────────────────────────────────────────────────");
    println!("│ This is the stub. Once Slice \u{03b3} ships the WASI HTTPS fetch");
    println!("│ shim, the next invocation of this exact binary will reach");
    println!("│ api.anthropic.com and stream a real response into this");
    println!("│ same terminal. The module hash you booted with stays the");
    println!("│ same — only the host shim's network capability changes.");
    println!("└──────────────────────────────────────────────────────────");
    ExitCode::SUCCESS
}

fn short(w: &str) -> String {
    if w.len() < 12 { return w.into(); }
    format!("{}…{}", &w[..6], &w[w.len() - 4..])
}
