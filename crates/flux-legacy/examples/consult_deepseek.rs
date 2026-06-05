//! P-corpus → DeepSeek v4 1M: analyze the WHOLE Quillon node in one shot.
//! Composes flux-legacy (rank by fan-in/role) + flux-context (pack to a token budget) + DeepSeek v4
//! (1M window). This is the "legacy + context work together, exploit the 1M context" hop that P9
//! drives autonomously. No new HTTP dep: the request JSON is built with serde_json (already a dep,
//! so the multi-MB bundle is escaped correctly) and POSTed via a `curl` subprocess.
//!
//!   DEEPSEEK_API_KEY=$(cat /root/.config/deepseek/api_key) \
//!   flux-cargo-wrapper run -p flux-legacy --example consult_deepseek -- <root> [window] [model]
//!
//! Sending the corpus PUBLISHES that source to the DeepSeek API — operator-gated (run only when asked).

use flux_legacy::analyze::analyze_workspace_legacy;
use flux_legacy::corpus::{build_corpus, bundle_string};
use std::process::Command;

const SYSTEM: &str = "You are a principal Rust architect reviewing a ~100-crate blockchain node \
(Quillon Graph). The user message is a token-budgeted bundle: the highest-fan-in crates + API \
surfaces VERBATIM, lower tiers as signature OUTLINES, the rest named-but-skipped (the manifest \
header lists every file's fate). Analyze the WHOLE node in one pass. Answer concisely, ALWAYS with \
specific crate/file names:\n\
1. The 3 biggest architectural risks or coupling problems.\n\
2. The single highest-leverage refactor (which god-file, and how to split it).\n\
3. Any cross-crate inconsistency or duplicated logic you can spot.\n\
4. One thing that looks dangerous for a blockchain specifically (consensus / balance / sync).";

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/home/orobit/qnk".into());
    let window: u32 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(1_000_000);
    let model = std::env::args().nth(3).unwrap_or_else(|| "deepseek-v4-flash".into());

    eprintln!("[1/3] analyze {root} …");
    let report = analyze_workspace_legacy(&root);
    eprintln!(
        "      {} crates · {} LOC · {} god-files",
        report.crate_count, report.total_loc, report.god_files.len()
    );

    eprintln!("[2/3] pack corpus → {window}-tok window …");
    let corpus = build_corpus(&report, window);
    let bundle = bundle_string(&corpus);
    eprintln!(
        "      {} full · {} outline · {} skipped · ~{} tok ({} chars)",
        corpus.full_count,
        corpus.outline_count,
        corpus.skipped_count,
        bundle.len() / 4,
        bundle.len()
    );

    let key = std::env::var("DEEPSEEK_API_KEY")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| std::fs::read_to_string("/root/.config/deepseek/api_key").ok().map(|s| s.trim().to_string()))
        .expect("need DEEPSEEK_API_KEY env or /root/.config/deepseek/api_key");

    let body = serde_json::json!({
        "model": model,
        "messages": [
            { "role": "system", "content": SYSTEM },
            { "role": "user", "content": bundle },
        ],
        "stream": false,
        "max_tokens": 4000,
        "temperature": 0.2
    });
    let req = "/tmp/quillon-deepseek-req.json";
    std::fs::write(req, serde_json::to_vec(&body).expect("serialize body")).expect("write req");

    eprintln!("[3/3] DeepSeek {model} — sending {} chars …", bundle.len());
    let out = Command::new("curl")
        .args([
            "-s", "--max-time", "600",
            "https://api.deepseek.com/chat/completions",
            "-H", &format!("Authorization: Bearer {key}"),
            "-H", "Content-Type: application/json",
            "--data-binary", &format!("@{req}"),
        ])
        .output()
        .expect("curl spawn");

    let resp = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value =
        serde_json::from_str(&resp).unwrap_or_else(|_| serde_json::json!({ "raw": resp }));
    match v.pointer("/choices/0/message/content").and_then(|c| c.as_str()) {
        Some(content) => {
            println!("\n═══ DeepSeek {model} — whole-node analysis ═══\n{content}");
            if let Some(u) = v.get("usage") {
                eprintln!("\nusage: {u}");
            }
        }
        None => {
            eprintln!("unexpected response: {}", resp.chars().take(900).collect::<String>());
            std::process::exit(1);
        }
    }
}
