//! flux-moe **builder** — "qwen3.6 helps build through flux-moe".
//!
//! Hand flux-moe a build goal; it routes the goal to the coder expert (qwen3.6 by default) through
//! the think-aware [`crate::generate`] path and returns *compile-ready* Rust (the fenced block,
//! unwrapped). This closes the loop: the same router that answers prompts now contributes code.
//!
//! **Dogfood provenance:** the [`extract_rust`] core was drafted by **qwen3.6 itself, through
//! flux-moe** (`FLUX_MOE_MODEL=qwen3.6 flux-moe "..."`, 2026-06-03, free local Epsilon CPU). It was
//! then reviewed + bug-fixed at the 2-of-2 gate: the model wrote `start.unwrap()` inside the
//! `rust_end` branch where `start` was out of scope — corrected to `rust_start.unwrap()`. That
//! review step is the point: the expert proposes, the gate verifies, only green code lands.

/// A build request handed to the expert pool.
#[derive(Debug, Clone)]
pub struct BuildTask {
    /// Package the work targets (for the engineer prompt context).
    pub package: String,
    /// What to build, in plain language.
    pub goal: String,
}

/// What the coder expert produced for a [`BuildTask`].
#[derive(Debug, Clone)]
pub struct BuildProposal {
    /// Which expert model authored it.
    pub model: String,
    /// Compile-ready Rust, extracted from the reply's fenced block.
    pub code: String,
    /// The full untrimmed reply (kept for audit / the review gate).
    pub raw: String,
}

/// Extract the first fenced ```` ```rust ```` block from an LLM reply and return its inner contents,
/// trimmed. Falls back to the first generic ```` ``` ```` block, then to the whole input trimmed.
/// std-only.
///
/// Drafted by qwen3.6 via flux-moe, reviewed + fixed (see module docs).
pub fn extract_rust(reply: &str) -> String {
    let trimmed = reply.trim();
    let rust_start = trimmed.find("```rust");
    let rust_end = if let Some(start) = rust_start {
        let after_start = &trimmed[start + 7..];
        after_start.find("```").map(|e| start + 7 + e)
    } else {
        None
    };

    if let Some(end) = rust_end {
        // fix: `rust_start` is the bound Option here, not `start` (qwen3.6's draft said `start`).
        return trimmed[(rust_start.unwrap() + 7)..end].trim().to_string();
    }

    let generic_start = trimmed.find("```");
    if let Some(start) = generic_start {
        let after_start = &trimmed[start + 3..];
        if let Some(end) = after_start.find("```") {
            return trimmed[(start + 3)..(start + 3 + end)].trim().to_string();
        }
    }

    trimmed.to_string()
}

/// The engineer prompt sent to the coder expert. Pure (testable) so the contract is pinned.
pub fn build_prompt(task: &BuildTask) -> String {
    format!(
        "You are a senior Rust engineer on the Flux compiler project. \
         Target package: `{}`. Implement the following as idiomatic, std-only Rust \
         (no external crates unless already in the package). Goal: {}\n\n\
         Output ONLY the code in a single ```rust block. No prose, no explanation.",
        task.package, task.goal
    )
}

/// Live: ask the coder expert (e.g. `qwen3.6`) — through flux-moe's think-aware [`crate::generate`] —
/// to write code for `task`, and return the compile-ready extraction.
pub fn assist(endpoint: &str, model: &str, task: &BuildTask) -> Result<BuildProposal, String> {
    let raw = crate::generate(endpoint, model, &build_prompt(task))?;
    let code = extract_rust(&raw);
    if code.trim().is_empty() {
        return Err("coder expert returned an empty proposal".into());
    }
    Ok(BuildProposal { model: model.to_string(), code, raw })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_rust_fence() {
        let reply = "Sure:\n```rust\nfn x() -> i32 { 42 }\n```\nDone.";
        assert_eq!(extract_rust(reply), "fn x() -> i32 { 42 }");
    }

    #[test]
    fn falls_back_to_generic_fence() {
        let reply = "```\nlet y = 1;\n```";
        assert_eq!(extract_rust(reply), "let y = 1;");
    }

    #[test]
    fn falls_back_to_whole_text() {
        assert_eq!(extract_rust("  fn bare() {}  "), "fn bare() {}");
    }

    #[test]
    fn rust_fence_preferred_over_generic() {
        // a stray generic fence before the rust fence must not win
        let reply = "```\nignored\n```\nthen\n```rust\nfn keep() {}\n```";
        assert_eq!(extract_rust(reply), "fn keep() {}");
    }

    #[test]
    fn prompt_carries_goal_and_package() {
        let p = build_prompt(&BuildTask { package: "flux-moe".into(), goal: "add a foo()".into() });
        assert!(p.contains("flux-moe"));
        assert!(p.contains("add a foo()"));
        assert!(p.contains("```rust"));
    }
}
