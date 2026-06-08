//! flux-context — measure & optimize the AI's context-window utilization.
//!
//! # The problem (why this crate exists)
//!
//! An agent has a ~1M-token context window, but most of it is either (a) filled
//! with *static boilerplate* that is loaded every session regardless of the task
//! (a multi-server runbook, a 100+ entry memory index, the full tool catalog), or
//! (b) left empty while the agent `Read`s source 200 lines at a time instead of
//! holding a whole subsystem resident. We optimize *output* tokens (combos) but
//! never measure whether the *loaded* context is actually used.
//!
//! You can't optimize what you don't measure. This crate measures it.
//!
//! # The metric
//!
//! **Context Utilization Ratio (CUR) = tokens_referenced / tokens_loaded**, taken
//! over the sources for which a "referenced" figure is known. A low CUR on a large
//! source = dead weight = a slice/defer/digest candidate.
//!
//! # Honesty boundary (what's real vs pending)
//!
//! - **Real & precise:** the *loaded* side. [`audit_paths`] reads the actual files
//!   and computes their token cost. [`ContextBudget`] math is exact.
//! - **Heuristic:** [`est_tokens`] is `chars/4`, not a real BPE tokenizer. Good to
//!   ±15% for English+code; swap in a real tokenizer later behind the same API.
//! - **Pending:** the *referenced* side. Knowing which loaded tokens were actually
//!   used requires the harness/model to report usage. v0 leaves `tokens_referenced`
//!   `Option`al; you set it via [`ContextBudget::mark_referenced`]. Until wired,
//!   CUR is computed only over sources you've marked.

use serde::{Deserialize, Serialize};

/// Semantic chunking + dependency-ripple scoring over the Flux workspace.
/// Task 1 of `docs/1M_CONTEXT_WINDOW_PLAN.md` — the "what to load" side that
/// complements this crate's CUR ("how well we used what we loaded"). Built on
/// flux-graph (no new manifest parser).
pub mod chunk;

/// Differential context updates (Task 2). Compares a fresh chunk manifest against
/// the last snapshot — only changed chunks need re-serializing. Fingerprints +
/// snapshot persistence reuse flux-rev (BLAKE3 + content-addressed Store).
pub mod diff;

/// Context-watch daemon (Task 3): poll-based filesystem watcher that keeps the
/// chunk manifest + diff hot in an L1 (/dev/shm) cache so the 1M context is ready
/// in <500ms. Cheap idle cost via an mtime-signature gate.
pub mod watch;

/// Multi-model context routing (Task 4): pick a model tier per task and fill its
/// token budget with the highest-ripple chunks. Tiers map onto the model ladder
/// (and flux-moe's local/DeepSeek two-mind for the cheap path).
pub mod router;

/// Default model context window (tokens). Override per-model if needed.
pub const DEFAULT_WINDOW_TOKENS: u32 = 1_000_000;

/// Heuristic token estimate: ~4 chars/token. NOT a real tokenizer — documented
/// as an estimate so call sites don't mistake it for ground truth.
pub fn est_tokens(text: &str) -> u32 {
    // chars (not bytes) so multibyte UTF-8 like 'æ' counts as one char, matching
    // how a tokenizer roughly sees it, then /4.
    ((text.chars().count() as f64) / 4.0).ceil() as u32
}

/// What category of context a source is — drives the optimization recommendation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContextKind {
    /// Harness/system prompt — fixed cost, not optimizable by us.
    SystemPrompt,
    /// Global instructions (e.g. CLAUDE.md) — sliceable by task domain.
    Runbook,
    /// Memory index (MEMORY.md) — top-K by relevance instead of dumping all.
    MemoryIndex,
    /// A single recalled memory body — already on-demand; keep.
    MemoryTopic,
    /// A skill document — digestible (load summary, expand on demand).
    SkillDoc,
    /// The deferred tool catalog — keep deferred; preload only task-relevant cluster.
    ToolCatalog,
    /// A file pulled into context via Read — the high-value, task-relevant kind.
    FileRead,
    /// Conversation history.
    Conversation,
    Other,
}

impl ContextKind {
    /// Is this category one the *agent* can shrink (vs fixed harness cost)?
    pub fn optimizable(self) -> bool {
        matches!(
            self,
            ContextKind::Runbook
                | ContextKind::MemoryIndex
                | ContextKind::SkillDoc
                | ContextKind::ToolCatalog
        )
    }
}

/// One source of loaded context, with its token cost and (optionally) how much of
/// it was actually referenced during the task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSource {
    pub name: String,
    pub kind: ContextKind,
    pub tokens_loaded: u32,
    /// `None` until the harness reports usage. See crate honesty boundary.
    pub tokens_referenced: Option<u32>,
}

impl ContextSource {
    pub fn new(name: impl Into<String>, kind: ContextKind, tokens_loaded: u32) -> Self {
        ContextSource { name: name.into(), kind, tokens_loaded, tokens_referenced: None }
    }

    /// Per-source utilization, if a referenced figure is known.
    pub fn utilization(&self) -> Option<f64> {
        match self.tokens_referenced {
            Some(r) if self.tokens_loaded > 0 => Some(r as f64 / self.tokens_loaded as f64),
            _ => None,
        }
    }

    /// Dead weight = optimizable, sizeable, and (known to be) under-referenced.
    /// Sources with unknown referencing are NOT called dead weight — we don't
    /// guess. `min_tokens` filters out trivially small sources.
    pub fn is_dead_weight(&self, util_threshold: f64, min_tokens: u32) -> bool {
        self.kind.optimizable()
            && self.tokens_loaded >= min_tokens
            && matches!(self.utilization(), Some(u) if u < util_threshold)
    }
}

/// The full context budget for a session/task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextBudget {
    pub sources: Vec<ContextSource>,
    pub window_tokens: u32,
}

impl Default for ContextBudget {
    fn default() -> Self {
        ContextBudget { sources: Vec::new(), window_tokens: DEFAULT_WINDOW_TOKENS }
    }
}

/// A concrete optimization suggestion for one source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recommendation {
    pub source: String,
    pub tokens_loaded: u32,
    pub lever: String,
    pub est_savings_tokens: u32,
}

impl ContextBudget {
    pub fn new(window_tokens: u32) -> Self {
        ContextBudget { sources: Vec::new(), window_tokens }
    }

    pub fn add(&mut self, source: ContextSource) -> &mut Self {
        self.sources.push(source);
        self
    }

    /// Record that `tokens` of the named source were actually referenced.
    pub fn mark_referenced(&mut self, name: &str, tokens: u32) -> bool {
        if let Some(s) = self.sources.iter_mut().find(|s| s.name == name) {
            s.tokens_referenced = Some(tokens.min(s.tokens_loaded));
            true
        } else {
            false
        }
    }

    pub fn total_loaded(&self) -> u32 {
        self.sources.iter().map(|s| s.tokens_loaded).sum()
    }

    /// Referenced total over sources where it's known.
    pub fn total_referenced_known(&self) -> u32 {
        self.sources.iter().filter_map(|s| s.tokens_referenced).sum()
    }

    /// Loaded total over sources where referencing is known (the CUR denominator).
    pub fn loaded_with_known_ref(&self) -> u32 {
        self.sources
            .iter()
            .filter(|s| s.tokens_referenced.is_some())
            .map(|s| s.tokens_loaded)
            .sum()
    }

    /// Context Utilization Ratio over the sources we have referencing data for.
    /// `None` if nothing has been marked yet (don't fabricate a number).
    pub fn cur(&self) -> Option<f64> {
        let denom = self.loaded_with_known_ref();
        if denom == 0 {
            None
        } else {
            Some(self.total_referenced_known() as f64 / denom as f64)
        }
    }

    /// How full the window is, 0.0..=1.0.
    pub fn fill_ratio(&self) -> f64 {
        if self.window_tokens == 0 {
            0.0
        } else {
            self.total_loaded() as f64 / self.window_tokens as f64
        }
    }

    pub fn dead_weight(&self, util_threshold: f64, min_tokens: u32) -> Vec<&ContextSource> {
        self.sources
            .iter()
            .filter(|s| s.is_dead_weight(util_threshold, min_tokens))
            .collect()
    }

    /// Optimization levers, largest-savings first. Each optimizable source over
    /// `min_tokens` gets a kind-specific lever and a conservative savings estimate.
    pub fn recommend(&self, min_tokens: u32) -> Vec<Recommendation> {
        let mut recs: Vec<Recommendation> = self
            .sources
            .iter()
            .filter(|s| s.kind.optimizable() && s.tokens_loaded >= min_tokens)
            .map(|s| {
                let (lever, frac) = match s.kind {
                    ContextKind::Runbook => ("slice to task-relevant sections", 0.80),
                    ContextKind::MemoryIndex => ("top-K by relevance, drop the rest", 0.75),
                    ContextKind::SkillDoc => ("load a digest, expand on demand", 0.60),
                    ContextKind::ToolCatalog => ("keep deferred; preload only the task cluster", 0.50),
                    _ => ("review", 0.0),
                };
                Recommendation {
                    source: s.name.clone(),
                    tokens_loaded: s.tokens_loaded,
                    lever: lever.to_string(),
                    est_savings_tokens: ((s.tokens_loaded as f64) * frac) as u32,
                }
            })
            .collect();
        recs.sort_by(|a, b| b.est_savings_tokens.cmp(&a.est_savings_tokens));
        recs
    }

    /// Human-readable report (visual — the operator is a visual learner).
    pub fn report(&self) -> String {
        let mut out = String::new();
        out.push_str("⚡ flux-context — context budget audit\n");
        let loaded = self.total_loaded();
        out.push_str(&format!(
            "  Window: {} tok · Loaded: {} tok ({:.1}% full) · {} sources\n",
            self.window_tokens,
            loaded,
            self.fill_ratio() * 100.0,
            self.sources.len()
        ));
        match self.cur() {
            Some(c) => out.push_str(&format!("  CUR (utilization): {:.0}%\n", c * 100.0)),
            None => out.push_str("  CUR: n/a (no referencing data marked yet)\n"),
        }
        out.push_str("  ── sources (by tokens) ──\n");
        let mut sorted: Vec<&ContextSource> = self.sources.iter().collect();
        sorted.sort_by(|a, b| b.tokens_loaded.cmp(&a.tokens_loaded));
        for s in sorted {
            let util = match s.utilization() {
                Some(u) => format!("{:.0}% used", u * 100.0),
                None => "ref?".to_string(),
            };
            out.push_str(&format!(
                "    {:<28} {:>7} tok  [{:?}] {}\n",
                s.name, s.tokens_loaded, s.kind, util
            ));
        }
        let recs = self.recommend(2000);
        if !recs.is_empty() {
            out.push_str("  ── optimize ──\n");
            for r in &recs {
                out.push_str(&format!(
                    "    {:<28} −{} tok  ({})\n",
                    r.source, r.est_savings_tokens, r.lever
                ));
            }
            let total: u32 = recs.iter().map(|r| r.est_savings_tokens).sum();
            out.push_str(&format!("  Reclaimable static budget: ~{} tok\n", total));
        }
        out
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// One entry to audit: (display name, kind, filesystem path).
pub type AuditEntry<'a> = (&'a str, ContextKind, &'a str);

/// Read each path and measure its *loaded* token cost (real, not estimated from
/// guesses — the bytes are on disk). Missing files are skipped (0 sources added),
/// so a partial environment still yields a valid partial budget.
pub fn audit_paths(entries: &[AuditEntry], window_tokens: u32) -> ContextBudget {
    let mut budget = ContextBudget::new(window_tokens);
    for (name, kind, path) in entries {
        if let Ok(text) = std::fs::read_to_string(path) {
            budget.add(ContextSource::new(*name, *kind, est_tokens(&text)));
        }
    }
    budget
}

/// Infer a [`ContextKind`] from a file name or path — used by the args API so a
/// caller can pass bare paths and still get the right optimize lever. Order
/// matters: CLAUDE.md is a Runbook even though it ends in ".md" like a skill.
pub fn kind_from_name(name: &str) -> ContextKind {
    let n = name.to_lowercase();
    if n.contains("claude.md") {
        ContextKind::Runbook
    } else if n.contains("memory") {
        ContextKind::MemoryIndex
    } else if n.contains("tools") {
        ContextKind::ToolCatalog
    } else if n.contains("skill") {
        ContextKind::SkillDoc
    } else {
        ContextKind::FileRead
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn est_tokens_basic() {
        assert_eq!(est_tokens(""), 0);
        // 8 chars / 4 = 2
        assert_eq!(est_tokens("abcdefgh"), 2);
        // multibyte char counts as one char, not its byte length
        assert_eq!(est_tokens("ææææ"), 1); // 4 chars / 4
    }

    #[test]
    fn cur_is_none_until_marked() {
        let mut b = ContextBudget::default();
        b.add(ContextSource::new("CLAUDE.md", ContextKind::Runbook, 20_000));
        assert!(b.cur().is_none(), "no referencing => no fabricated CUR");
        assert_eq!(b.total_loaded(), 20_000);
    }

    #[test]
    fn cur_computed_over_marked_sources() {
        let mut b = ContextBudget::default();
        b.add(ContextSource::new("CLAUDE.md", ContextKind::Runbook, 20_000));
        b.add(ContextSource::new("flux-frontend.rs", ContextKind::FileRead, 10_000));
        b.mark_referenced("CLAUDE.md", 2_000); // 10% of the runbook used
        b.mark_referenced("flux-frontend.rs", 9_000); // 90% of the source used
        // CUR = (2000+9000) / (20000+10000) = 11000/30000 ≈ 0.366
        let cur = b.cur().unwrap();
        assert!((cur - 0.3667).abs() < 0.01, "cur was {cur}");
    }

    #[test]
    fn dead_weight_needs_known_low_utilization() {
        let mut b = ContextBudget::default();
        b.add(ContextSource::new("CLAUDE.md", ContextKind::Runbook, 20_000));
        // Unknown referencing: NOT dead weight (we don't guess).
        assert!(b.dead_weight(0.3, 2_000).is_empty());
        b.mark_referenced("CLAUDE.md", 1_000); // 5% utilization
        let dw = b.dead_weight(0.3, 2_000);
        assert_eq!(dw.len(), 1);
        assert_eq!(dw[0].name, "CLAUDE.md");
    }

    #[test]
    fn high_utilization_is_not_dead_weight() {
        let mut b = ContextBudget::default();
        b.add(ContextSource::new("skill", ContextKind::SkillDoc, 10_000));
        b.mark_referenced("skill", 8_000); // 80% used
        assert!(b.dead_weight(0.3, 2_000).is_empty());
    }

    #[test]
    fn fileread_is_not_optimizable_away() {
        // The high-value kind: pulling task-relevant source IS the good use of
        // the window. recommend() must never propose shrinking a FileRead.
        let mut b = ContextBudget::default();
        b.add(ContextSource::new("flux-backend.rs", ContextKind::FileRead, 30_000));
        assert!(b.recommend(2_000).is_empty());
    }

    #[test]
    fn recommend_sorts_by_savings_desc() {
        let mut b = ContextBudget::default();
        b.add(ContextSource::new("CLAUDE.md", ContextKind::Runbook, 20_000)); // .80 => 16000
        b.add(ContextSource::new("MEMORY.md", ContextKind::MemoryIndex, 7_000)); // .75 => 5250
        b.add(ContextSource::new("skill", ContextKind::SkillDoc, 10_000)); // .60 => 6000
        let recs = b.recommend(2_000);
        assert_eq!(recs.len(), 3);
        assert_eq!(recs[0].source, "CLAUDE.md");
        assert!(recs[0].est_savings_tokens >= recs[1].est_savings_tokens);
        assert!(recs[1].est_savings_tokens >= recs[2].est_savings_tokens);
    }

    #[test]
    fn fill_ratio_and_report() {
        let mut b = ContextBudget::new(1_000_000);
        b.add(ContextSource::new("CLAUDE.md", ContextKind::Runbook, 20_000));
        assert!((b.fill_ratio() - 0.02).abs() < 1e-9);
        let r = b.report();
        assert!(r.contains("flux-context"));
        assert!(r.contains("CLAUDE.md"));
        assert!(r.contains("optimize"));
    }

    #[test]
    fn audit_paths_reads_real_files() {
        let dir = std::env::temp_dir().join(format!("flux-context-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("fake_claude.md");
        std::fs::write(&p, "x".repeat(4000)).unwrap(); // ~1000 tokens
        let entries = [("CLAUDE.md", ContextKind::Runbook, p.to_str().unwrap())];
        let b = audit_paths(&entries, DEFAULT_WINDOW_TOKENS);
        assert_eq!(b.sources.len(), 1);
        assert_eq!(b.sources[0].tokens_loaded, 1000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn kind_from_name_infers_correctly() {
        assert_eq!(kind_from_name("/root/.claude/CLAUDE.md"), ContextKind::Runbook);
        assert_eq!(kind_from_name("MEMORY.md"), ContextKind::MemoryIndex);
        assert_eq!(kind_from_name("flux-dev/TOOLS.md"), ContextKind::ToolCatalog);
        assert_eq!(kind_from_name("skills/sigil/SKILL.md"), ContextKind::SkillDoc);
        assert_eq!(kind_from_name("crates/flux-backend/src/lib.rs"), ContextKind::FileRead);
        // CLAUDE.md wins over the generic ".md" → SkillDoc fallthrough.
        assert_eq!(kind_from_name("CLAUDE.md"), ContextKind::Runbook);
    }

    #[test]
    fn audit_paths_skips_missing() {
        let entries = [("nope", ContextKind::Runbook, "/no/such/path/xyz.md")];
        let b = audit_paths(&entries, DEFAULT_WINDOW_TOKENS);
        assert!(b.sources.is_empty());
    }
}
