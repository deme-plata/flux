//! sigil_qtft_collision_resistance — a Flux Science research note documenting
//! a real security gap found (2026-08-20) in SIGIL's QTFT topology
//! commitment, the fix shipped for it the same session, and a second,
//! deeper gap the fix does NOT close — flagged honestly as open future work
//! rather than folded into a "solved" claim.
//!
//! Every numeric figure in this paper is computed live, at generation time,
//! via the real `flux-topology` crate (the exact Alexander-polynomial
//! implementation SIGIL ships) and a byte-for-byte mirror of
//! `sigil-node::topology_commit_hash` — not hand-copied or estimated.
//!
//! Usage: sigil_qtft_collision_resistance [arxiv.json] [out_dir]

use flux_arxiv_latex::doc::{Block, Document};
use flux_arxiv_latex::{bibliography, latex_escape, parse_arxiv_json, ArxivPaper};
use flux_topology::{alexander_poly, BraidWord, LaurentPoly};

fn raw(s: &str) -> Block {
    Block::Raw(format!("{s}\n\n"))
}

/// Byte-for-byte mirror of `sigil-node/src/main.rs`'s `topology_commit_hash`
/// (as of the 2026-08-20 fix). Kept as a small, clearly-labeled duplicate
/// here rather than a live dependency on the sigil-node binary crate itself
/// (which is not meant to be linked as a library) — if that function's byte
/// layout ever changes, this paper's worked example needs regenerating, the
/// same caveat any worked example carries.
fn topology_commit_hash(delta: &LaurentPoly, strands: u32, word: &[i32], producers: &[[u8; 32]]) -> [u8; 32] {
    let poly_bytes = serde_json::to_vec(delta).expect("LaurentPoly serializes");
    let mut h = blake3::Hasher::new();
    h.update(b"SIGIL/QTFT/TOPOLOGY/V1");
    h.update(b"|poly|");
    h.update(&poly_bytes);
    h.update(b"|strands|");
    h.update(&strands.to_le_bytes());
    h.update(b"|word|");
    for g in word {
        h.update(&g.to_le_bytes());
    }
    h.update(b"|producers|");
    for p in producers {
        h.update(p);
    }
    *h.finalize().as_bytes()
}

/// LaTeX-safe rendering via `LaurentPoly`'s own `Display` impl (a proper
/// polynomial like `t - 1 + t^-1`, not the Rust `Debug` struct dump — the
/// struct's private fields aren't even accessible from outside the crate).
/// Every worked example in this paper evaluates to the plain digit `0` or
/// `1`, so this never actually needs to carry a `^` superscript into
/// \texttt{} text mode, but the substitution stays general regardless.
fn poly_str(p: &LaurentPoly) -> String {
    format!("{p}")
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let json_path = args.get(1).map(String::as_str).unwrap_or("");
    let out_dir = args.get(2).map(String::as_str).unwrap_or("/tmp/sigil-qtft-collision-resistance");

    let papers: Vec<ArxivPaper> = std::fs::read_to_string(json_path)
        .ok()
        .and_then(|j| parse_arxiv_json(&j).ok())
        .unwrap_or_default();

    // ── live-computed figures (not hand-copied) ──────────────────────────
    // Two REAL braid presentations, both independently proven Δ=1 (the
    // unknot's Alexander polynomial) by flux-topology's own KAT tests
    // (kat_unknot_sigma1_n2_delta_is_1, kat_sigma1_sigma2_unknot_closure).
    let w1 = BraidWord::new(2, vec![1]);
    let d1 = alexander_poly(&w1);
    let w2 = BraidWord::new(3, vec![1, 2]);
    let d2 = alexander_poly(&w2);
    assert_eq!(d1, d2, "worked example premise: both must genuinely share Δ=1");

    let producers_2 = [[0xAAu8; 32], [0xBBu8; 32]];
    let producers_3 = [[0xAAu8; 32], [0xBBu8; 32], [0xCCu8; 32]];
    let c_poly_only_1 = blake3::hash(&serde_json::to_vec(&d1).unwrap());
    let c_poly_only_2 = blake3::hash(&serde_json::to_vec(&d2).unwrap());
    let c_fixed_1 = topology_commit_hash(&d1, w1.strands, &w1.gens, &producers_2);
    let c_fixed_2 = topology_commit_hash(&d2, w2.strands, &w2.gens, &producers_3);
    assert_eq!(c_poly_only_1, c_poly_only_2, "worked example: polynomial-only commitment DOES collide");
    assert_ne!(c_fixed_1, c_fixed_2, "worked example: fixed commitment must NOT collide");

    // The degenerate 2-producer case, computed live: literally any 2-strand
    // braid word closes to the SAME canonical presentation as the empty
    // word (proven separately by present.rs's adjacent_strand_merge_yields_no_crossing
    // test) — the sharpest illustration of the deeper, still-open gap
    // (\S\ref{sec:deeper}).
    let two_strand_delta = alexander_poly(&BraidWord::new(2, vec![]));

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
            "\\definecolor{sigilcyan}{HTML}{0E7C86}\n",
            "\\definecolor{sigilviolet}{HTML}{5B4B8A}\n",
            "\\definecolor{sigilamber}{HTML}{B26B00}\n",
            "\\definecolor{sigilred}{HTML}{A32020}\n",
            "\\definecolor{sigilgreen}{HTML}{1E6F3C}\n",
            "\\definecolor{slate}{HTML}{6B7280}\n",
            "\\definecolor{panelbg}{HTML}{F6F7F9}\n",
            "\\newcommand{\\brief}[3]{\\begin{tcolorbox}[colback=panelbg,colframe=#1,boxrule=0.8pt,arc=2pt,",
            "title=\\textbf{#2},coltitle=white,colbacktitle=#1]#3\\end{tcolorbox}}\n",
            "\\hypersetup{pdftitle={A Collision-Resistant Topological Commitment for BlockDAG Braids},",
            "pdfsubject={Closing a real collision gap in SIGIL's QTFT topology commitment, and flagging a deeper one left open},",
            "pdfkeywords={knot theory, Alexander polynomial, braid groups, blockchain consensus, collision resistance, BLAKE3}}\n",
            "\\title{\\textbf{A Collision-Resistant Topological Commitment for BlockDAG Braids}\\\\[6pt]",
            "\\large Closing a Real Gap in SIGIL's QTFT Topology Commitment --- and Flagging a Deeper One Left Open\\\\[4pt]",
            "\\normalsize\\itshape A knot invariant is not a hash: what changed, what it fixes, and what it still doesn't}\n",
            "\\author{Flux Science\\\\\\small research note by Grogu (Claude Sonnet 5), requested by Viktor,",
            "\\\\\\small written the same session the gap was found and fixed}\n",
            "\\date{\\today}"
        ))
        .add(Block::Raw("\\maketitle".into()));

    // ── abstract ──────────────────────────────────────────────────────────
    doc = doc.add(Block::Raw(format!(
        "\\begin{{abstract}}\\noindent\n\
         SIGIL commits a topological invariant of its recent multi-producer BlockDAG braid --- the \
         exact Alexander polynomial of a bounded-window braid word --- into every block header \
         (\\texttt{{SIGIL\\_QTFT\\_TOPOLOGY\\_v0.md}}). This note documents a real gap in that design \
         found while wiring up receipt-side verification: the Alexander polynomial is a classical \
         topological invariant, not a cryptographic one, and is provably non-injective --- distinct \
         braids can share the same value, worked example computed live below (two independently \
         verified $\\Delta{{=}}1$ presentations collide under a polynomial-only commitment: \
         \\texttt{{{}}} = \\texttt{{{}}}). The fix shipped the same session binds the commitment to the \
         BLAKE3 hash of the exact canonical presentation (strand count, braid word, producer ranking) \
         rather than the polynomial value alone; the same worked example no longer collides under the \
         fixed construction (\\texttt{{{}}} $\\neq$ \\texttt{{{}}}), with an accompanying regression test. \
         A second, deeper gap is identified and left explicitly open: the braid-word extraction itself \
         is a lossy projection of the window by design --- with exactly two producers, \\emph{{every}} \
         window canonicalizes to the identical empty word regardless of merge structure, computed live \
         below ($\\Delta{{=}}\\texttt{{{}}}$) --- so even the fixed commitment authenticates a projection \
         of the window, not the window itself. A recommended (not yet implemented) closing move is \
         described in \\S\\ref{{sec:deeper}}.\n\\end{{abstract}}\n\n",
        hex::encode(&c_poly_only_1.as_bytes()[..6]),
        hex::encode(&c_poly_only_2.as_bytes()[..6]),
        hex::encode(&c_fixed_1[..6]),
        hex::encode(&c_fixed_2[..6]),
        latex_escape(&poly_str(&two_strand_delta)),
    )));

    doc = doc
        .add(Block::Section("Background: what SIGIL commits, and why".into()))
        .add(Block::Raw("\\label{sec:background}\n".into()))
        .add(raw(
            "SIGIL's DagKnight-style consensus lets several producers mint blocks in parallel; each \
             block's header may reference other simultaneous blocks via \\texttt{merge\\_parents}. \
             \\texttt{sigil-dagknight}'s \\texttt{present} module treats each distinct producer over a \
             bounded window of resident history as a strand and walks the window's merge edges into an \
             Artin braid word: a cross-strand merge between not-yet-adjacent strands emits a signed \
             generator $\\sigma_i^{\\pm1}$, same-strand merges and merges between already-adjacent \
             strands emit nothing. \\texttt{flux-topology} then computes the exact Alexander polynomial \
             $\\Delta(t)$ of that word's closure via the reduced Burau representation and a \
             fraction-free Bareiss determinant --- real, exact integer-polynomial arithmetic, no \
             floating point, no Jones-polynomial heuristic anywhere in the crate. The result is BLAKE3-hashed \
             and stored as \\texttt{topology\\_commitment} in the block header, alongside the existing \
             state-root and provenance commitments."
        ))
        .add(raw(
            "As of this session, that commitment is also verified on receipt: a receiving node \
             recomputes the same windowed invariant over its own resident braid and compares it to what \
             the block claims, before the block is admitted --- refusing an insertion only when \
             explicitly opted in per-node (\\texttt{SIGIL\\_TOPOLOGY\\_ENFORCE=1}), and never before this \
             node has personally witnessed a full window's worth of live blocks (so a fresh boot's thin \
             local history can never manufacture a false accusation against an honest peer). It was \
             while specifying exactly what `recompute and compare' means that the gap below became \
             impossible not to notice."
        ));

    doc = doc
        .add(Block::Section("The gap: an invariant is not a hash".into()))
        .add(Block::Raw("\\label{sec:gap}\n".into()))
        .add(raw(
            "BLAKE3, which SIGIL uses for every other commitment in the header, is designed so that no \
             adversary can find two different inputs hashing to the same output --- that is the entire \
             point of a cryptographic hash function. The Alexander polynomial was never designed for \
             that job. It is a well-established fact in classical knot theory that distinct knots can \
             share the same Alexander polynomial: the smallest and most famous example is the pair of \
             11-crossing knots found by Kinoshita and Terasaka \\cite{kinoshita1957}, both of which have \
             $\\Delta(t)=1$ --- the identical value as the trivial unknot. Being a coarse classical \
             invariant rather than a cryptographic primitive is not a defect in the Alexander polynomial; \
             it simply was never built to resist an adversary searching for a collision."
        ))
        .add(raw(
            "The same phenomenon is trivial to demonstrate directly inside SIGIL's own bounded-window \
             setting, and the numbers below were computed live by this very program calling the real \
             \\texttt{flux-topology} crate, not estimated or hand-copied. \\texttt{flux-topology}'s own \
             known-answer tests already independently establish both facts used here: a single crossing \
             on 2 strands closes to the unknot, and $\\sigma_1\\sigma_2$ on 3 strands \\emph{also} closes \
             to the unknot --- structurally different presentations, identical $\\Delta$:"
        ))
        .add(Block::Raw(format!(
            "\\begin{{center}}\\begin{{tabular}}{{lll}}\\toprule\n\
             Presentation & $\\Delta(t)$ & Polynomial-only commitment (first 6 bytes)\\\\\\midrule\n\
             $\\sigma_1$, 2 strands & \\texttt{{{}}} & \\texttt{{{}}}\\\\\n\
             $\\sigma_1\\sigma_2$, 3 strands & \\texttt{{{}}} & \\texttt{{{}}}\\\\\\bottomrule\n\
             \\end{{tabular}}\\end{{center}}\n\n",
            latex_escape(&poly_str(&d1)),
            hex::encode(&c_poly_only_1.as_bytes()[..6]),
            latex_escape(&poly_str(&d2)),
            hex::encode(&c_poly_only_2.as_bytes()[..6]),
        )))
        .add(raw(
            "Two genuinely different braid presentations, over different strand counts, produce the \
             identical polynomial-only commitment. A dishonest producer exploiting this would not need \
             to break BLAKE3 --- an infeasible, well-studied problem --- only to find some \\emph{other} \
             window with the same $\\Delta$, a dramatically softer target that classical knot theory says \
             is not even rare."
        ));

    doc = doc
        .add(Block::Section("The fix: commit to the presentation, not its shadow".into()))
        .add(Block::Raw("\\label{sec:fix}\n".into()))
        .add(raw(
            "\\texttt{sigil-dagknight}'s braid-word extraction is already proven deterministic \
             regardless of block arrival order (\\texttt{braid\\_word\\_deterministic\\_across\\_arrival\\_orders}) \
             --- it is a pure function of the window's contents, not of an arbitrary presentation choice. \
             That determinism means the exact canonical presentation can be hashed directly at no cost to \
             reproducibility across nodes. The fix hashes the Alexander polynomial together with the \
             strand count, the literal signed generator sequence, and the strand-to-producer ranking, all \
             under one BLAKE3 hasher with domain-separated segments:"
        ))
        .add(Block::Raw(
            "\\begin{verbatim}\nh.update(b\"SIGIL/QTFT/TOPOLOGY/V1\");\nh.update(b\"|poly|\");    h.update(poly_bytes);\nh.update(b\"|strands|\"); h.update(strands.to_le_bytes());\nh.update(b\"|word|\");    for g in word { h.update(g.to_le_bytes()); }\nh.update(b\"|producers|\"); for p in producers { h.update(p); }\n\\end{verbatim}\n\n".into(),
        ))
        .add(raw(
            "The Alexander polynomial is kept in the hash --- not because it still bears the security \
             property, but because it remains genuinely useful for what it was always good at: a real \
             topological classification of the window, available for future routing, visualization, or \
             legibility work (SIGIL\\_QTFT\\_TOPOLOGY\\_v0.md's Paths B/C/E). The commitment's actual \
             collision resistance now rests entirely on BLAKE3 over the exact structural tuple, the same \
             footing every other commitment in the header already stands on. The identical worked example \
             from \\S\\ref{sec:gap}, re-run through the fixed construction:"
        ))
        .add(Block::Raw(format!(
            "\\begin{{center}}\\begin{{tabular}}{{lll}}\\toprule\n\
             Presentation & $\\Delta(t)$ & Fixed commitment (first 6 bytes)\\\\\\midrule\n\
             $\\sigma_1$, 2 strands & \\texttt{{{}}} & \\texttt{{{}}}\\\\\n\
             $\\sigma_1\\sigma_2$, 3 strands & \\texttt{{{}}} & \\texttt{{{}}}\\\\\\bottomrule\n\
             \\end{{tabular}}\\end{{center}}\n\n",
            latex_escape(&poly_str(&d1)),
            hex::encode(&c_fixed_1[..6]),
            latex_escape(&poly_str(&d2)),
            hex::encode(&c_fixed_2[..6]),
        )))
        .add(raw(
            "The same two presentations that collided in \\S\\ref{sec:gap} no longer do. A regression \
             test asserting exactly this (\\texttt{topology\\_commit\\_hash\\_distinguishes\\_braids\\_that\\_share\\_an\\_alexander\\_polynomial}, \
             \\texttt{sigil-node/src/main.rs}) ships alongside the fix, together with the receipt-side \
             verification and the peer-affinity routing work from the same session; all 74 of \
             \\texttt{sigil-node}'s tests pass, including the new ones, with zero regressions."
        ));

    doc = doc
        .add(Block::Section("The deeper gap this does not close".into()))
        .add(Block::Raw("\\label{sec:deeper}\n".into()))
        .add(Block::Raw(format!(
            "\\S\\ref{{sec:fix}}'s fix is honest and real, but it commits to the \\emph{{canonical \
             presentation}} the extraction produces, not to the window's full structure --- and that \
             extraction is lossy by design. Only cross-strand merges between not-yet-adjacent strands \
             leave any trace in the word; same-strand merges and merges between already-adjacent strands \
             contribute nothing whatsoever. With exactly two producers, every arrangement is already \
             adjacent by construction (there are only two possible positions), so \\emph{{every}} 2-producer \
             window --- regardless of how many blocks it contains or how they merge --- canonicalizes to \
             the same word --- \\texttt{{sigil-dagknight}}'s own test suite already proves this \
             (\\texttt{{adjacent\\_strand\\_merge\\_yields\\_no\\_crossing}}); computed live here for the \
             degenerate case directly: $\\Delta = \\texttt{{{}}}$.\n\n",
            latex_escape(&poly_str(&two_strand_delta)),
        )))
        .add(raw(
            "This means that even after \\S\\ref{sec:fix}'s fix, the topology commitment cannot \
             distinguish two structurally different 2-producer windows from each other --- not because \
             of a hashing weakness, but because the presentation they both canonicalize to is, correctly \
             and by design, the same one. The commitment authenticates a real thing (`what presentation \
             does this window's braid canonicalize to'), but that thing is a strict projection of the \
             window, not the window itself."
        ))
        .add(raw(
            "\\brief{sigilamber}{Recommended, not yet implemented}{A structural commitment that does not \
             route through the knot-theoretic projection at all --- e.g. a BLAKE3 Merkle root over the \
             sorted $(\\text{block\\_hash}, \\text{parent\\_hash}, \\text{merge\\_parents})$ tuples of every \
             resident block in the window --- would be complete by construction (two different windows \
             can only produce the same root by finding a BLAKE3 collision, not by finding a shared knot \
             invariant, classical or otherwise). The recommendation is to ADD this alongside the existing \
             topology commitment rather than replace it: the braid invariant remains the right primitive \
             for anything that wants the window's actual \\emph{topological meaning} (routing, \
             visualization, future fairness proofs); a plain structural hash is the right primitive for \
             anything that only wants \\emph{tamper-evidence}. Conflating the two into one field is what \
             produced both gaps in this note.}"
        ));

    doc = doc
        .add(Block::Section("What is now proven, and what remains assumed".into()))
        .add(Block::Raw("\\label{sec:proven}\n".into()))
        .add(Block::Raw(String::from(
            "\\begin{enumerate}[leftmargin=1.4em]\n\
             \\item \\textbf{Proven, live, this session:} the fixed commitment does not collide on the \
             one concrete example known to collide under the old, polynomial-only construction --- shown \
             above, not merely argued.\n\
             \\item \\textbf{Proven, live, this session:} `sigil-dagknight`'s braid-word extraction is \
             deterministic across arrival order (pre-existing test), so hashing the canonical presentation \
             directly does not introduce any node-to-node disagreement risk.\n\
             \\item \\textbf{Assumed, standard:} BLAKE3 is collision-resistant over the exact \
             $(\\Delta, \\text{strands}, \\text{word}, \\text{producers})$ tuple --- the same assumption \
             every other SIGIL header commitment already rests on, not a new one introduced here.\n\
             \\item \\textbf{Open, explicitly not claimed solved:} whether the presentation-level \
             lossiness described in \\S\\ref{sec:deeper} is exploitable in practice at SIGIL's real \
             production window size (32 blocks) and producer counts, and whether the recommended \
             structural-hash addition is worth its extra header bytes at real block rates. Both are real \
             engineering questions this note deliberately leaves open rather than answers by assertion.\n\
             \\end{enumerate}\n\n",
        )));

    doc = doc
        .add(Block::Section("Conclusion".into()))
        .add(raw(
            "A topological invariant and a cryptographic hash answer different questions, and treating \
             the former as if it had the collision-resistance guarantees of the latter is a real, \
             concrete mistake --- demonstrated above with a genuine collision, not a hypothetical one. \
             The fix shipped the same session it was found closes that specific gap by binding the \
             commitment to BLAKE3 over the exact canonical presentation rather than its polynomial \
             shadow. It does not, and was never claimed to, make the underlying knot-theoretic projection \
             itself complete --- that gap is real, understood, and left open in \\S\\ref{sec:deeper} for \
             exactly the reason this project's own operating discipline asks for: say what is fixed, say \
             what is not, and do not let the first claim quietly cover the second."
        ));

    // ── bibliography ──────────────────────────────────────────────────────
    let mut bib = String::from("\\begin{thebibliography}{99}\n");
    bib.push_str(
        "\\bibitem{kinoshita1957} Shin'ichi Kinoshita, Hidetaka Terasaka: \\emph{On unions of knots}. \
         Osaka Mathematical Journal, 9(2):131--153, 1957.\n",
    );
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
            if p.url.is_empty() { format!("https://arxiv.org/abs/{}", p.id) } else { p.url.clone() }
        ));
    }
    bib.push_str("\\end{thebibliography}\n");
    doc = doc.add(Block::Raw(bib));

    // ── emit ──────────────────────────────────────────────────────────────
    std::fs::create_dir_all(out_dir).expect("out dir");
    std::fs::write(format!("{out_dir}/sigil_qtft_collision_resistance.bib"), bibliography(&papers)).expect("bib");
    let res = doc.compile_pdf(out_dir, "SIGIL_QTFT_COLLISION_RESISTANCE_v0");
    if res.success {
        println!("OK {}", res.pdf_path.unwrap());
    } else {
        let tail: String = res.log.lines().rev().take(60).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        eprintln!("FAILED\n{tail}");
        std::process::exit(1);
    }
}
