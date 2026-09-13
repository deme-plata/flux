// wire_safety.rs — serde attributes a non-self-describing codec cannot decode.
//
// bincode and postcard answer no `deserialize_any`: they are not self-describing.
// serde implements internally-tagged enums, untagged enums and `#[serde(flatten)]`
// by buffering through `deserialize_any` — so such a type ENCODES fine and can
// never be DECODED. `skip_serializing_if` writes zero bytes for a skipped field
// while the decoder still reads one, desynchronising every value after it.
//
// Five SIGIL incidents, each hidden until a value actually took the path:
//   2026-08-15  sigil-header    skip_serializing_if, twice in one day
//   2026-09-11  sigil-tx        skip_serializing_if on Shield::note_ciphertext
//   2026-09-11  sigil-state     skip_serializing_if on two StateMutation variants
//   2026-09-11  block backfill  `#[serde(tag = "kind")]` on SigilEvent — a follower sat
//                               6 M blocks behind on a payload that arrived COMPLETE
//
// The audit is a source scan, not a type check: it flags the attribute wherever a
// codec is IN SCOPE for the crate (a direct dependency, or a dependent crate that
// could put the type on that wire). A line carrying `flux-wire: allow` — on the
// attribute or the line above it — is counted, not reported: that is how a type
// that provably travels only on MessagePack says so, next to the attribute.

use crate::{CrateInfo, WorkspaceGraph};
use std::path::{Path, PathBuf};

/// Serde codecs that cannot answer `deserialize_any`.
pub const NON_SELF_DESCRIBING: &[&str] = &["bincode", "postcard"];

/// Marker that silences a finding. Put it on the attribute line or the line above.
pub const ALLOW_MARKER: &str = "flux-wire: allow";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireRule {
    /// `#[serde(tag = "…")]` without `content = "…"` — decoded via `deserialize_any`.
    InternallyTagged,
    /// `#[serde(untagged)]` — tries each variant via buffered `deserialize_any`.
    Untagged,
    /// `#[serde(flatten)]` — collects the map via `deserialize_any`.
    Flatten,
    /// `skip_serializing_if` — zero bytes on skip, but the decoder still reads the field.
    SkipSerializingIf,
}

impl WireRule {
    pub fn code(&self) -> &'static str {
        match self {
            WireRule::InternallyTagged => "W1",
            WireRule::Untagged => "W2",
            WireRule::Flatten => "W3",
            WireRule::SkipSerializingIf => "W4",
        }
    }

    pub fn why(&self) -> &'static str {
        match self {
            WireRule::InternallyTagged =>
                "internally-tagged enum: serde buffers through deserialize_any, which bincode/postcard cannot answer — encodes, never decodes",
            WireRule::Untagged =>
                "untagged enum: each variant is tried via deserialize_any — encodes, never decodes",
            WireRule::Flatten =>
                "flatten: the field map is collected via deserialize_any — encodes, never decodes",
            WireRule::SkipSerializingIf =>
                "skip_serializing_if: a skipped field writes zero bytes but the decoder still reads one — every value after it desynchronises",
        }
    }

    pub fn fix(&self) -> &'static str {
        match self {
            WireRule::InternallyTagged | WireRule::Untagged =>
                "use the default externally-tagged form, or adjacent tagging (tag + content), or a self-describing wire (MessagePack/JSON) with a magic prefix",
            WireRule::Flatten => "name the nested struct as a field instead of flattening it",
            WireRule::SkipSerializingIf =>
                "keep the field on the struct; strip it inside encode()/hashing explicitly (the sigil-header way)",
        }
    }
}

#[derive(Debug, Clone)]
pub struct WireFinding {
    pub crate_name: String,
    /// Path relative to the workspace root.
    pub file: String,
    /// 1-based line of the attribute's first line.
    pub line: usize,
    pub rule: WireRule,
    /// The attribute text, whitespace-collapsed.
    pub attribute: String,
    /// Which codec(s) put this crate in scope, and through which crate ("bincode via sigil-net").
    pub codecs: Vec<String>,
    /// `true` when the crate itself depends on the codec — it can decode with it, so this is a
    /// FINDING. `false` when only a dependent crate carries the codec: the type *could* be put on
    /// that wire, so it is listed for REVIEW and does not fail the gate unless `--strict`.
    pub direct: bool,
}

#[derive(Debug, Clone, Default)]
pub struct WireAudit {
    pub findings: Vec<WireFinding>,
    pub crates_scanned: usize,
    /// Crates with at least one non-self-describing codec in scope.
    pub crates_in_scope: usize,
    pub files_scanned: usize,
    /// Attributes that matched a rule but carried the allow marker.
    pub allowed: usize,
}

impl WireAudit {
    /// No direct findings. Reachable-only findings are review items, not failures.
    pub fn is_clean(&self) -> bool { self.direct_findings() == 0 }
    pub fn is_clean_strict(&self) -> bool { self.findings.is_empty() }
    pub fn direct_findings(&self) -> usize { self.findings.iter().filter(|f| f.direct).count() }
    pub fn review_findings(&self) -> usize { self.findings.iter().filter(|f| !f.direct).count() }
}

/// Direct dependencies of `ci` that are non-self-describing codecs.
pub fn direct_codecs(ci: &CrateInfo) -> Vec<String> {
    let mut out: Vec<String> = ci.dependencies.iter()
        .filter(|d| NON_SELF_DESCRIBING.contains(&d.name.as_str()))
        .map(|d| d.name.clone())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Codecs in scope for crate `idx`: its own, plus those of every crate that
/// (transitively) depends on it — a dependent is exactly who can put this
/// crate's types on a bincode wire. Entries read "bincode" or "bincode via <crate>".
pub fn codecs_in_scope(ws: &WorkspaceGraph, idx: usize) -> Vec<String> {
    let mut out = direct_codecs(&ws.crates[idx]);
    for dep_idx in crate::agility::transitive_dependents(ws, &ws.crates[idx].name) {
        for c in direct_codecs(&ws.crates[dep_idx]) {
            let via = format!("{} via {}", c, ws.crates[dep_idx].name);
            if !out.iter().any(|o| o == &c || o == &via) {
                out.push(via);
            }
        }
    }
    out
}

fn classify(attr: &str) -> Option<WireRule> {
    // Only serde attributes count; `#[cfg_attr(feature = "x", serde(...))]` included.
    if !attr.contains("serde(") { return None; }
    let body = &attr[attr.find("serde(").unwrap() + "serde(".len()..];
    let compact: String = body.split_whitespace().collect();
    if compact.contains("skip_serializing_if") {
        return Some(WireRule::SkipSerializingIf);
    }
    if compact.contains("untagged") {
        return Some(WireRule::Untagged);
    }
    if compact.contains("flatten") {
        return Some(WireRule::Flatten);
    }
    if compact.contains("tag=") && !compact.contains("content=") {
        return Some(WireRule::InternallyTagged);
    }
    None
}

/// Scan one source file. Returns findings plus the number of allowed matches.
/// Only attribute lines (`#[…`) are inspected — a whole-file substring search
/// would match doc comments and the guard test's own error message.
pub fn scan_source(crate_name: &str, rel_path: &str, content: &str, codecs: &[String], direct: bool) -> (Vec<WireFinding>, usize) {
    let lines: Vec<&str> = content.lines().collect();
    let mut findings = Vec::new();
    let mut allowed = 0usize;
    let mut i = 0usize;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        if !trimmed.starts_with("#[") { i += 1; continue; }
        // Accumulate a multi-line attribute until its brackets balance.
        let start = i;
        let mut attr = String::new();
        let mut depth: i32 = 0;
        loop {
            let l = lines[i];
            attr.push_str(l.trim());
            attr.push(' ');
            depth += l.matches('[').count() as i32 - l.matches(']').count() as i32;
            i += 1;
            if depth <= 0 || i >= lines.len() { break; }
        }
        // A line INSIDE a string literal can start with `#[` too (a test fixture, a doc
        // example). Real attribute text never contains an escaped newline or quote.
        if attr.contains("\\n") || attr.contains("\\\"") { continue; }
        let Some(rule) = classify(&attr) else { continue; };
        let prev = if start > 0 { lines[start - 1] } else { "" };
        let marked = attr.contains(ALLOW_MARKER) || prev.contains(ALLOW_MARKER)
            || lines[start..i].iter().any(|l| l.contains(ALLOW_MARKER));
        if marked { allowed += 1; continue; }
        findings.push(WireFinding {
            crate_name: crate_name.to_string(),
            file: rel_path.to_string(),
            line: start + 1,
            rule,
            attribute: attr.split_whitespace().collect::<Vec<_>>().join(" "),
            codecs: codecs.to_vec(),
            direct,
        });
    }
    (findings, allowed)
}

fn walk_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return; };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        if p.is_dir() { walk_rs(&p, out); }
        else if p.extension().map_or(false, |e| e == "rs") { out.push(p); }
    }
}

/// Audit every workspace crate that has a non-self-describing codec in scope.
pub fn audit_wire_safety(ws: &WorkspaceGraph) -> WireAudit {
    let mut audit = WireAudit { crates_scanned: ws.crates.len(), ..Default::default() };
    for (idx, ci) in ws.crates.iter().enumerate() {
        let codecs = codecs_in_scope(ws, idx);
        if codecs.is_empty() { continue; }
        let direct = !direct_codecs(ci).is_empty();
        audit.crates_in_scope += 1;
        let mut files = Vec::new();
        walk_rs(&ci.path.join("src"), &mut files);
        for path in files {
            let Ok(content) = std::fs::read_to_string(&path) else { continue; };
            audit.files_scanned += 1;
            let rel = path.strip_prefix(&ws.root).unwrap_or(&path).to_string_lossy().to_string();
            let (f, a) = scan_source(&ci.name, &rel, &content, &codecs, direct);
            audit.findings.extend(f);
            audit.allowed += a;
        }
    }
    audit
}

/// Human-readable report. Direct findings first (the crate decodes with the codec
/// itself), then reachable ones for review, then the legend for the rules that fired —
/// so the reader gets the why and the fix without a lookup.
pub fn render_text(audit: &WireAudit) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "⚡ Flux wire-safety audit — {} crate(s), {} with bincode/postcard in scope, {} file(s)\n",
        audit.crates_scanned, audit.crates_in_scope, audit.files_scanned
    ));
    let (direct, review) = (audit.direct_findings(), audit.review_findings());
    if audit.findings.is_empty() {
        s.push_str(&format!("  ✓ clean — 0 findings, {} allowed by marker\n", audit.allowed));
        return s;
    }
    if direct == 0 {
        s.push_str(&format!("  ✓ 0 direct findings ({} to review, {} allowed by marker)\n", review, audit.allowed));
    } else {
        s.push_str(&format!("  ✗ {} direct finding(s), {} to review, {} allowed by marker\n", direct, review, audit.allowed));
    }
    let line = |f: &WireFinding| format!("  {} {}:{}  [{}]  {}\n      codec: {}\n",
        f.rule.code(), f.file, f.line, f.crate_name, f.attribute, f.codecs.join(", "));
    if direct > 0 {
        s.push_str("\n  FINDINGS — the crate depends on the codec itself:\n");
        for f in audit.findings.iter().filter(|f| f.direct) { s.push_str(&line(f)); }
    }
    if review > 0 {
        s.push_str("\n  REVIEW — a dependent crate carries the codec, so the type could travel on it:\n");
        for f in audit.findings.iter().filter(|f| !f.direct) { s.push_str(&line(f)); }
    }
    let mut seen: Vec<&WireRule> = Vec::new();
    for f in &audit.findings {
        if !seen.contains(&&f.rule) { seen.push(&f.rule); }
    }
    s.push('\n');
    for r in seen {
        s.push_str(&format!("  {}  {}\n      fix: {}\n", r.code(), r.why(), r.fix()));
    }
    s.push_str(&format!("\n  silence a deliberate one with `// {}` on (or above) the attribute\n", ALLOW_MARKER));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CrateType, DepKind, Dependency};

    fn codecs() -> Vec<String> { vec!["bincode".to_string()] }

    #[test]
    fn internally_tagged_is_w1_but_adjacent_is_fine() {
        let src = "#[derive(Serialize, Deserialize)]\n#[serde(tag = \"kind\")]\npub enum E { A }\n\
                   \x23[serde(tag = \"t\", content = \"c\")]\npub enum F { B }\n";
        let (f, allowed) = scan_source("c", "src/lib.rs", src, &codecs(), true);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule, WireRule::InternallyTagged);
        assert_eq!(f[0].line, 2);
        assert_eq!(allowed, 0);
    }

    #[test]
    fn skip_serializing_if_untagged_flatten() {
        let src = "struct S {\n    #[serde(default, skip_serializing_if = \"Option::is_none\")]\n    ct: Option<u8>,\n}\n\
                   \x23[serde(untagged)]\nenum U { A }\n\
                   struct T {\n    #[serde(flatten)]\n    inner: S,\n}\n";
        let (f, _) = scan_source("c", "src/x.rs", src, &codecs(), true);
        let rules: Vec<_> = f.iter().map(|x| x.rule.clone()).collect();
        assert_eq!(rules, vec![WireRule::SkipSerializingIf, WireRule::Untagged, WireRule::Flatten]);
    }

    #[test]
    fn multiline_attribute_and_allow_marker() {
        let src = "// flux-wire: allow — travels only on MessagePack (SIGILM1), pinned by test\n\
                   \x23[serde(\n    tag = \"kind\"\n)]\npub enum Ev { A }\n\
                   \x23[serde(\n    tag = \"kind\"\n)]\npub enum Ev2 { A }\n";
        let (f, allowed) = scan_source("c", "src/lib.rs", src, &codecs(), false);
        assert_eq!(allowed, 1);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].line, 6);
        assert_eq!(f[0].attribute, "#[serde( tag = \"kind\" )]");
        assert!(!f[0].direct, "codec came via a dependent → review, not a finding");
    }

    #[test]
    fn comments_and_strings_are_not_attributes() {
        // The sigil-tx guard test learned this: a whole-file search matches its own message.
        // `h` stands in for `#` so THIS file's lines never start with an attribute-in-a-string.
        let h = "#";
        let src = format!(
            "// {h}[serde(tag = \"kind\")] is forbidden here\n\
             const MSG: &str = \"{h}[serde(skip_serializing_if)] returned\";\n\
             {h}[serde(rename_all = \"snake_case\")]\nstruct Ok1;\n{h}[derive(Serialize)]\nstruct Ok2;\n\
             let fixture = \"line one\\\n\
             {h}[serde(untagged)]\\nenum InString {{}}\";\n"
        );
        // Sanity: the fixture really has a line that starts with `#[` inside a string literal.
        assert!(src.lines().any(|l| l.starts_with("#[serde(untagged)]\\n")), "{src}");
        let (f, allowed) = scan_source("c", "src/lib.rs", &src, &codecs(), true);
        assert!(f.is_empty(), "{:?}", f);
        assert_eq!(allowed, 0);
    }

    #[test]
    fn cfg_attr_serde_counts() {
        let src = "#[cfg_attr(feature = \"serde\", serde(untagged))]\nenum U { A }\n";
        let (f, _) = scan_source("c", "src/lib.rs", src, &codecs(), true);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule, WireRule::Untagged);
    }

    fn ci(name: &str, deps: &[&str]) -> CrateInfo {
        CrateInfo {
            name: name.to_string(),
            path: PathBuf::from(format!("/nonexistent/{name}")),
            dependencies: deps.iter().map(|d| Dependency {
                name: d.to_string(),
                path: if d.starts_with("wire") || d.starts_with("types") { Some(PathBuf::from(format!("/nonexistent/{d}"))) } else { None },
                kind: if d.starts_with("wire") || d.starts_with("types") { DepKind::Path } else { DepKind::CratesIo },
                optional: false,
                dev: false,
            }).collect(),
            edition: "2021".into(),
            crate_type: CrateType::Lib,
            features: vec![],
        }
    }

    #[test]
    fn codec_scope_flows_from_dependents() {
        // types has no codec of its own; wire depends on types AND bincode → types is in scope.
        let crates = vec![ci("types", &["serde"]), ci("wire", &["types", "bincode"]), ci("cli", &["wire"])];
        let dag = crate::graph::build_dag(&crates).unwrap();
        let batches = crate::graph::topological_batches(&dag);
        let ws = WorkspaceGraph { root: PathBuf::from("/nonexistent"), crates, batches };
        assert_eq!(codecs_in_scope(&ws, 0), vec!["bincode via wire".to_string()]);
        assert_eq!(codecs_in_scope(&ws, 1), vec!["bincode".to_string()]);
        assert!(codecs_in_scope(&ws, 2).is_empty(), "cli depends on wire, nothing depends on cli");
    }

    #[test]
    fn render_lists_rule_legend_once_per_rule() {
        let (mut findings, _) = scan_source("c", "src/lib.rs",
            "#[serde(untagged)]\nenum A {}\n#[serde(untagged)]\nenum B {}\n", &codecs(), true);
        findings[1].direct = false;
        let audit = WireAudit { findings, crates_scanned: 1, crates_in_scope: 1, files_scanned: 1, allowed: 0 };
        assert!(!audit.is_clean() && !audit.is_clean_strict());
        let text = render_text(&audit);
        assert_eq!(text.matches("W2 ").count(), 3, "two findings + one legend line:\n{text}");
        assert!(text.contains("FINDINGS") && text.contains("REVIEW"), "{text}");
        assert!(text.contains(ALLOW_MARKER));
        assert!(render_text(&WireAudit::default()).contains("✓ clean"));
        // Review-only is clean for the gate, not for --strict.
        let mut review_only = audit.clone();
        review_only.findings.retain(|f| !f.direct);
        assert!(review_only.is_clean() && !review_only.is_clean_strict());
    }
}
