//! A minimal, dependency-free LaTeX **document builder** for Flux.
//!
//! `flux-arxiv-latex` already turns arXiv results into BibTeX + a Related-Work
//! section; this module is the other half the crate's doc-comment promised —
//! assembling a whole `.tex` (the flux-foundation / SIGIL whitepapers, the
//! fluxc cheatsheet, the ShadowChain book) from a small AST instead of ad-hoc
//! `String` concatenation, then compiling it (tectonic → pdflatex fallback).
//!
//! ```
//! use flux_arxiv_latex::{Document, Block};
//! let tex = Document::new("article")
//!     .option("11pt")
//!     .package_opt("inputenc", &["utf8"])
//!     .package("amsmath")
//!     .preamble("\\title{Demo}\\author{Flux}")
//!     .add(Block::Raw("\\maketitle".into()))
//!     .add(Block::Section("Intro".into()))
//!     .add(Block::Paragraph("Hello.".into()))
//!     .render();
//! assert!(tex.contains("\\documentclass[11pt]{article}"));
//! assert!(tex.contains("\\usepackage[utf8]{inputenc}"));
//! assert!(tex.contains("\\begin{document}"));
//! assert!(tex.contains("\\section{Intro}"));
//! ```

use std::process::Command;

/// A body element. The book port leans on `Raw` (chapter `.tex` content is
/// emitted verbatim); the typed variants exist for programmatic documents.
#[derive(Debug, Clone)]
pub enum Block {
    /// Verbatim LaTeX, emitted as-is.
    Raw(String),
    /// `\section{..}`
    Section(String),
    /// `\subsection{..}`
    Subsection(String),
    /// A paragraph of body text (escaped via [`crate::latex_escape`]).
    Paragraph(String),
}

/// Outcome of a [`Document::compile_pdf`] run.
#[derive(Debug, Clone)]
pub struct PdfResult {
    pub success: bool,
    /// Path to the produced PDF, when compilation succeeded.
    pub pdf_path: Option<String>,
    /// Combined engine stdout+stderr (or a diagnostic when no engine exists).
    pub log: String,
}

/// A LaTeX document assembled from a class, packages, a preamble and a body.
#[derive(Debug, Clone, Default)]
pub struct Document {
    class: String,
    options: Vec<String>,
    /// (package name, options)
    packages: Vec<(String, Vec<String>)>,
    preamble: String,
    body: Vec<Block>,
}

impl Document {
    /// Start a document of the given class (`book`, `article`, …).
    pub fn new(class: &str) -> Self {
        Document { class: class.to_string(), ..Default::default() }
    }

    /// Add a `\documentclass[..]` option (e.g. `11pt`, `a4paper`).
    pub fn option(mut self, opt: &str) -> Self {
        self.options.push(opt.to_string());
        self
    }

    /// `\usepackage{name}`.
    pub fn package(mut self, name: &str) -> Self {
        self.packages.push((name.to_string(), Vec::new()));
        self
    }

    /// `\usepackage[opts]{name}`.
    pub fn package_opt(mut self, name: &str, opts: &[&str]) -> Self {
        self.packages
            .push((name.to_string(), opts.iter().map(|s| s.to_string()).collect()));
        self
    }

    /// Verbatim preamble text inserted after the packages, before
    /// `\begin{document}` (definecolor, hypersetup, `\title`, …).
    pub fn preamble(mut self, p: &str) -> Self {
        self.preamble = p.to_string();
        self
    }

    /// Append a body block.
    pub fn add(mut self, b: Block) -> Self {
        self.body.push(b);
        self
    }

    /// Render the full `.tex` source.
    pub fn render(&self) -> String {
        let mut s = String::new();
        if self.options.is_empty() {
            s.push_str(&format!("\\documentclass{{{}}}\n", self.class));
        } else {
            s.push_str(&format!(
                "\\documentclass[{}]{{{}}}\n",
                self.options.join(","),
                self.class
            ));
        }
        for (name, opts) in &self.packages {
            if opts.is_empty() {
                s.push_str(&format!("\\usepackage{{{}}}\n", name));
            } else {
                s.push_str(&format!("\\usepackage[{}]{{{}}}\n", opts.join(","), name));
            }
        }
        if !self.preamble.is_empty() {
            s.push_str(&self.preamble);
            if !self.preamble.ends_with('\n') {
                s.push('\n');
            }
        }
        s.push_str("\n\\begin{document}\n");
        for b in &self.body {
            s.push_str(&render_block(b));
            s.push('\n');
        }
        s.push_str("\\end{document}\n");
        s
    }

    /// Write `<out_dir>/<job>.tex` and compile it to PDF. Tries `tectonic`
    /// first (self-contained, no TeX install), then `pdflatex` run twice so
    /// the table of contents resolves. Never panics; failures land in the log.
    pub fn compile_pdf(&self, out_dir: &str, job: &str) -> PdfResult {
        if let Err(e) = std::fs::create_dir_all(out_dir) {
            return PdfResult { success: false, pdf_path: None, log: format!("mkdir {out_dir}: {e}") };
        }
        let tex_path = format!("{out_dir}/{job}.tex");
        if let Err(e) = std::fs::write(&tex_path, self.render()) {
            return PdfResult { success: false, pdf_path: None, log: format!("write {tex_path}: {e}") };
        }
        let pdf_path = format!("{out_dir}/{job}.pdf");

        // 1) tectonic
        if let Ok(out) = Command::new("tectonic")
            .args(["-o", out_dir, &tex_path])
            .output()
        {
            let log = String::from_utf8_lossy(&out.stderr).into_owned();
            if out.status.success() && std::path::Path::new(&pdf_path).exists() {
                return PdfResult { success: true, pdf_path: Some(pdf_path), log };
            }
        }

        // 2) pdflatex twice (resolve \tableofcontents / refs)
        let mut last_log = String::new();
        let mut ran_pdflatex = false;
        for _ in 0..2 {
            match Command::new("pdflatex")
                .args(["-interaction=nonstopmode", "-halt-on-error", "-output-directory", out_dir, &tex_path])
                .output()
            {
                Ok(out) => {
                    ran_pdflatex = true;
                    last_log = String::from_utf8_lossy(&out.stdout).into_owned();
                }
                Err(e) => {
                    return PdfResult {
                        success: false,
                        pdf_path: None,
                        log: if ran_pdflatex {
                            last_log
                        } else {
                            format!("no LaTeX engine found (tried tectonic, pdflatex): {e}")
                        },
                    };
                }
            }
        }
        let success = std::path::Path::new(&pdf_path).exists();
        PdfResult {
            success,
            pdf_path: success.then(|| pdf_path.clone()),
            log: last_log,
        }
    }
}

fn render_block(b: &Block) -> String {
    match b {
        Block::Raw(s) => s.clone(),
        Block::Section(t) => format!("\\section{{{}}}", t),
        Block::Subsection(t) => format!("\\subsection{{{}}}", t),
        Block::Paragraph(p) => crate::latex_escape(p),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_class_packages_and_body() {
        let tex = Document::new("book")
            .option("11pt")
            .option("a4paper")
            .package_opt("inputenc", &["utf8"])
            .package("amsmath")
            .preamble("\\title{T}")
            .add(Block::Raw("\\maketitle".into()))
            .add(Block::Section("Intro".into()))
            .render();
        assert!(tex.contains("\\documentclass[11pt,a4paper]{book}"));
        assert!(tex.contains("\\usepackage[utf8]{inputenc}"));
        assert!(tex.contains("\\usepackage{amsmath}"));
        assert!(tex.contains("\\title{T}"));
        assert!(tex.contains("\\begin{document}"));
        assert!(tex.contains("\\maketitle"));
        assert!(tex.contains("\\section{Intro}"));
        assert!(tex.trim_end().ends_with("\\end{document}"));
    }

    #[test]
    fn no_options_renders_bare_class() {
        let tex = Document::new("article").render();
        assert!(tex.contains("\\documentclass{article}"));
    }

    #[test]
    fn paragraph_block_is_escaped() {
        let tex = Document::new("article")
            .add(Block::Paragraph("a & b_c".into()))
            .render();
        assert!(tex.contains(r"a \& b\_c"));
    }
}
