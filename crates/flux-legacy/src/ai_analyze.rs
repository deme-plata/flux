//! ai_analyze.rs — the corpus ⇄ DeepSeek bridge: reason over a WHOLE subsystem in one 1M-token call.
//!
//! [`corpus`](crate::corpus) (with [`flux_context`]) packs the highest-value Quillon code into a
//! ≤1M-token bundle. This module is the missing transport: hand that bundle + a question to a model
//! with a 1M context (deepseek-v4) and get back a concrete, source-cited answer — typically a
//! surgical `diff`. Where [`ai_refactor`](crate::ai_refactor) decomposes ONE file, this reasons over
//! the whole packed corpus (e.g. the entire sync subsystem) so the model sees real control flow, not
//! snippets.
//!
//! Same discipline as ai_refactor: the crate stays transport-free. [`analyze_prompt`] +
//! [`parse_analysis`] are PURE and unit-tested with no network; [`ai_analyze`] glues them via an
//! injected `call` closure, so the live DeepSeek HTTP lives in the bin. Propose-only — returns an
//! analysis + an optional patch, writes nothing.

use serde::{Deserialize, Serialize};

/// A model's answer over a corpus bundle, plus the first fenced patch it proposed (if any).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Analysis {
    /// the full prose answer (source-cited findings, explanation)
    pub answer: String,
    /// the first ```diff / ```rust fenced block, extracted for direct application via P4 verify
    pub proposed_patch: Option<String>,
}

/// Build the analysis prompt over a packed corpus bundle. PURE. The bundle is the
/// [`corpus::write_bundle`](crate::corpus::write_bundle) output (concatenated, highest-value-first).
pub fn analyze_prompt(bundle: &str, question: &str) -> String {
    format!(
        "You have the COMPLETE source of a Rust subsystem below (full files packed by flux-legacy, \
         highest-value first — use the full context to trace real control flow, not snippets). \
         Answer the QUESTION concretely: cite the exact functions and line-regions from the source, \
         explain the root cause, and when a code change is needed give a MINIMAL unified diff in a \
         single ```diff fenced block (surgical — do not rewrite whole files).\n\n\
         QUESTION:\n{question}\n\n\
         SOURCE BUNDLE:\n{bundle}"
    )
}

/// Extract the first fenced code block (```diff preferred, else ```rust, else any ```) from a reply.
/// PURE. Returns the inner content without the fences, or `None` if the reply has no fenced block.
pub fn extract_patch(reply: &str) -> Option<String> {
    for tag in ["```diff", "```rust", "```"] {
        if let Some(start) = reply.find(tag) {
            let after = &reply[start + tag.len()..];
            // skip to end of the opening fence line
            let body_start = after.find('\n').map(|i| i + 1).unwrap_or(0);
            let body = &after[body_start..];
            if let Some(end) = body.find("```") {
                let patch = body[..end].trim_end();
                if !patch.trim().is_empty() {
                    return Some(patch.to_string());
                }
            }
        }
    }
    None
}

/// Parse a model reply into an [`Analysis`]. PURE.
pub fn parse_analysis(reply: &str) -> Analysis {
    Analysis { answer: reply.trim().to_string(), proposed_patch: extract_patch(reply) }
}

/// Reason over a corpus `bundle` with `question`. `call` performs the model request (prompt ->
/// reply); injected so the crate needs no HTTP client and tests can mock it. Propose-only.
pub fn ai_analyze<F>(bundle: &str, question: &str, call: F) -> Result<Analysis, String>
where
    F: Fn(&str) -> Result<String, String>,
{
    let prompt = analyze_prompt(bundle, question);
    let reply = call(&prompt)?;
    if reply.trim().is_empty() {
        return Err("model returned an empty analysis".into());
    }
    Ok(parse_analysis(&reply))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_carries_question_and_bundle_and_asks_for_diff() {
        let p = analyze_prompt("===== FILE: a.rs =====\npub fn sync() {}", "why is it frozen?");
        assert!(p.contains("why is it frozen?"));
        assert!(p.contains("pub fn sync"));
        assert!(p.contains("```diff"));
    }

    #[test]
    fn extracts_diff_block() {
        let reply = "Root cause: stale anchor.\n\n```diff\n- let end = max_peer;\n+ let end = end.min(peer_h);\n```\nDone.";
        let patch = extract_patch(reply).unwrap();
        assert!(patch.contains("end.min(peer_h)"));
        assert!(!patch.contains("```"));
    }

    #[test]
    fn prefers_diff_over_rust_fence() {
        let reply = "```rust\nfn x(){}\n```\nand\n```diff\n+real patch\n```";
        assert_eq!(extract_patch(reply).unwrap(), "+real patch");
    }

    #[test]
    fn no_fence_yields_none() {
        assert!(extract_patch("just prose, no code block").is_none());
        assert!(extract_patch("```\n\n```").is_none()); // empty fence
    }

    #[test]
    fn ai_analyze_parses_answer_and_patch_via_injected_call() {
        let a = ai_analyze("bundle src", "fix the clamp", |prompt| {
            assert!(prompt.contains("fix the clamp"));
            Ok("The bug is the anchor.\n```diff\n+clamp here\n```".into())
        })
        .unwrap();
        assert!(a.answer.contains("anchor"));
        assert_eq!(a.proposed_patch.as_deref(), Some("+clamp here"));
    }

    #[test]
    fn ai_analyze_errors_on_empty() {
        assert!(ai_analyze("b", "q", |_| Ok("   ".into())).is_err());
        assert!(ai_analyze("b", "q", |_| Err("network down".into())).is_err());
    }
}
