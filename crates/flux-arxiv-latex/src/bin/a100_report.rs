//! a100_report — builds the A100 productive-work report as LaTeX (deepseek+qwen
//! authored the content; flux-arxiv-latex compiles it). Dogfoods the crate:
//! reads each titled work folder, emits a sectioned article, compiles to PDF.
use flux_arxiv_latex::doc::{Block, Document};
use std::fs;

/// pdflatex (default fonts) can't render emoji/unicode — keep ASCII, map arrows.
fn ascii(s: &str) -> String {
    s.replace('→', "->").replace('—', "--").replace('·', "-").replace('×', "x")
        .chars().filter(|c| c.is_ascii()).collect()
}

fn main() {
    let root = "/home/orobit/q-narwhalknight/dist-final/a100-work";
    let mut doc = Document::new("article")
        .package("geometry")
        .package("hyperref")
        .preamble(concat!(
            "\\title{A100 Productive Work\\\\\\large Documented by deepseek-r1:70b + qwen2.5:32b, ",
            "built with flux-arxiv-latex}\n",
            "\\author{SIGIL :: The Agent Channel}\n\\date{\\today}\n",
            "\\geometry{margin=1in}"
        ))
        .add(Block::Raw("\\maketitle".into()))
        .add(Block::Section("Abstract".into()))
        .add(Block::Paragraph(
            "This report documents real, productive work performed on a Vast A100 SXM4 \
             (instance 38897456). Two open models cooperated: qwen2.5:32b executed agentic \
             MCP tool-loops against the live Flux/SIGIL codebase, and deepseek-r1:70b reviewed, \
             narrated, and authored code. Each work item is recorded in its own titled folder; \
             this document is compiled from those folders by the flux-arxiv-latex crate."
                .into(),
        ));

    let mut dirs: Vec<_> = fs::read_dir(root)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_else(|_| Vec::new());
    dirs.sort();

    for dir in &dirs {
        let title_md = dir.join("TITLE.md");
        if let Ok(txt) = fs::read_to_string(&title_md) {
            let mut lines = txt.lines();
            let title = lines
                .next()
                .unwrap_or("Work Item")
                .trim_start_matches("# Title:")
                .trim()
                .to_string();
            doc = doc.add(Block::Section(ascii(&title)));
            // body, minus the title line and fenced code, as escaped paragraphs
            let body: String = lines
                .filter(|l| !l.trim_start().starts_with("```"))
                .collect::<Vec<_>>()
                .join("\n");
            // chunk into paragraphs on blank lines so it reads like a paper
            for para in body.split("\n\n") {
                let p = para.trim();
                if !p.is_empty() {
                    doc = doc.add(Block::Paragraph(ascii(&p.chars().take(1200).collect::<String>())));
                }
            }
        }
    }

    doc = doc
        .add(Block::Section("Provenance".into()))
        .add(Block::Paragraph(
            "All artifacts are live at quillon.xyz (show.html, successes.html, tr.html, \
             gallery.html). Models served via ollama OpenAI-compatible endpoint on the A100. \
             Compiled with flux-arxiv-latex (tectonic, pdflatex fallback)."
                .into(),
        ));

    let res = doc.compile_pdf(root, "a100-report");
    println!("LaTeX render: {} chars", doc.render().len());
    println!("compile_pdf success={} pdf={:?}", res.success, res.pdf_path);
    if !res.success {
        eprintln!("--- log tail ---\n{}", &res.log[res.log.len().saturating_sub(800)..]);
    }
}
