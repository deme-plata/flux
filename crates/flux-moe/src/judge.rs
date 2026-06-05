//! flux-moe v0.2 — the ensemble JUDGE (true voting).
//!
//! v0.1 ensemble returned BOTH answers labeled and let a human pick. v0.2 closes the loop:
//! two experts answer, then a `judge` model returns a VERDICT (winner + one-line rationale).
//! This is the qwen3.6-vs-deepseek-r1 "wrestle" the swarm runs — now a library primitive, so
//! the 2-of-2 gate is code, not vibes. Runs over the same distributed endpoints as `generate`.

use crate::generate;

/// The outcome of a 2-of-2 wrestle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// Model id that won.
    pub winner: String,
    /// Model id that lost.
    pub loser: String,
    /// The judge's one-line reasoning (raw).
    pub rationale: String,
    pub answer_a: String,
    pub answer_b: String,
}

/// Parse a judge's pick → `(winner, loser)` model ids. The judge is told to put EXACTLY
/// 'A' or 'B' on the first line; we tolerate "A.", "Answer A", "[B]", "B is better", etc.
/// Ties / unparseable default to A (deterministic — never panic, never silently swap).
pub fn parse_verdict<'a>(judge_out: &str, model_a: &'a str, model_b: &'a str) -> (&'a str, &'a str) {
    let first = judge_out
        .lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let up = first.to_uppercase();
    let picks_a = up.starts_with('A') || up.contains("ANSWER A") || up.contains("[A]") || up.contains(" A ") || up.contains(" A.");
    let picks_b = up.starts_with('B') || up.contains("ANSWER B") || up.contains("[B]") || up.contains(" B ") || up.contains(" B.");
    if picks_b && !picks_a {
        (model_b, model_a)
    } else {
        (model_a, model_b)
    }
}

/// 2-of-2 wrestle: `model_a` and `model_b` answer `prompt`; `judge_model` picks the winner.
/// All three calls go through [`generate`] (so they honor the ollama/vLLM think + keep-alive
/// fixes). Returns a [`Verdict`].
pub fn judge_pair(
    endpoint: &str,
    model_a: &str,
    model_b: &str,
    judge_model: &str,
    prompt: &str,
) -> Result<Verdict, String> {
    let answer_a = generate(endpoint, model_a, prompt).map_err(|e| format!("A({model_a}): {e}"))?;
    let answer_b = generate(endpoint, model_b, prompt).map_err(|e| format!("B({model_b}): {e}"))?;
    let jp = format!(
        "You are a strict, impartial judge. Question:\n{prompt}\n\n\
         Answer A:\n{answer_a}\n\nAnswer B:\n{answer_b}\n\n\
         Which answer is better? Reply with EXACTLY 'A' or 'B' on the first line, then ONE sentence why."
    );
    let rationale = generate(endpoint, judge_model, &jp).map_err(|e| format!("judge({judge_model}): {e}"))?;
    let (winner, loser) = parse_verdict(&rationale, model_a, model_b);
    Ok(Verdict {
        winner: winner.to_string(),
        loser: loser.to_string(),
        rationale,
        answer_a,
        answer_b,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdict_parsing_tolerates_shapes() {
        assert_eq!(parse_verdict("A\nbecause it is concrete", "qwen", "ds"), ("qwen", "ds"));
        assert_eq!(parse_verdict("B is clearer and correct", "qwen", "ds"), ("ds", "qwen"));
        assert_eq!(parse_verdict("[B]\nmore rigorous", "qwen", "ds"), ("ds", "qwen"));
        assert_eq!(parse_verdict("Answer A wins", "qwen", "ds"), ("qwen", "ds"));
        // unparseable → deterministic default to A (never panic)
        assert_eq!(parse_verdict("hmm, hard to say", "qwen", "ds"), ("qwen", "ds"));
        assert_eq!(parse_verdict("", "qwen", "ds"), ("qwen", "ds"));
    }

    #[test]
    fn verdict_winner_and_loser_are_distinct() {
        let (w, l) = parse_verdict("B", "qwen3.6", "deepseek-r1:70b");
        assert_eq!(w, "deepseek-r1:70b");
        assert_eq!(l, "qwen3.6");
        assert_ne!(w, l);
    }
}
