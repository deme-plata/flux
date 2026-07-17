//! dispatch — wire the context Router (Task 4) to flux-moe model dispatch.
//!
//! The router (`router.rs`) picks a [`ModelTier`] + a ripple-prioritized slice of
//! crate-chunks within a token budget, but stops at the *plan*. This module closes
//! the loop (the doc's "Multi-model routing = flux-moe" enhancement): it reads the
//! selected crates' source, builds a context+task prompt, and sends it to the tier's
//! model via [`flux_moe::generate`] — which endpoint-switches between local **Ollama**
//! (qwen, the free Cheap tier) and the **DeepSeek** OpenAI-compatible API (Full /
//! Reasoning). Tier→endpoint/model is env-overridable so ops can repoint without a
//! rebuild.
//!
//! Note: `ModelTier::model()` returns Claude ids (haiku/sonnet/opus) as the *abstract*
//! tier label; the concrete dispatch substrate here is flux-moe's ollama/DeepSeek
//! switch, exactly as the plan specifies ("flux-moe can substitute local/DeepSeek").

use crate::chunk::ChunkManifest;
use crate::router::{ModelTier, RoutePlan};
use std::path::Path;

/// Resolve a tier to a concrete `(endpoint, model)` for `flux_moe::generate`.
/// `generate` routes by URL: an ollama host → `/api/generate`, anything else →
/// OpenAI `/chat/completions` (DeepSeek). Defaults match the flux-dev setup
/// (free local qwen for Cheap; DeepSeek chat/reasoner for Full/Reasoning).
///
/// Overrides: `FLUX_MOE_OLLAMA`, `FLUX_MOE_DEEPSEEK`,
/// `FLUX_MOE_{CHEAP,FULL,REASONING}_MODEL`.
pub fn tier_endpoint_model(tier: ModelTier) -> (String, String) {
    let deepseek = std::env::var("FLUX_MOE_DEEPSEEK")
        .unwrap_or_else(|_| "https://api.deepseek.com".to_string());
    match tier {
        ModelTier::Cheap => (
            std::env::var("FLUX_MOE_OLLAMA").unwrap_or_else(|_| "http://localhost:11434".to_string()),
            std::env::var("FLUX_MOE_CHEAP_MODEL").unwrap_or_else(|_| "qwen3.6:latest".to_string()),
        ),
        ModelTier::Full => (
            deepseek,
            std::env::var("FLUX_MOE_FULL_MODEL").unwrap_or_else(|_| "deepseek-chat".to_string()),
        ),
        ModelTier::Reasoning => (
            deepseek,
            std::env::var("FLUX_MOE_REASONING_MODEL").unwrap_or_else(|_| "deepseek-reasoner".to_string()),
        ),
    }
}

/// Collect a crate dir's `.rs` sources (sorted, `target/` skipped).
fn collect_rs(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().map(|n| n == "target" || n == ".flux-rev").unwrap_or(false) {
                    continue;
                }
                collect_rs(&p, out);
            } else if p.extension().map(|x| x == "rs").unwrap_or(false) {
                out.push(p);
            }
        }
    }
}

/// Concatenate a crate's sources, stopping once ~`max_tokens` is reached.
fn read_crate_src(crate_dir: &Path, max_tokens: u64) -> String {
    let mut files = Vec::new();
    collect_rs(crate_dir, &mut files);
    files.sort();
    let mut out = String::new();
    let mut tok = 0u64;
    for f in files {
        if tok >= max_tokens {
            break;
        }
        if let Ok(body) = std::fs::read_to_string(&f) {
            out.push_str(&format!("\n// ── {} ──\n", f.display()));
            tok += crate::est_tokens(&body) as u64;
            out.push_str(&body);
        }
    }
    out
}

/// Build the full context+task prompt from a [`RoutePlan`]'s selected crates
/// (ripple-DESC, capped at the plan's token budget).
pub fn build_prompt(manifest: &ChunkManifest, plan: &RoutePlan, task: &str) -> String {
    let ws = Path::new(&manifest.workspace);
    let sel: std::collections::HashSet<&str> = plan.selected.iter().map(|s| s.as_str()).collect();
    let mut ctx = String::new();
    let mut tok = 0u64;
    for c in &manifest.chunks {
        // manifest.chunks is pre-sorted ripple DESC, so the highest-impact crates lead.
        if !sel.contains(c.crate_name.as_str()) {
            continue;
        }
        if tok >= plan.budget_tokens {
            break;
        }
        let remaining = plan.budget_tokens.saturating_sub(tok);
        let src = read_crate_src(&ws.join(&c.path), remaining);
        ctx.push_str(&format!(
            "\n// ════ crate {} (category {:?}, ripple {:.2}) ════\n",
            c.crate_name, c.category, c.ripple_score
        ));
        ctx.push_str(&src);
        tok += c.estimated_tokens;
    }
    format!(
        "You are a Flux workspace engineering assistant. Below is the highest-ripple \
slice of the workspace selected for this task ({} crates, tier {:?}, ~{} ctx tokens), \
followed by the task. Use the context; be concrete and cite crate names.\n\n\
# CONTEXT\n{}\n\n# TASK\n{}\n",
        plan.selected.len(),
        plan.tier,
        tok,
        ctx,
        task
    )
}

/// Result of a dispatched route: which tier/model ran + the model's answer.
#[derive(Debug, Clone)]
pub struct DispatchResult {
    pub tier: ModelTier,
    pub endpoint: String,
    pub model: String,
    pub crates: usize,
    pub prompt_tokens_est: u64,
    pub answer: String,
}

/// Route → build prompt → dispatch to the tier's model via flux-moe. The whole
/// point of Task 4: turn a routing *decision* into an actual model call.
pub fn dispatch(manifest: &ChunkManifest, plan: &RoutePlan, task: &str) -> Result<DispatchResult, String> {
    let (endpoint, model) = tier_endpoint_model(plan.tier);
    let prompt = build_prompt(manifest, plan, task);
    let prompt_tokens_est = crate::est_tokens(&prompt) as u64;
    let answer = flux_moe::generate(&endpoint, &model, &prompt)?;
    Ok(DispatchResult {
        tier: plan.tier,
        endpoint,
        model,
        crates: plan.selected.len(),
        prompt_tokens_est,
        answer,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_maps_to_endpoint_and_model() {
        // defaults (no env overrides): Cheap=ollama/qwen, Full/Reasoning=deepseek
        std::env::remove_var("FLUX_MOE_OLLAMA");
        std::env::remove_var("FLUX_MOE_DEEPSEEK");
        std::env::remove_var("FLUX_MOE_CHEAP_MODEL");
        std::env::remove_var("FLUX_MOE_FULL_MODEL");
        std::env::remove_var("FLUX_MOE_REASONING_MODEL");
        let (ep, m) = tier_endpoint_model(ModelTier::Cheap);
        assert!(ep.contains("11434"), "cheap → local ollama");
        assert!(m.contains("qwen"), "cheap → qwen");
        let (ep, m) = tier_endpoint_model(ModelTier::Full);
        assert!(ep.contains("deepseek"), "full → deepseek api");
        assert_eq!(m, "deepseek-chat");
        let (_, m) = tier_endpoint_model(ModelTier::Reasoning);
        assert_eq!(m, "deepseek-reasoner");
    }

    #[test]
    fn env_override_repoints_without_rebuild() {
        std::env::set_var("FLUX_MOE_CHEAP_MODEL", "qwen3:4b");
        let (_, m) = tier_endpoint_model(ModelTier::Cheap);
        assert_eq!(m, "qwen3:4b");
        std::env::remove_var("FLUX_MOE_CHEAP_MODEL");
    }
}
