//! context.rs — flux-legacy **P3: CONTEXT INJECTION**.
//!
//! A P2 [`RefactorBrief`] is generic — "split god-file X", "add tests to crate Y" — so a code model
//! asked to act on it invents plausible-but-wrong code. P3 GROUNDS the brief in the REAL target:
//! it reads the actual `.rs` source, extracts a bounded outline (file head + public signatures)
//! within a char budget, and folds it into the prompt so the flux-moe pipeline refactors what is
//! genuinely there. Pure over an injected source string (testable); the caller does the file I/O.

use crate::execute::RefactorBrief;

/// Max chars of real code embedded in a grounded prompt — keeps the proposer within a sane token
/// budget on big god-files (the whole point is bounded context, not the whole 3000-LOC file).
pub const DEFAULT_BUDGET_CHARS: usize = 6000;

/// Extract a bounded outline from real Rust source: the file head (first lines, for module docs +
/// imports) plus every public signature (`pub fn` / `pub struct` / `pub enum` / `pub trait` /
/// `impl`). Truncated to `budget` chars so a 3000-LOC god-file still yields a promptable summary.
pub fn outline(src: &str, budget: usize) -> String {
    let mut sigs = Vec::new();
    for line in src.lines() {
        let t = line.trim_start();
        if t.starts_with("pub fn ")
            || t.starts_with("pub async fn ")
            || t.starts_with("pub struct ")
            || t.starts_with("pub enum ")
            || t.starts_with("pub trait ")
            || t.starts_with("pub const ")
            || t.starts_with("impl ")
        {
            sigs.push(line.trim_end().to_string());
        }
    }
    let head: String = src.lines().take(40).collect::<Vec<_>>().join("\n");
    let mut out = format!(
        "// --- file head ---\n{head}\n// --- public signatures ({}) ---\n{}",
        sigs.len(),
        sigs.join("\n")
    );
    if out.len() > budget {
        // truncate at the nearest char boundary ≤ budget — real source has multi-byte chars
        // (emoji/UTF-8 in comments+strings); a raw byte truncate panics (is_char_boundary).
        let mut end = budget.min(out.len());
        while end > 0 && !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
        out.push_str("\n// …(truncated to budget)");
    }
    out
}

/// Ground a brief in the real target code: append the bounded [`outline`] to its prompt so the
/// proposer works from THESE items, not invented ones. Returns a new, grounded brief (the original
/// is unchanged). All other brief fields (acceptance, budget, est) carry over.
pub fn inject(brief: &RefactorBrief, target_src: &str, budget: usize) -> RefactorBrief {
    let ctx = outline(target_src, budget);
    let mut grounded = brief.clone();
    grounded.prompt = format!(
        "{}\n\nHere is the ACTUAL code to work from (bounded outline of the real file):\n\
         ```rust\n{}\n```\n\
         Base your output STRICTLY on these real items — preserve existing public names/signatures, \
         do not invent APIs that aren't shown.",
        brief.prompt, ctx
    );
    grounded
}

/// Convenience: is a grounded brief safe to feed the pipeline's standalone rustc-gate? A grounded
/// refactor that references the crate's own items won't compile in isolation — surface that so the
/// caller (or P-next) can route it to an in-crate verify instead of the standalone gate.
pub fn gateable_standalone(brief: &RefactorBrief) -> bool {
    // add-tests / split-skeleton can be std-only; decouple + grounded edits usually need crate ctx.
    matches!(brief.kind.as_str(), "add-tests" | "split-god-file")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execute::brief_for;
    use crate::RefactorTask;

    fn task(kind: &str) -> RefactorTask {
        RefactorTask {
            rank: 1,
            crate_name: "q-storage".into(),
            kind: kind.into(),
            target: "src/lib.rs".into(),
            detail: "big file".into(),
            impact: 0.9,
            effort: "high".into(),
            est_minutes: 120,
        }
    }

    const SRC: &str = "//! my module\nuse std::io;\n\npub fn alpha(x: u64) -> u64 { x + 1 }\n\
        fn private_helper() {}\npub struct Cfg { pub n: u32 }\npub trait Sink { fn put(&self); }\n\
        impl Sink for Cfg { fn put(&self) {} }\n";

    #[test]
    fn outline_extracts_public_signatures_only() {
        let o = outline(SRC, DEFAULT_BUDGET_CHARS);
        assert!(o.contains("pub fn alpha"));
        assert!(o.contains("pub struct Cfg"));
        assert!(o.contains("pub trait Sink"));
        assert!(o.contains("impl Sink for Cfg"));
        // private items may appear in the file head, but must NOT be advertised as signatures
        let sigs = o.split("public signatures").nth(1).unwrap_or("");
        assert!(!sigs.contains("private_helper"), "private items are not advertised as signatures");
    }

    #[test]
    fn outline_respects_char_budget() {
        let big = "pub fn f() {}\n".repeat(1000);
        let o = outline(&big, 300);
        assert!(o.len() <= 360, "must truncate near budget (got {})", o.len());
        assert!(o.contains("truncated"));
    }

    #[test]
    fn inject_grounds_prompt_in_real_code() {
        let b = brief_for(&task("split-god-file"));
        let g = inject(&b, SRC, DEFAULT_BUDGET_CHARS);
        assert!(g.prompt.contains("ACTUAL code"));
        assert!(g.prompt.contains("pub fn alpha"), "real signature embedded");
        assert!(g.prompt.len() > b.prompt.len(), "grounding adds the outline");
        assert_eq!(g.acceptance, b.acceptance, "other fields preserved");
        assert_eq!(g.budget_usd, b.budget_usd);
    }

    #[test]
    fn standalone_gate_routing() {
        assert!(gateable_standalone(&brief_for(&task("add-tests"))));
        assert!(!gateable_standalone(&brief_for(&task("decouple"))));
    }
}
