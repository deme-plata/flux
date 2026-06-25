//! flux_whitepaper — the Flux whitepaper, built THROUGH flux-arxiv-latex (Document/Block API).
//! Co-authored: DeepSeek-V4-Pro (vision/architecture narrative) + a Claude agent (empirical
//! evaluation + assembly) + Viktor S. Kristensen (direction). pdflatex-compatible (fontenc T1 + lmodern).
use flux_arxiv_latex::doc::{Block, Document};

const TITLE: &str = include_str!("flux_whitepaper_title.txt");
const BODY: &str = include_str!("flux_whitepaper_body.tex");

const PREAMBLE: &str = r#"
\definecolor{ink}{HTML}{0E1116}
\definecolor{accent}{HTML}{14A7C9}
\hypersetup{colorlinks=true,urlcolor=accent,linkcolor=accent,citecolor=accent}
\titleformat{\section}{\large\bfseries\color{ink}}{\thesection}{0.6em}{}
\titleformat{\subsection}{\normalsize\bfseries\color{ink}}{\thesubsection}{0.6em}{}
\titlespacing{\section}{0pt}{12pt}{5pt}
\setlist[itemize]{leftmargin=15pt,itemsep=2pt,topsep=3pt}
\title{\bfseries __TITLE__}
\author{Viktor S. Kristensen \and DeepSeek-V4-Pro \and Claude (agent)}
\date{June 2026 \\ \small github.com/deme-plata/flux}
"#;

fn main() {
    let preamble = PREAMBLE.replace("__TITLE__", TITLE.trim());
    let body = format!("\\maketitle\n\n{}", BODY);

    let doc = Document::new("article")
        .option("11pt")
        .option("a4paper")
        .package_opt("fontenc", &["T1"])
        .package("lmodern")
        .package_opt("geometry", &["a4paper", "top=24mm", "bottom=22mm", "left=24mm", "right=24mm"])
        .package("xcolor")
        .package("titlesec")
        .package("enumitem")
        .package("hyperref")
        .preamble(&preamble)
        .add(Block::Raw(body));

    let out_dir = "/tmp/wp/out";
    std::fs::create_dir_all(out_dir).ok();
    let res = doc.compile_pdf(out_dir, "flux-whitepaper");
    println!("flux-arxiv-latex: success={} pdf={:?}", res.success, res.pdf_path);
    if !res.success {
        eprintln!("COMPILE FAILED");
        std::process::exit(1);
    }
}
