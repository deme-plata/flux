//! lang.rs — **BETA 2: multi-language.** flux-legacy stops being Rust-only.
//!
//! A brownfield repo can be in any language flux's SDK/API target. This module detects the
//! language(s) from marker files + extensions and surveys ANY source tree language-agnostically
//! (files, LOC, god-files) — the foundation for importing the "billion projects" onto Flux. The
//! god-file / LOC / smell metrics don't care about syntax; only the file discovery does.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// A language flux-legacy can survey.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    Rust,
    JavaScript,
    TypeScript,
    Python,
    Go,
    Java,
    Kotlin,
    CSharp,
    Cpp,
    C,
    Ruby,
    Php,
    Swift,
    Scala,
    Solidity,
    Other,
}

impl Language {
    pub fn name(self) -> &'static str {
        use Language::*;
        match self {
            Rust => "Rust", JavaScript => "JavaScript", TypeScript => "TypeScript", Python => "Python",
            Go => "Go", Java => "Java", Kotlin => "Kotlin", CSharp => "C#", Cpp => "C++", C => "C",
            Ruby => "Ruby", Php => "PHP", Swift => "Swift", Scala => "Scala", Solidity => "Solidity",
            Other => "Other",
        }
    }
    /// the source extensions that belong to this language
    pub fn exts(self) -> &'static [&'static str] {
        use Language::*;
        match self {
            Rust => &["rs"],
            JavaScript => &["js", "jsx", "mjs", "cjs"],
            TypeScript => &["ts", "tsx"],
            Python => &["py", "pyi"],
            Go => &["go"],
            Java => &["java"],
            Kotlin => &["kt", "kts"],
            CSharp => &["cs"],
            Cpp => &["cpp", "cc", "cxx", "hpp", "hh"],
            C => &["c", "h"],
            Ruby => &["rb"],
            Php => &["php"],
            Swift => &["swift"],
            Scala => &["scala", "sc"],
            Solidity => &["sol"],
            Other => &[],
        }
    }
    /// classify a file extension into a language
    pub fn from_ext(ext: &str) -> Language {
        use Language::*;
        for l in ALL {
            if l.exts().contains(&ext) {
                return *l;
            }
        }
        Other
    }
}

/// every language we recognize (Other excluded — it's the fallback)
pub const ALL: &[Language] = &[
    Language::Rust, Language::TypeScript, Language::JavaScript, Language::Python, Language::Go,
    Language::Java, Language::Kotlin, Language::CSharp, Language::Cpp, Language::C, Language::Ruby,
    Language::Php, Language::Swift, Language::Scala, Language::Solidity,
];

/// Marker files that pin a project's primary language/build system.
pub fn marker_language(file_name: &str) -> Option<Language> {
    Some(match file_name {
        "Cargo.toml" => Language::Rust,
        "package.json" | "tsconfig.json" => Language::TypeScript, // JS/TS ecosystem; refined by ext counts
        "go.mod" => Language::Go,
        "pom.xml" | "build.gradle" | "build.gradle.kts" => Language::Java,
        "requirements.txt" | "pyproject.toml" | "setup.py" | "Pipfile" => Language::Python,
        "Gemfile" => Language::Ruby,
        "composer.json" => Language::Php,
        "Package.swift" => Language::Swift,
        "build.sbt" => Language::Scala,
        "foundry.toml" | "hardhat.config.js" | "hardhat.config.ts" => Language::Solidity,
        _ => return None,
    })
}

/// Per-language tally in a survey.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LangTally {
    pub language: String,
    pub files: usize,
    pub loc: usize,
}

/// A language-agnostic survey of any source tree.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LangSurvey {
    pub root: String,
    pub primary: String,
    /// marker files found at/near the root
    pub markers: Vec<String>,
    pub tallies: Vec<LangTally>,
    pub total_files: usize,
    pub total_loc: usize,
    /// single source files over the god-file threshold (path, loc), worst-first
    pub god_files: Vec<(String, usize)>,
}

/// Survey a whole repo by language. Walks all source files (skips vendor/build dirs), tallies LOC
/// per language, and finds god-files across every language.
pub fn survey(root: &str) -> LangSurvey {
    use std::collections::BTreeMap;
    let root_path = PathBuf::from(root);
    let mut by_lang: BTreeMap<&'static str, (usize, usize)> = BTreeMap::new();
    let mut god_files = Vec::new();
    let mut total_files = 0;
    let mut total_loc = 0;
    let mut markers = Vec::new();

    for f in walk_source(&root_path) {
        let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if marker_language(name).is_some() {
            if let Ok(rel) = f.strip_prefix(&root_path) {
                let r = rel.to_string_lossy().to_string();
                if r.matches('/').count() <= 2 {
                    markers.push(r);
                }
            }
        }
        let ext = f.extension().and_then(|e| e.to_str()).unwrap_or("");
        let lang = Language::from_ext(ext);
        if lang == Language::Other {
            continue;
        }
        let loc = fs::read_to_string(&f).map(|c| c.lines().count()).unwrap_or(0);
        let e = by_lang.entry(lang.name()).or_insert((0, 0));
        e.0 += 1;
        e.1 += loc;
        total_files += 1;
        total_loc += loc;
        if loc >= crate::GOD_FILE_LOC {
            let rel = f.strip_prefix(&root_path).unwrap_or(&f).to_string_lossy().to_string();
            god_files.push((rel, loc));
        }
    }

    let mut tallies: Vec<LangTally> = by_lang.into_iter()
        .map(|(language, (files, loc))| LangTally { language: language.to_string(), files, loc })
        .collect();
    tallies.sort_by(|a, b| b.loc.cmp(&a.loc));
    god_files.sort_by(|a, b| b.1.cmp(&a.1));
    markers.sort();
    markers.dedup();

    // primary = the marker's language if present, else the heaviest by LOC
    let primary = root_marker_language(&root_path)
        .map(|l| l.name().to_string())
        .or_else(|| tallies.first().map(|t| t.language.clone()))
        .unwrap_or_else(|| "Unknown".to_string());

    LangSurvey { root: root.to_string(), primary, markers, tallies, total_files, total_loc, god_files }
}

/// Detect the primary language from a marker file at the repo root.
pub fn root_marker_language(root: &Path) -> Option<Language> {
    let entries = fs::read_dir(root).ok()?;
    let mut langs = Vec::new();
    for e in entries.flatten() {
        if let Some(name) = e.file_name().to_str() {
            if let Some(l) = marker_language(name) {
                langs.push(l);
            }
        }
    }
    // Cargo.toml wins if present (most specific), else first marker
    langs.iter().find(|l| **l == Language::Rust).copied().or_else(|| langs.first().copied())
}

/// Render the survey.
pub fn render_survey(s: &LangSurvey) -> String {
    let mut out = format!(
        "🌐 LANGUAGE SURVEY — {}\n   primary: {} · {} source files · {} LOC · markers: {}\n\n",
        s.root, s.primary, s.total_files, s.total_loc,
        if s.markers.is_empty() { "(none)".into() } else { s.markers.join(", ") },
    );
    out.push_str("  language        files     LOC\n");
    for t in &s.tallies {
        out.push_str(&format!("  {:<14} {:>5}  {:>7}\n", t.language, t.files, t.loc));
    }
    if !s.god_files.is_empty() {
        out.push_str(&format!("\n  god-files (>{} LOC):\n", crate::GOD_FILE_LOC));
        for (p, loc) in s.god_files.iter().take(10) {
            out.push_str(&format!("    {:>7}  {}\n", loc, p));
        }
    }
    out
}

/// Walk a tree for source files, skipping vendor/build/VCS dirs.
fn walk_source(root: &Path) -> Vec<PathBuf> {
    const SKIP: &[&str] = &[
        ".git", "target", "node_modules", "vendor", "dist", "build", ".venv", "venv",
        "__pycache__", ".gradle", "bin", "obj", ".next", "out", ".cargo",
    ];
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(entries) = fs::read_dir(&d) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !SKIP.contains(&name) {
                        stack.push(p);
                    }
                } else {
                    out.push(p);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_extensions() {
        assert_eq!(Language::from_ext("rs"), Language::Rust);
        assert_eq!(Language::from_ext("tsx"), Language::TypeScript);
        assert_eq!(Language::from_ext("py"), Language::Python);
        assert_eq!(Language::from_ext("go"), Language::Go);
        assert_eq!(Language::from_ext("sol"), Language::Solidity);
        assert_eq!(Language::from_ext("xyz"), Language::Other);
    }

    #[test]
    fn markers_pin_the_language() {
        assert_eq!(marker_language("Cargo.toml"), Some(Language::Rust));
        assert_eq!(marker_language("go.mod"), Some(Language::Go));
        assert_eq!(marker_language("pyproject.toml"), Some(Language::Python));
        assert_eq!(marker_language("README.md"), None);
    }

    #[test]
    fn surveys_a_polyglot_tree_and_finds_god_files() {
        let tmp = std::env::temp_dir().join(format!("flux-lang-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("src")).unwrap();
        fs::write(tmp.join("go.mod"), "module x\n").unwrap();
        fs::write(tmp.join("src/main.go"), "package main\n".repeat(50)).unwrap();
        fs::write(tmp.join("src/app.py"), "print(1)\n".repeat(900)).unwrap(); // god-file
        fs::write(tmp.join("src/util.ts"), "export const a = 1;\n".repeat(20)).unwrap();
        // vendored dir must be skipped
        fs::create_dir_all(tmp.join("node_modules/pkg")).unwrap();
        fs::write(tmp.join("node_modules/pkg/huge.js"), "x\n".repeat(5000)).unwrap();

        let s = survey(tmp.to_str().unwrap());
        assert_eq!(s.primary, "Go", "go.mod pins primary");
        let langs: Vec<&str> = s.tallies.iter().map(|t| t.language.as_str()).collect();
        assert!(langs.contains(&"Go") && langs.contains(&"Python") && langs.contains(&"TypeScript"));
        assert!(!langs.contains(&"JavaScript"), "node_modules vendored js skipped");
        // the 900-line python file is the god-file
        assert!(s.god_files.iter().any(|(p, _)| p.ends_with("app.py")));
        let _ = fs::remove_dir_all(&tmp);
    }
}
