//! sigil_project_report — the SIGIL Graph project report, built THROUGH
//! flux-arxiv-latex, modeled on flux_project_report.rs's Copenhagen Business
//! School HA(it.) project-management toolbox: Gantt (pgfgantt), milestone
//! plan, risk matrix, stakeholder grid, SWOT, execution analytics — mined
//! from SIGIL's own git history and its live sigil-api node.
//! Co-authored: Rocky (analysis + assembly) + Viktor S. Kristensen (direction).
use flux_arxiv_latex::doc::{Block, Document};

const BODY: &str = include_str!("sigil_project_report_body.tex");

const PREAMBLE: &str = r#"
\usetikzlibrary{calc}
\definecolor{ink}{HTML}{0E1116}
\definecolor{accent}{HTML}{14A7C9}
\definecolor{gold}{HTML}{C9A227}
\definecolor{risk}{HTML}{C0392B}
\definecolor{okgreen}{HTML}{27AE60}
\hypersetup{colorlinks=true,urlcolor=accent,linkcolor=accent,citecolor=accent}
\titleformat{\section}{\large\bfseries\color{ink}}{\thesection}{0.6em}{}
\titleformat{\subsection}{\normalsize\bfseries\color{ink}}{\thesubsection}{0.6em}{}
\titlespacing{\section}{0pt}{12pt}{5pt}
\setlist[itemize]{leftmargin=15pt,itemsep=2pt,topsep=3pt}
\pgfplotsset{compat=1.17}
\title{\bfseries SIGIL Graph: A DagKnight Sister Chain \\ \large A Project Report on a Second Blockchain, Built by an Agent Swarm \\ \normalsize analysed with the HA(it.) project-management toolbox}
\author{Viktor S. Kristensen \and Rocky (agent) \and the SIGIL swarm}
\date{August 2026 \\ \small sigilgraph.org \,\textperiodcentered\, data: git history, live sigil-api node}
"#;

fn main() {
    let body = format!("\\maketitle\n\n{}", BODY);

    let doc = Document::new("article")
        .option("11pt")
        .option("a4paper")
        .package_opt("fontenc", &["T1"])
        .package("lmodern")
        .package_opt("geometry", &["a4paper", "top=24mm", "bottom=22mm", "left=22mm", "right=22mm"])
        .package("xcolor")
        .package("titlesec")
        .package("enumitem")
        .package("booktabs")
        .package("tikz")
        .package("pgfplots")
        .package("pgfgantt")
        .package("hyperref")
        .preamble(PREAMBLE)
        .add(Block::Raw(body));

    let out_dir = "/home/storage/claude-code/scratch-sigil-shots/sigil-report/out";
    std::fs::create_dir_all(out_dir).ok();
    let res = doc.compile_pdf(out_dir, "sigil-project-report");
    println!("flux-arxiv-latex: success={} pdf={:?}", res.success, res.pdf_path);
    if !res.success {
        eprintln!("COMPILE FAILED (the emitted .tex is still in {out_dir})");
        std::process::exit(1);
    }
}
