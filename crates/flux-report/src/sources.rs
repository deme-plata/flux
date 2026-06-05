// Source-doc loader: walks a directory of markdown reports (typically
// Beta's `/opt/orobit/shared/q-narwhalknight/docs/`, rsync'd locally), picks
// out the ones that look like project / incident / technical-review reports,
// and digests each one to (title, first paragraph) for the report's "What
// changed this month" section.
//
// We intentionally don't summarize via LLM here — the digest is structural,
// so generating reports is deterministic and reproducible. v2 can layer
// `flux-ai` LLM summarization behind a `--summarize` flag.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceDigest {
    /// Relative path (from source root), with the leading directory stripped
    /// for readability.
    pub relative_path: String,
    pub title: String,
    /// First paragraph after the title. Empty string if the file has no body
    /// after the header.
    pub lead_paragraph: String,
    pub category: SourceCategory,
    /// Bytes in the source file — coarse proxy for "how much work was logged".
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceCategory {
    TechnicalReview,
    IncidentReport,
    ProjectReport,
    Handoff,
    Plan,
    Other,
}

impl SourceCategory {
    pub fn label(self) -> &'static str {
        match self {
            SourceCategory::TechnicalReview => "Technical Review",
            SourceCategory::IncidentReport => "Incident Report",
            SourceCategory::ProjectReport => "Project Report",
            SourceCategory::Handoff => "Handoff",
            SourceCategory::Plan => "Plan",
            SourceCategory::Other => "Other",
        }
    }

    fn classify(name: &str) -> Self {
        let n = name.to_lowercase();
        if n.contains("technical-review") || n.starts_with("tr-") {
            Self::TechnicalReview
        } else if n.contains("incident-report") || n.contains("emergency") {
            Self::IncidentReport
        } else if n.contains("project-report") || n.contains("monthly-report") {
            Self::ProjectReport
        } else if n.contains("handoff") {
            Self::Handoff
        } else if n.starts_with("technical-plan") || n.contains("-plan-") || n.contains("plan-")
        {
            Self::Plan
        } else {
            Self::Other
        }
    }
}

/// Walk `root` recursively, return one digest per markdown file. Skips
/// generated artifacts (`.aux`, `.log`, `.out`, `.pdf`) and hidden files.
pub fn load_sources(root: &Path) -> Vec<SourceDigest> {
    if !root.exists() {
        return vec![];
    }
    let mut out = Vec::new();
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if name.starts_with('.') {
            continue;
        }
        if !name.ends_with(".md") && !name.ends_with(".MD") {
            continue;
        }
        let bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
        let body = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let (title, lead) = digest_markdown(&body);
        let relative_path = relative_to(root, path);
        out.push(SourceDigest {
            relative_path,
            title,
            lead_paragraph: lead,
            category: SourceCategory::classify(name),
            bytes,
        });
    }
    out.sort_by(|a, b| {
        a.category
            .label()
            .cmp(b.category.label())
            .then(a.relative_path.cmp(&b.relative_path))
    });
    out
}

fn relative_to(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string_lossy().to_string())
}

/// Extract a (title, lead_paragraph) pair from a markdown body. The title is
/// the first `# Heading` (or the bare first non-empty line if there is no
/// heading); the lead is the first non-empty paragraph after the title.
pub fn digest_markdown(body: &str) -> (String, String) {
    let mut lines = body.lines().map(|l| l.trim_end());
    let mut title = String::new();
    while let Some(line) = lines.next() {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('#') {
            title = rest.trim_start_matches('#').trim().to_string();
        } else {
            title = line.trim().to_string();
        }
        break;
    }
    // Walk past blank lines to find the first paragraph that isn't a
    // metadata block ("**Author:**", front-matter, etc.).
    let mut paragraph = String::new();
    let mut in_para = false;
    for line in lines {
        let t = line.trim();
        if t.is_empty() {
            if in_para {
                break;
            }
            continue;
        }
        if t.starts_with("**Author:")
            || t.starts_with("**Date:")
            || t.starts_with("**Session length:")
            || t.starts_with("---")
        {
            continue;
        }
        in_para = true;
        if !paragraph.is_empty() {
            paragraph.push(' ');
        }
        paragraph.push_str(t.trim_start_matches('#').trim());
    }
    (title, paragraph)
}

/// Truncate to `max_chars` chars, ending on a word boundary, with `…` if cut.
pub fn truncate_to_words(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max_chars).collect();
    match truncated.rfind(' ') {
        Some(idx) => format!("{}…", &truncated[..idx]),
        None => format!("{truncated}…"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_technical_review_by_filename() {
        assert_eq!(
            SourceCategory::classify("technical-review-balance-bounce-after-swap-v1.md"),
            SourceCategory::TechnicalReview
        );
        assert_eq!(
            SourceCategory::classify("TR-2026-004-x-algorithm-inspiration.md"),
            SourceCategory::TechnicalReview
        );
        assert_eq!(
            SourceCategory::classify("incident-report-balance-replay-2026-05-09.md"),
            SourceCategory::IncidentReport
        );
        assert_eq!(
            SourceCategory::classify("project-report-2026-05.md"),
            SourceCategory::ProjectReport
        );
        assert_eq!(
            SourceCategory::classify("technical-plan-instant-bootstrap-recursive-snark.md"),
            SourceCategory::Plan
        );
        assert_eq!(
            SourceCategory::classify("DEEPSEEK_MCP_HANDOFF.md"),
            SourceCategory::Handoff
        );
        assert_eq!(SourceCategory::classify("random.md"), SourceCategory::Other);
    }

    #[test]
    fn digest_picks_h1_and_first_paragraph() {
        let body = "# Foo Bar\n\n**Author:** Someone\n\nThe quick brown fox.\nJumped.\n\nNext.";
        let (t, p) = digest_markdown(body);
        assert_eq!(t, "Foo Bar");
        assert_eq!(p, "The quick brown fox. Jumped.");
    }

    #[test]
    fn digest_handles_no_heading() {
        let body = "just a sentence\n\nanother.";
        let (t, _p) = digest_markdown(body);
        assert_eq!(t, "just a sentence");
    }

    #[test]
    fn truncate_stops_at_word_boundary() {
        let s = "The quick brown fox jumps over the lazy dog";
        let t = truncate_to_words(s, 20);
        assert!(t.ends_with("…"));
        assert!(t.len() <= 24);
        assert!(!t.contains("fox jumps over the")); // truncated mid-word would
    }

    #[test]
    fn nonexistent_root_returns_empty() {
        assert!(load_sources(Path::new("/tmp/__nope__/__nope__")).is_empty());
    }
}
