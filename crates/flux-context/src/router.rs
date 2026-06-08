//! router — multi-model context routing (Task 4 of `docs/1M_CONTEXT_WINDOW_PLAN.md`).
//!
//! Not every query needs the full 1M tokens. Given a task, pick a **model tier** and
//! fill that tier's token budget with the **highest-ripple** chunks — seeding
//! high-impact/core chunks first, then the task's category chunks by ripple. The tier
//! ladder maps onto the Anthropic models (and, via flux-moe, onto the local-qwen /
//! DeepSeek-V4 two-mind for the cheap/veto path).

use crate::chunk::{ChunkCategory, ChunkManifest, SemanticChunk};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelTier {
    Cheap,
    Full,
    Reasoning,
}

impl ModelTier {
    /// Suggested model id per tier (flux-moe can substitute local/DeepSeek for Cheap).
    pub fn model(&self) -> &'static str {
        match self {
            ModelTier::Cheap => "claude-haiku-4-5",
            ModelTier::Full => "claude-sonnet-4-6",
            ModelTier::Reasoning => "claude-opus-4-8",
        }
    }
}

/// Task archetypes from the plan's routing matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    ReadExplain,
    SimpleEdit,
    MultiFileRefactor,
    ArchReview,
    FullAudit,
    BuildDebug,
    SwarmCoord,
}

impl TaskKind {
    pub fn tier(&self) -> ModelTier {
        match self {
            TaskKind::ReadExplain
            | TaskKind::SimpleEdit
            | TaskKind::BuildDebug
            | TaskKind::SwarmCoord => ModelTier::Cheap,
            TaskKind::MultiFileRefactor | TaskKind::ArchReview => ModelTier::Full,
            TaskKind::FullAudit => ModelTier::Reasoning,
        }
    }

    pub fn budget_tokens(&self) -> u64 {
        match self {
            TaskKind::SwarmCoord => 4_000,
            TaskKind::ReadExplain => 8_000,
            TaskKind::BuildDebug => 8_000,
            TaskKind::SimpleEdit => 16_000,
            TaskKind::MultiFileRefactor => 64_000,
            TaskKind::ArchReview => 256_000,
            TaskKind::FullAudit => 1_000_000,
        }
    }

    /// Categories the task needs. Empty = every category is eligible.
    pub fn categories(&self) -> Vec<ChunkCategory> {
        use ChunkCategory::*;
        match self {
            TaskKind::FullAudit | TaskKind::ArchReview => vec![], // everything
            TaskKind::SwarmCoord => vec![Mcp, Sigil],
            _ => vec![Core],
        }
    }

    pub fn parse(s: &str) -> Option<TaskKind> {
        Some(match s.to_lowercase().replace(['-', ' '], "_").as_str() {
            "read" | "explain" | "read_explain" => TaskKind::ReadExplain,
            "edit" | "simple_edit" => TaskKind::SimpleEdit,
            "refactor" | "multi_file_refactor" => TaskKind::MultiFileRefactor,
            "review" | "arch_review" | "architecture" => TaskKind::ArchReview,
            "audit" | "full_audit" => TaskKind::FullAudit,
            "build" | "debug" | "build_debug" => TaskKind::BuildDebug,
            "swarm" | "swarm_coord" => TaskKind::SwarmCoord,
            _ => return None,
        })
    }
}

/// The resolved routing decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutePlan {
    pub kind: TaskKind,
    pub tier: ModelTier,
    pub model: String,
    pub budget_tokens: u64,
    pub categories: Vec<ChunkCategory>,
    pub selected: Vec<String>,
    pub used_tokens: u64,
    pub fill_ratio: f64,
}

pub struct ContextRouter<'a> {
    pub manifest: &'a ChunkManifest,
}

impl<'a> ContextRouter<'a> {
    pub fn new(manifest: &'a ChunkManifest) -> Self {
        Self { manifest }
    }

    /// Route a task to a tier + a ripple-prioritized chunk slice within budget.
    /// `extra_categories` widens the filter (e.g. a keyword match "p2p" → [P2p]).
    pub fn route(&self, kind: TaskKind, extra_categories: &[ChunkCategory]) -> RoutePlan {
        let tier = kind.tier();
        let budget = kind.budget_tokens();
        let mut cats = kind.categories();
        cats.extend_from_slice(extra_categories);
        let all = cats.is_empty(); // empty ⇒ every category eligible

        let chunks = &self.manifest.chunks; // pre-sorted ripple DESC
        let mut selected: Vec<&SemanticChunk> = Vec::new();
        let mut used = 0u64;

        // 1) seed: always include high-ripple / core chunks, capped at ⅓ budget so a
        //    single fat core crate can't crowd out the task-specific context.
        let seed_cap = budget / 3;
        for c in chunks {
            let is_seed = c.category == ChunkCategory::Core || c.ripple_score > 0.7;
            if is_seed && used + c.estimated_tokens <= seed_cap {
                used += c.estimated_tokens;
                selected.push(c);
            }
        }
        // 2) fill: task-category chunks by ripple, within the full budget.
        for c in chunks {
            if selected.iter().any(|s| s.crate_name == c.crate_name) {
                continue;
            }
            let cat_ok = all || cats.contains(&c.category);
            if cat_ok && used + c.estimated_tokens <= budget {
                used += c.estimated_tokens;
                selected.push(c);
            }
        }

        RoutePlan {
            kind,
            tier,
            model: tier.model().to_string(),
            budget_tokens: budget,
            categories: cats,
            selected: selected.iter().map(|c| c.crate_name.clone()).collect(),
            used_tokens: used,
            fill_ratio: if budget > 0 { used as f64 / budget as f64 } else { 0.0 },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::ChunkCategory::*;

    fn mk(name: &str, cat: ChunkCategory, ripple: f64, tok: u64) -> SemanticChunk {
        SemanticChunk {
            path: name.into(),
            crate_name: name.into(),
            category: cat,
            ripple_score: ripple,
            estimated_tokens: tok,
            blake3_hex: String::new(),
            mtime_ns: 0,
            deps: vec![],
            rev_deps: vec![],
        }
    }

    fn manifest(mut chunks: Vec<SemanticChunk>) -> ChunkManifest {
        chunks.sort_by(|a, b| b.ripple_score.partial_cmp(&a.ripple_score).unwrap());
        ChunkManifest {
            version: 1,
            workspace: "ws".into(),
            crate_count: chunks.len(),
            total_tokens_estimated: chunks.iter().map(|c| c.estimated_tokens).sum(),
            chunks,
        }
    }

    #[test]
    fn tiers_and_budgets() {
        assert_eq!(TaskKind::ReadExplain.tier(), ModelTier::Cheap);
        assert_eq!(TaskKind::MultiFileRefactor.tier(), ModelTier::Full);
        assert_eq!(TaskKind::FullAudit.tier(), ModelTier::Reasoning);
        assert!(TaskKind::FullAudit.budget_tokens() > TaskKind::ReadExplain.budget_tokens());
        assert_eq!(TaskKind::parse("refactor"), Some(TaskKind::MultiFileRefactor));
        assert_eq!(TaskKind::parse("nonsense"), None);
    }

    #[test]
    fn budget_is_respected_and_scales() {
        let m = manifest(vec![
            mk("fluxc-core", Core, 0.2, 5_000),
            mk("flux-p2p", P2p, 0.9, 6_000),
            mk("flux-cache", Other, 1.0, 7_000),
            mk("flux-frontend", Frontend, 0.1, 3_000),
        ]);
        let r = ContextRouter::new(&m);

        let cheap = r.route(TaskKind::ReadExplain, &[]);
        assert_eq!(cheap.tier, ModelTier::Cheap);
        assert!(cheap.used_tokens <= cheap.budget_tokens);

        let audit = r.route(TaskKind::FullAudit, &[]);
        assert_eq!(audit.tier, ModelTier::Reasoning);
        assert!(audit.used_tokens <= audit.budget_tokens);
        // a bigger budget never selects fewer chunks
        assert!(audit.selected.len() >= cheap.selected.len());
        assert!(audit.used_tokens >= cheap.used_tokens);
    }

    #[test]
    fn extra_category_widens_selection() {
        let m = manifest(vec![
            mk("fluxc-core", Core, 0.5, 1_000),
            mk("flux-p2p", P2p, 0.4, 1_000),
        ]);
        let r = ContextRouter::new(&m);
        // SimpleEdit defaults to [Core]; without extra, p2p is excluded.
        let base = r.route(TaskKind::SimpleEdit, &[]);
        assert!(!base.selected.iter().any(|n| n == "flux-p2p"));
        // widen with P2p → now included.
        let wide = r.route(TaskKind::SimpleEdit, &[P2p]);
        assert!(wide.selected.iter().any(|n| n == "flux-p2p"));
    }
}
