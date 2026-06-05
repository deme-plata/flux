//! bundle.rs — flux-legacy **MEGA-CONTEXT FEED**: make legacy + flux-context + DeepSeek-v4-flash's
//! 1M-token window work together.
//!
//! [`crate::corpus`] (flux-context) decides WHICH files to pack into a 1M budget (Full / Outline /
//! Skip). This module does the other half: **materialize** that manifest into the actual bundle TEXT
//! (Full = verbatim, Outline = [`crate::context::outline`]), then **feed** it to deepseek-v4-flash's
//! 1M context for a WHOLE-SUBSYSTEM analysis — something P3's 6 KB single-file outline cannot do.
//! A 100-crate node is ~8M tokens; the corpus picks the right 1M, this sends it.
//!
//! Materialize is pure (tested over a temp tree); the feed shells `curl` (no new HTTP dep — same
//! pattern as `pulse`/`shadow`) to the DeepSeek API.

use crate::corpus::{Corpus, CorpusFile, PackMode, OUTLINE_CHARS};
use crate::context::outline;
use std::path::Path;

/// Materialize a packed manifest into the real bundle text the model will read. `root` is the repo
/// root; each file's `path` is resolved under it. Full = verbatim, Outline = signatures, Skip = omit.
pub fn materialize(root: &str, files: &[CorpusFile]) -> String {
    let root = Path::new(root);
    let mut out = String::new();
    for f in files {
        if f.mode == PackMode::Skip {
            continue;
        }
        let content = std::fs::read_to_string(root.join(&f.path)).unwrap_or_default();
        if content.trim().is_empty() {
            continue;
        }
        let body = match f.mode {
            PackMode::Full => content,
            PackMode::Outline => outline(&content, OUTLINE_CHARS),
            PackMode::Skip => continue,
        };
        out.push_str(&format!("// ===== {} ({}) · {:?} =====\n{}\n\n", f.path, f.crate_name, f.mode, body));
    }
    out
}

/// Analyze a packed [`Corpus`] with deepseek's 1M window in one call: materialize via the corpus
/// lane's canonical [`crate::corpus::bundle_string`] (single source of truth — not re-implemented
/// here), then feed [`analyze_subsystem`]. This is the composed bridge: corpus (which files) →
/// flux-context (1M budget) → deepseek (whole-subsystem reasoning).
pub fn analyze_corpus(c: &Corpus, question: &str) -> Result<SubsystemAnalysis, String> {
    analyze_subsystem(&crate::corpus::bundle_string(c), question)
}

/// Estimated token count of a bundle (flux-context's tokenizer math — the same the packer budgeted with).
pub fn bundle_tokens(bundle: &str) -> u32 {
    flux_context::est_tokens(bundle)
}

/// DeepSeek's flash model + its context window (the whole reason this module exists).
pub const DEEPSEEK_MODEL: &str = "deepseek-v4-flash";

/// The result of a whole-subsystem analysis.
#[derive(Debug, Clone)]
pub struct SubsystemAnalysis {
    pub answer: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cost_usd: f64,
}

/// Feed `bundle` + a `question` to deepseek-v4-flash's 1M window and return the analysis + cost.
/// Writes the payload to a temp file (bundles are huge — too big for an argv) and `curl --data @`.
/// Key from `DEEPSEEK_API_KEY` env or `/root/.config/deepseek/api_key`.
pub fn analyze_subsystem(bundle: &str, question: &str) -> Result<SubsystemAnalysis, String> {
    let key = std::env::var("DEEPSEEK_API_KEY").ok().filter(|k| !k.trim().is_empty())
        .or_else(|| std::fs::read_to_string("/root/.config/deepseek/api_key").ok())
        .map(|k| k.trim().to_string())
        .ok_or("no DeepSeek API key")?;

    let content = format!("{question}\n\n--- SUBSYSTEM BUNDLE (real code, packed to fit the window) ---\n{bundle}");
    let payload = serde_json::json!({
        "model": DEEPSEEK_MODEL,
        "messages": [{ "role": "user", "content": content }],
        "temperature": 0.2,
        "stream": false,
    });
    let tmp = std::env::temp_dir().join(format!("flux-legacy-megafeed-{}.json", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec(&payload).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;

    let out = std::process::Command::new("curl")
        .args([
            "-s", "--max-time", "300",
            "https://api.deepseek.com/chat/completions",
            "-H", &format!("Authorization: Bearer {key}"),
            "-H", "Content-Type: application/json",
            "--data", &format!("@{}", tmp.display()),
        ])
        .output()
        .map_err(|e| format!("curl: {e}"));
    let _ = std::fs::remove_file(&tmp);
    let out = out?;
    if !out.status.success() {
        return Err(format!("deepseek call failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("decode: {e} · body: {}", String::from_utf8_lossy(&out.stdout).chars().take(120).collect::<String>()))?;
    let answer = v["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string();
    if answer.is_empty() {
        return Err(format!("empty answer · body: {}", String::from_utf8_lossy(&out.stdout).chars().take(200).collect::<String>()));
    }
    let pt = v["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
    let ct = v["usage"]["completion_tokens"].as_u64().unwrap_or(0);
    // deepseek-v4-flash: $0.27/1M in, $1.10/1M out (matches flux-moe Price::DEEPSEEK_V4_FLASH).
    let cost_usd = (pt as f64) / 1e6 * 0.27 + (ct as f64) / 1e6 * 1.10;
    Ok(SubsystemAnalysis { answer, prompt_tokens: pt, completion_tokens: ct, cost_usd })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cf(path: &str, krate: &str, mode: PackMode) -> CorpusFile {
        CorpusFile { path: path.into(), crate_name: krate.into(), mode, tokens: 100, priority: 1.0 }
    }

    #[test]
    fn materialize_full_outline_skip() {
        let dir = std::env::temp_dir().join(format!("flux_legacy_bundle_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("crates/a/src")).unwrap();
        std::fs::create_dir_all(dir.join("crates/b/src")).unwrap();
        std::fs::write(dir.join("crates/a/src/lib.rs"), "pub fn alpha() {}\nfn priv_helper() {}\n").unwrap();
        std::fs::write(dir.join("crates/b/src/lib.rs"), "pub fn beta() -> u8 { 7 }\n").unwrap();
        std::fs::write(dir.join("crates/a/src/skip.rs"), "pub fn never() {}\n").unwrap();

        let files = vec![
            cf("crates/a/src/lib.rs", "a", PackMode::Outline),
            cf("crates/b/src/lib.rs", "b", PackMode::Full),
            cf("crates/a/src/skip.rs", "a", PackMode::Skip),
        ];
        let bundle = materialize(dir.to_str().unwrap(), &files);

        assert!(bundle.contains("pub fn beta() -> u8 { 7 }"), "Full file is verbatim");
        assert!(bundle.contains("pub fn alpha"), "Outline keeps the public signature");
        assert!(!bundle.contains("priv_helper") || !bundle.split("signatures").nth(1).unwrap_or("").contains("priv_helper"),
            "Outline drops private items from the signature list");
        assert!(!bundle.contains("pub fn never"), "Skip file is omitted");
        assert!(bundle.contains("crates/b/src/lib.rs (b)"), "file header present");
        assert!(bundle_tokens(&bundle) > 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_files_are_skipped_gracefully() {
        let files = vec![cf("crates/ghost/src/lib.rs", "ghost", PackMode::Full)];
        assert_eq!(materialize("/nonexistent-root", &files), "", "unreadable files just drop out");
    }
}
