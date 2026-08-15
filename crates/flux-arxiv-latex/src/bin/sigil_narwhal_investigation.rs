//! sigil_narwhal_investigation — a Flux Science research note investigating what,
//! if anything, is actually new in the SIGIL Narwhal-style DAG mempool design
//! (see `sigil/SIGIL_NARWHAL_MEMPOOL_v0.md`) against the real published literature.
//!
//! This is deliberately NOT a numbers-and-extrapolation paper like
//! `idle_machine`/`legibility_dividend` — there is no measured kernel to report.
//! It is a literature-grounded correction: the design doc's first draft claimed an
//! "invented upgrade" (erasure-coded Narwhal-style batch dissemination) that turns
//! out to already be published (Imitater, arXiv:2409.19286, Sep 2024). This paper
//! is the honest writeup of that finding — what's prior art, what's a real
//! counter-argument from a production team (Aptos) that was checked and engaged
//! with rather than routed around, and what (if anything) is left once the
//! literature is accounted for.
//!
//! Usage: sigil_narwhal_investigation [arxiv.json] [out_dir]

use flux_arxiv_latex::doc::{Block, Document};
use flux_arxiv_latex::{bibliography, latex_escape, parse_arxiv_json, ArxivPaper};

fn raw(s: &str) -> Block {
    Block::Raw(format!("{s}\n\n"))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let json_path = args
        .get(1)
        .map(String::as_str)
        .unwrap_or("crates/flux-arxiv-latex/sigil_narwhal_investigation.arxiv.json");
    let out_dir = args.get(2).map(String::as_str).unwrap_or("/tmp/sigil-narwhal-investigation");

    let papers: Vec<ArxivPaper> = std::fs::read_to_string(json_path)
        .ok()
        .and_then(|j| parse_arxiv_json(&j).ok())
        .unwrap_or_default();

    let mut doc = Document::new("article")
        .option("11pt")
        .option("a4paper")
        .package_opt("inputenc", &["utf8"])
        .package_opt("geometry", &["margin=1.1in"])
        .package("amsmath")
        .package("amssymb")
        .package("booktabs")
        .package("tabularx")
        .package("ragged2e")
        .package("colortbl")
        .package("tcolorbox")
        .package_opt("enumitem", &[])
        .package_opt("hyperref", &["hidelinks"])
        .preamble(concat!(
            "\\providecommand{\\textcite}[1]{\\cite{#1}}\n",
            "\\definecolor{sigilcyan}{HTML}{0E7C86}\n",
            "\\definecolor{sigilviolet}{HTML}{5B4B8A}\n",
            "\\definecolor{sigilamber}{HTML}{B26B00}\n",
            "\\definecolor{sigilred}{HTML}{A32020}\n",
            "\\definecolor{sigilgreen}{HTML}{1E6F3C}\n",
            "\\definecolor{slate}{HTML}{6B7280}\n",
            "\\definecolor{panelbg}{HTML}{F6F7F9}\n",
            "\\newcommand{\\prior}[1]{\\textcolor{sigilred}{\\textbf{#1}}}\n",
            "\\newcommand{\\newv}[1]{\\textcolor{sigilgreen}{\\textbf{#1}}}\n",
            "\\newcommand{\\stP}{\\textcolor{sigilred}{$\\blacksquare$}~{\\scriptsize\\textsc{prior art}}}\n",
            "\\newcommand{\\stN}{\\textcolor{sigilgreen}{$\\square$}~{\\scriptsize\\textsc{new for sigil}}}\n",
            "\\newcommand{\\stC}{\\textcolor{sigilamber}{$\\triangle$}~{\\scriptsize\\textsc{open counter-argument}}}\n",
            "\\newcommand{\\brief}[3]{\\begin{tcolorbox}[colback=panelbg,colframe=#1,boxrule=0.8pt,arc=2pt,",
            "title=\\textbf{#2},coltitle=white,colbacktitle=#1]#3\\end{tcolorbox}}\n",
            "\\hypersetup{pdftitle={What Is Actually New Here?},pdfsubject={A literature check of a proposed ",
            "SIGIL DAG mempool design against Narwhal, Bullshark, and erasure-coded mempool prior art},",
            "pdfkeywords={DAG mempool, Narwhal, Bullshark, erasure coding, Byzantine fault tolerance, ",
            "blockchain, research integrity}}\n",
            "\\title{\\textbf{What Is Actually New Here?}\\\\[6pt]\\large A Literature Check of a Proposed SIGIL ",
            "DAG Mempool Against Narwhal, Bullshark, and Erasure-Coded Mempool Prior Art\\\\[4pt]\\normalsize",
            "\\itshape Correcting a false novelty claim, in public, the same day it was made}\n",
            "\\author{Flux Science\\\\\\small research note by Grogu (Claude Opus 5), requested by Viktor,",
            "\\\\\\small related work drawn from a live arXiv sweep}\n",
            "\\date{\\today}"
        ))
        .add(Block::Raw("\\maketitle".into()));

    // ── abstract
    doc = doc.add(Block::Raw(String::from(
        "\\begin{abstract}\\noindent\n\
         A same-session design document (\\texttt{SIGIL\\_NARWHAL\\_MEMPOOL\\_v0.md}) proposed a \
         Narwhal-style DAG mempool for the SIGIL blockchain and, in one section, described \
         erasure-coded batch dissemination as an ``invented upgrade.'' This note is the requested \
         literature check of that claim. It finds the claim was wrong: \\textbf{Imitater} \
         \\cite{arxiv2409_19286}, published in September 2024, already erasure-codes mempool \
         microblocks with $(f{+}1,n)$ Reed--Solomon codes and forms $2f{+}1$-signature \
         availability certificates --- the same shape as the design's proposed mechanism, arrived \
         at independently but a year later. Erasure-coded propagation is separately proven at the \
         block-fanout layer in Solana's Turbine and Monad's RaptorCast, and at the blob-availability \
         layer in Ethereum's Data Availability Sampling \\cite{arxiv2407_18085}. Aptos's own \
         engineering team explicitly evaluated erasure coding for their Narwhal-derived Quorum \
         Store and rejected it, reasoning that it adds complexity with no load-balancing benefit \
         over their already-symmetric full-broadcast design --- a real, considered counter-argument \
         from a production system, not an oversight. What remains defensible after this correction is \
         narrow: reuse of an already-built in-tree Reed--Solomon coder, and pairing erasure-coded \
         dispersal with a Narwhal/Bullshark-family DAG-certificate consensus layer rather than \
         Imitater's leader-based one --- a combination not found in this search, though the search \
         was not exhaustive enough to call that a confirmed gap. The purpose of this note is not to \
         defend the original claim but to correct it on the record, in the open, the same day it was \
         made.\n\\end{abstract}\n\n",
    )));

    doc = doc
        .add(Block::Section("Why this note exists".into()))
        .add(raw(
            "The operating discipline for this project (see the accompanying \
             \\texttt{feedback\\_verify\\_before\\_claiming\\_results} memory and the SIGIL skill's own \
             Rule 0 --- ``measure the live path first, no dark victories'') requires checking a claim \
             before stating it, not after. The design document this note audits was written and \
             committed to source control before this check was run. That is the failure this note \
             exists to correct: the honest process is not ``never be wrong,'' it is ``check, and when \
             wrong, say so in public, in the same place the wrong claim was made.'' The design \
             document itself has already been edited in place with a dated correction pointing here; \
             this note is the fuller writeup that correction promises.",
        ))
        .add(raw(
            "Section~\\ref{sec:background} summarizes the actual Narwhal/Bullshark family this design \
             builds on. Section~\\ref{sec:priorart} is the core of the note: what the erasure-coding \
             claim ran into. Section~\\ref{sec:remains} states, at a calibrated confidence level, what \
             is left once the prior art is accounted for. Section~\\ref{sec:lesson} is the process \
             lesson, stated plainly because it is the more durable output of this exercise.",
        ));

    doc = doc
        .add(Block::Section("Background: the Narwhal/Bullshark family".into()))
        .add(Block::Raw("\\label{sec:background}\n".into()))
        .add(raw(
            "Narwhal \\cite{arxiv2105_11827} separates transaction \\emph{dissemination} from \
             transaction \\emph{ordering}. Each validator runs several worker processes; a worker \
             batches transactions from many senders and broadcasts the batch (not the raw \
             transactions) to the corresponding worker on every other validator; each receiver acks \
             with a signature over the batch digest; once a batch collects $2f{+}1$ acks (the standard \
             $n=3f{+}1$ Byzantine quorum), that is a \\emph{certificate of availability}. Validators' \
             primaries then build small headers --- a round number, digests of this round's certified \
             batches, and $2f{+}1$ certificates referencing the previous round's headers --- and these \
             headers chain into a DAG that already proves everything it references is available before \
             consensus looks at it. Consensus (Tusk, or the later Bullshark \\cite{arxiv2201_05677, \
             arxiv2209_05633}) walks this DAG deterministically for a total order, at close to zero \
             extra message cost, because the hard part already happened. \\cite{arxiv2012_06128} \
             surveys this family and its relatives more broadly.",
        ))
        .add(raw(
            "The design under audit pairs a sharded, lock-parallel mempool ingestion layer (its own \
             contribution, not addressed further here since it makes no erasure-coding or DAG-novelty \
             claim) with a certificate layer intended to ride SIGIL's already-shipped GHOSTDAG-style \
             braid \\cite{PHANTOM} instead of building Narwhal's separate header-DAG from scratch --- a \
             reuse of already-built consensus machinery, and the one part of the design this note does \
             not find directly anticipated in the searched literature, with the explicit caveat in \
             \\S\\ref{sec:remains} that the search was not exhaustive.",
        ));

    doc = doc
        .add(Block::Section("The erasure-coding claim and what it ran into".into()))
        .add(Block::Raw("\\label{sec:priorart}\n".into()))
        .add(raw(
            "\\stP\\quad \\textbf{Imitater} \\cite{arxiv2409_19286} (Zeng, Li, Fu, Liu, Jiang; submitted \
             28 Sep 2024, revised 14 Apr 2025) is a Shared Mempool protocol designed to integrate into \
             BFT systems. Its dispersal phase: a distributing node encodes a microblock with an \
             $(f{+}1,\\,n)$ Reed--Solomon code into $n=3f{+}1$ chunks, sends each recipient one chunk \
             plus a Merkle proof, and collects $2f{+}1$ signed acknowledgements into an Availability \
             Certificate. Its retrieval phase, triggered after consensus commits a block: nodes \
             broadcast whatever chunks they hold; once a node collects $f{+}1$ chunks it decodes, \
             \\emph{re-encodes}, and compares the recomputed Merkle root against the committed one \
             before trusting the reconstruction. This is, mechanically, the same shape as the \
             \\texttt{BatchCertificate}/erasure-coded-dispersal mechanism the audited design proposed \
             --- published roughly eleven months earlier.",
        ))
        .add(raw(
            "\\stP\\quad Erasure-coded propagation is independently proven at the block-fanout layer: \
             Solana's Turbine and Monad's RaptorCast both erasure-code block/proposal data for \
             leader$\\to$validator multicast (RaptorCast: Raptor codes per RFC~5053, $2.5\\times$ \
             redundancy, reconstruction from any $k$ of $2.5k$ chunks), and at the blob-availability \
             layer, Ethereum's Data Availability Sampling \\cite{arxiv2407_18085} erasure-codes blob \
             data (a tensor of two Reed--Solomon codes over a $k$-by-$k$ matrix) so light nodes can \
             probabilistically verify availability by sampling instead of downloading. Three different \
             layers, one recurring primitive: $k$-of-$n$ Reed--Solomon coding to prove availability \
             without full replication.",
        ))
        .add(raw(
            "\\stC\\quad Set against that, Aptos's own engineering team \\emph{considered and rejected} \
             erasure coding for Quorum Store, their production Narwhal-derived shared mempool. Their \
             public reasoning: introducing coding would add complexity while yielding no benefit to \
             load balancing, because Quorum Store's symmetric full-broadcast (every validator \
             broadcasts every batch to every other validator) already distributes load evenly across \
             the validator set --- there is no asymmetric sender to relieve, unlike RaptorCast's \
             leader-driven topology, which \\emph{does} have one. This is a real, considered \
             engineering judgment from a team that shipped the alternative, not a gap this note can \
             dismiss.",
        ));

    doc = doc
        .add(Block::Section("What, calibrated, is left".into()))
        .add(Block::Raw("\\label{sec:remains}\n".into()))
        .add(raw(
            "Aptos's counter-argument is about \\emph{load-balancing symmetry}: full replication is \
             already fair, since every validator does identical work. That is a different axis from \
             \\emph{total bandwidth}, where $k$-of-$(k{+}\\mathrm{parity})$ sharding is a real \
             reduction versus full replication independent of fairness --- and Imitater already \
             demonstrates that axis empirically (Imitater-HS outperforms the Stratus baseline in \
             throughput and latency under faults, at up to 256 nodes, per its own evaluation). So the \
             bandwidth argument for coding survives Aptos's specific objection; it just is not new, \
             because Imitater already made it and built it.",
        ))
        .add(raw(
            "\\stN\\quad What the audited design can still honestly claim, at the confidence this note \
             assigns each item:"
        ))
        .add(Block::Raw(String::from(
            "\\begin{enumerate}[leftmargin=1.4em]\n\
             \\item \\textbf{(high confidence)} Reuse of an already-built, already-tested Reed--Solomon \
             coder already present in the target codebase (built originally for chain-snapshot \
             durability), rather than writing a new one --- a real engineering economy specific to \
             this project, explicitly not offered as a research contribution.\n\
             \\item \\textbf{(medium confidence)} Pairing erasure-coded batch dispersal with a \
             Narwhal/Bullshark-\\emph{family} DAG-certificate consensus layer, specifically one already \
             built and running (SIGIL's GHOSTDAG-style braid), rather than Imitater's HotStuff-style \
             leader-based BFT. This combination was not found in the papers this note actually read. It \
             is reported as ``not found in this search,'' not as ``does not exist'' --- the search covered \
             roughly a dozen queries and six fully-read sources, which is a spot-check, not a systematic \
             review.\n\
             \\item \\textbf{(low confidence, adopted not invented)} The re-encode-and-compare integrity \
             check on reconstructed batches (decode, re-encode, compare the committed digest, reject on \
             mismatch) is present in the design's implementation. Imitater does the same thing, published \
             first. This item moved from \\S3.3 of the design document's original ``genuine advance'' \
             framing to this note's explicit non-claim.\n\
             \\end{enumerate}\n\n",
        )))
        .add(raw(
            "None of this supports the original phrase ``invented upgrade beyond stock Narwhal.'' The \
             corrected framing, adopted in the design document as of this note: a proven technique, \
             assembled cheaply from parts already in this specific codebase, applied to a DAG-certificate \
             pairing that --- as far as a same-day, non-exhaustive search could tell --- has not been \
             published in exactly this combination.",
        ));

    doc = doc
        .add(Block::Section("The process lesson".into()))
        .add(Block::Raw("\\label{sec:lesson}\n".into()))
        .add(raw(
            "The more durable output of this exercise is not the citation list; it is the sequencing. \
             The design document was written, and a novelty claim was made in it, before the literature \
             was checked. The check was cheap --- a handful of web searches and two paper fetches, well \
             under an hour --- and it found direct, on-point prior art within the first two queries. The \
             discipline this project has tried to hold throughout this session (verify before claiming, \
             measure the live path before celebrating a benchmark, say ``I don't know'' rather than \
             extrapolate) applies exactly as hard to a novelty claim in a design document as it does to a \
             production metric. It was not applied here, on the first pass, and the fix was to say so \
             plainly rather than quietly soften the wording. That is the point of this note existing as a \
             separate, citable artifact rather than a silent edit.",
        ));

    // ── bibliography (manual thebibliography, matching the established pattern
    // in this crate's other papers — no biblatex dependency needed)
    if !papers.is_empty() {
        let mut bib = String::from("\\begin{thebibliography}{99}\n");
        for p in &papers {
            let mut authors: Vec<String> = p.authors.iter().take(4).map(|a| latex_escape(a)).collect();
            if p.authors.len() > 4 {
                authors.push("et al.".into());
            }
            let year = p.published.get(0..4).unwrap_or("n.d.");
            bib.push_str(&format!(
                "\\bibitem{{{}}} {}: \\emph{{{}}}. arXiv:{} ({}). \\url{{{}}}\n",
                p.cite_key(),
                authors.join(", "),
                latex_escape(&p.title),
                p.id,
                year,
                if p.url.is_empty() {
                    format!("https://arxiv.org/abs/{}", p.id)
                } else {
                    p.url.clone()
                }
            ));
        }
        bib.push_str(
            "\\bibitem{PHANTOM} Yonatan Sompolinsky, Shai Wyborski, Aviv Zohar: \\emph{PHANTOM and GHOSTDAG: \
             A Scalable Generalization of Nakamoto Consensus}. IACR ePrint 2018/104. \
             \\url{https://eprint.iacr.org/2018/104}\n",
        );
        bib.push_str("\\end{thebibliography}\n");
        doc = doc.add(Block::Raw(bib));
    }

    // ── emit
    std::fs::create_dir_all(out_dir).expect("out dir");
    std::fs::write(format!("{out_dir}/sigil_narwhal_investigation.bib"), bibliography(&papers)).expect("bib");
    let res = doc.compile_pdf(out_dir, "SIGIL_NARWHAL_ARXIV_INVESTIGATION_v0");
    if res.success {
        println!("OK {}", res.pdf_path.unwrap());
    } else {
        let tail: String = res
            .log
            .lines()
            .rev()
            .take(50)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        eprintln!("FAILED\n{tail}");
        std::process::exit(1);
    }
}
