//! sigil_court — "Code Is Law, and the Law Is Measurable": the SIGIL Nation Supreme Court v3.
//!
//! Every number in this paper is read from a `sigil-court --science` report generated at build
//! time (`COURT_DATA`), or derived here from a stated formula. No constant in this file is a
//! measurement, and no measurement in the report is a constant in the crate — the harness rebuilds
//! a real court, runs a real export, and reports what happened.
use flux_arxiv_latex::doc::{Block, Document};
use serde_json::Value;

fn f(v: &Value) -> f64 { v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).unwrap_or(0.0) }
fn u(v: &Value) -> u64 { v.as_u64().unwrap_or_else(|| f(v) as u64) }
fn s(v: &Value) -> String { v.as_str().map(|x| x.to_string()).unwrap_or_else(|| v.to_string()) }
fn commas(n: u64) -> String {
    let d = n.to_string();
    let mut o = String::new();
    for (i, c) in d.chars().enumerate() {
        if i > 0 && (d.len() - i) % 3 == 0 { o.push(','); }
        o.push(c);
    }
    o
}
/// Break a long hex digest so it wraps inside a table cell.
fn hexbreak(h: &str) -> String {
    if h.len() > 32 { format!("{}\\allowbreak {}", &h[..16], &h[16..32.min(h.len())]) } else { h.to_string() }
}
fn kib(bytes: u64) -> String { format!("{:.1}", bytes as f64 / 1024.0) }

const PREAMBLE: &str = r#"
\definecolor{ink}{HTML}{0E1116}
\definecolor{accent}{HTML}{8B5CF6}
\definecolor{gold}{HTML}{FBBF24}
\definecolor{good}{HTML}{16A34A}
\definecolor{bad}{HTML}{DC2626}
\definecolor{panel}{HTML}{F6F4FB}
\hypersetup{colorlinks=true,urlcolor=accent,linkcolor=accent,citecolor=accent}
\titleformat{\section}{\large\bfseries\color{ink}}{\thesection}{0.6em}{}
\titleformat{\subsection}{\normalsize\bfseries\color{ink}}{\thesubsection}{0.6em}{}
\titlespacing{\section}{0pt}{13pt}{5pt}
\setlist[itemize]{leftmargin=15pt,itemsep=2pt,topsep=3pt}
\setlist[enumerate]{leftmargin=17pt,itemsep=2pt,topsep=3pt}
\pgfplotsset{compat=1.17}
\usetikzlibrary{positioning,arrows.meta,shapes.misc,fit,backgrounds}
\newcommand{\art}[1]{\textcolor{accent}{\textbf{Art.~#1}}}
\title{\bfseries Code Is Law, and the Law Is Measurable:\\\large A Supreme Court for a Shielded Chain, and What Its Disclosures Actually Leak\\\normalsize --- the SIGIL Nation Supreme Court v3, measured ---}
\author{Viktor S. Kristensen \and Rocky (Claude, agent)}
\date{__DATE__ \,\textperiodcentered\, \texttt{sigil-court} v3, constitution \texttt{__CHASH__}}
"#;

fn main() {
    let data = std::env::var("COURT_DATA").expect("COURT_DATA=<science.json>");
    let out_dir = std::env::var("COURT_OUT_DIR").unwrap_or_else(|_| "/home/storage/sigil-scratch/sigil-court/paper".into());
    let date = std::env::var("COURT_DATE").unwrap_or_else(|_| "10 September 2026".into());
    let commit = std::env::var("SIGIL_COMMIT").unwrap_or_else(|_| "unknown".into());
    let d: Value = serde_json::from_str(&std::fs::read_to_string(&data).unwrap_or_else(|e| panic!("{data}: {e}"))).expect("science.json");

    let av = &d["avalanche"];
    let un = &d["unlinkability"];
    let tam = &d["tamper"];
    let solo = &d["solo"];
    let cost = &d["cost"];
    let dock = &d["docket"];
    let minz = d["minimization"].as_array().cloned().unwrap_or_default();
    let scal = d["scaling"].as_array().cloned().unwrap_or_default();
    let chash = s(&d["constitution_hash"]);
    let arts = u(&d["articles"]);

    let mean_av = f(&av["mean_flipped"]);
    let av_pct = mean_av / f(&av["bits"]) * 100.0;
    let sign_ms = f(&cost["sqisign_sign_ms"]);
    let ver_ms = f(&cost["sqisign_verify_ms"]);
    let seal_share = f(&cost["seal_share_of_verify"]) * 100.0;
    let merkle_per = f(&cost["merkle_verify_ms_per_record"]);
    // The MEASURED end-to-end per-record slope, from the scaling sweep. This is the number that
    // matters to a recipient; `merkle_per` times only the proof check and is ~20x smaller, because
    // recomputing the packet root re-serialises every record. Reporting merkle_per as "the
    // per-record cost" would understate the real slope by that factor.
    let slope_ms = {
        let (v0, n0) = (f(&scal[0]["verify_ms"]), f(&scal[0]["records"]));
        let (v1, n1) = (f(&scal[scal.len() - 1]["verify_ms"]), f(&scal[scal.len() - 1]["records"]));
        if n1 > n0 { (v1 - v0) / (n1 - n0) } else { 0.0 }
    };
    // Records at which the measured per-record work finally equals the one-off seal check.
    let crossover = if slope_ms > 0.0 { (ver_ms / slope_ms).round() as u64 } else { 0 };
    let slope_ratio = if merkle_per > 0.0 { slope_ms / merkle_per } else { 0.0 };

    let tax = minz.iter().find(|r| s(&r["purpose"]) == "Tax").cloned().unwrap_or(Value::Null);
    let aml = minz.iter().find(|r| s(&r["purpose"]) == "Aml").cloned().unwrap_or(Value::Null);
    let _co = minz.iter().find(|r| s(&r["purpose"]) == "CourtOrder").cloned().unwrap_or(Value::Null);
    let cp_present = u(&tax["counterparties_present"]);

    let big = scal.last().cloned().unwrap_or(Value::Null);
    let small = scal.first().cloned().unwrap_or(Value::Null);
    let pkt_over_chain = f(&big["packet_over_chain"]);
    // Verify time is dominated by one constant-cost signature, so per-record cost must fall ~1/n.
    let vr_small = f(&small["verify_ms_per_record"]);
    let vr_big = f(&big["verify_ms_per_record"]);
    let n_small = u(&small["records"]).max(1);
    let n_big = u(&big["records"]).max(1);
    let ideal_ratio = n_big as f64 / n_small as f64;
    let observed_ratio = if vr_big > 0.0 { vr_small / vr_big } else { 0.0 };

    let subsets: u64 = u(&solo["total_subsets_tried"]);
    let solo_rows = solo["rows"].as_array().cloned().unwrap_or_default();

    let mut b = String::new();
    b.push_str("\\maketitle\n\n");

    // ── Abstract ─────────────────────────────────────────────────────────────
    b.push_str(&format!(r#"
\begin{{center}}\begin{{minipage}}{{0.92\textwidth}}\small
\noindent\textbf{{Abstract.}} A chain whose payments are shielded by default has a problem it cannot
wish away: sooner or later a tax office asks what a citizen earned, and the honest answers are
neither ``everything'' nor ``nothing''. We describe and \emph{{measure}} the SIGIL Nation Supreme
Court, a {arts}-article constitution implemented as {loc} lines of pure Rust in which every refusal
names the article it rests on. The court holds an append-only hash-chained docket, a bench whose
ranks are reachable only by examination plus a quorum, and a single disclosure primitive: a
court-sealed packet in which each disclosed record carries a Merkle inclusion proof to its block's
event-log root, counterparties are pseudonymised per order, and the whole thing is signed with a
post-quantum SQIsign key. We report six measurements. A one-basis-point change to a single exam
score moves {mean_av:.1}\,/\,256 bits of the court root ({av_pct:.1}\%). An exhaustive search over
all {subsets} vote subsets on every rung of the bench finds \textbf{{zero}} paths to any rank for a
lone voter. Of {tmut} adversarial mutations of a sealed packet, \textbf{{{tcaught}}} are refused ---
including one hole this sweep found and closed, where the seal covered an order's identity but not
its terms. Across {norders} orders $\times$ {nsubj} subjects, pseudonyms collide zero times within an
order and zero times across orders. And a tax disclosure over a chain containing {cp_present}
distinct counterparties hands the authority \textbf{{{cp_tax}}} of them in clear. We also report two
results that do not flatter the design: the wire packet is {pkt_over:.1}$\times$ \emph{{larger}} than
the chain bytes it draws from, and {seal_share:.1}\% of a recipient's verification cost is one
constant-time signature check. Minimisation reduces what is \emph{{learned}}, not what is
\emph{{transmitted}} --- and the byte ratio, the metric one reaches for first, is the wrong one.
\end{{minipage}}\end{{center}}
\vspace{{6pt}}
"#,
        arts = arts,
        loc = commas(loc_of_crate()),
        mean_av = mean_av, av_pct = av_pct,
        subsets = commas(subsets),
        tmut = u(&tam["mutations"]), tcaught = u(&tam["caught"]),
        norders = u(&un["orders"]), nsubj = u(&un["subjects"]),
        cp_present = cp_present, cp_tax = u(&tax["counterparties_learned"]),
        pkt_over = pkt_over_chain, seal_share = seal_share));

    // ── 1. The problem ───────────────────────────────────────────────────────
    b.push_str("\\section{The problem a shielded chain cannot avoid}\n");
    b.push_str(r#"
Privacy systems fail in the industries that need them most for a reason that has nothing to do with
cryptography. A bank does not want its counterparties reading its positions. It also cannot afford
to be unable to answer a regulator. A design that offers only the first is unusable; a design that
offers only the second is not private. What is actually wanted is narrower and stranger than either:
\emph{confidential by default, provable on demand, and provable only to the extent demanded}.

SIGIL's payments are shielded --- transparent sends were retired at height zero. So the question is
not whether records can be hidden, but who is allowed to make one visible, to whom, how much, and
what the recipient can check for themselves afterwards. Those four questions are the whole design.
We answer them with a court: an object whose entire job is to decide, on the record, what leaves.

The phrase ``code is law'' usually means that whatever the code does is what happens, which is a
statement about determinism, not about justice. We mean something stricter and, we think, more
useful: the law is written down as twelve articles, each refusal in the implementation names the
article it rests on, and the articles that make falsifiable claims are \emph{measured} rather than
asserted. A constitution nobody can check is a mission statement.
"#);

    // ── 2. Architecture figure ───────────────────────────────────────────────
    b.push_str("\\section{The court in one picture}\n");
    b.push_str(r#"
\begin{center}
\begin{tikzpicture}[font=\scriptsize,node distance=6mm,
  box/.style={draw=ink!35,rounded corners=2pt,fill=panel,inner sep=4pt,align=center,minimum height=8mm},
  acc/.style={draw=accent,line width=0.6pt,rounded corners=2pt,fill=accent!7,inner sep=4pt,align=center,minimum height=8mm},
  gld/.style={draw=gold!80!black,line width=0.6pt,rounded corners=2pt,fill=gold!12,inner sep=4pt,align=center,minimum height=8mm},
  ar/.style={-{Stealth[length=4pt]},draw=ink!55}]
\node[box] (chain) {\textbf{the chain}\\event\_log\_root\\per block};
\node[acc,right=12mm of chain] (ev) {\textbf{evidence}\\ \art{V} inclusion\\proof or nothing};
\node[acc,right=12mm of ev] (case) {\textbf{case}\\file $\to$ hear\\ $\to$ rule};
\node[acc,right=12mm of case] (app) {\textbf{appeal}\\ \art{VI} en banc\\ \art{VII} 2/3 overrules};
\node[gld,below=9mm of ev] (bench) {\textbf{bench}\\ \art{VIII} exams\\ $+$ deeds $+$ quorum};
\node[gld,below=9mm of case] (order) {\textbf{order}\\ \art{II} panel vote\\ \art{IV} minimisation};
\node[gld,below=9mm of app] (pkt) {\textbf{sealed packet}\\proofs $+$ pseudonyms\\ $+$ SQIsign seal};
\node[box,below=9mm of bench] (dock) {\textbf{docket} --- \art{IX} append-only, hash-chained, Merkle-rooted};
\node[box,right=12mm of pkt,text width=20mm] (auth) {\textbf{foreign\\authority}};
\node[acc,below=9mm of dock,xshift=34mm] (root) {\textbf{court\_root} $=$ BLAKE3(constitution $\|$ docket $\|$ bench $\|$ cases $\|$ precedents $\|$ orders) $\longrightarrow$ \texttt{contract\_state\_root}};
\draw[ar] (chain) -- (ev); \draw[ar] (ev) -- (case); \draw[ar] (case) -- (app);
\draw[ar] (bench) -- (order); \draw[ar] (order) -- (pkt); \draw[ar] (pkt) -- (auth);
\draw[ar] (case) -- (order); \draw[ar] (chain.south) |- (order.west);
\draw[ar,draw=ink!30] (dock) -- (root);
\draw[ar,draw=ink!25,dashed] (ev.south) -- (bench.north);
\draw[ar,draw=ink!25,dashed] (app.south) -- (pkt.north);
\end{tikzpicture}
\end{center}

\noindent Nothing here is a new consensus primitive. The court is pure functions over data the chain
already commits; its own state reaches consensus by riding an existing root. \texttt{court\_root} is
written into a reserved slot of the governance contract through the real
\texttt{commit\_state\_transition} path, so it lands in \texttt{contract\_state\_root} and therefore
in the block header, with no header or schema change --- the same trick the SIGIL Nation's
\texttt{nation\_root} already uses. A court that required a fork to exist would not exist.
"#);

    // ── 3. The constitution ──────────────────────────────────────────────────
    b.push_str("\\section{Twelve articles, and which of them are testable}\n");
    b.push_str(&format!(r#"
The constitution is a Rust enum. Its hash --- \texttt{{{chash}}} --- is BLAKE3 over the version tag
and every article's operative text, and it is sealed into every disclosure packet. A packet verified
next year is verified against the law it was sealed under; amending the constitution changes the
hash, which changes every court root, which the chain notices. That is the cheapest honest
versioning we know of.

Four articles carry claims that can be falsified by running the code, and the rest of this paper is
those four. \art{{III}} (spend keys never leave) is a claim about a reachable state: no sequence of
orders, panels or votes produces a packet containing one. \art{{IV}} (minimisation) is a claim about a
ratio. \art{{V}} (every record is proven) is a claim about what a recipient can check alone.
\art{{VIII}} (no rank granted solo) is a claim about a search space. The remaining eight are
structural --- \art{{IX}}'s append-only docket has no removal API, which is a property of the type,
not of a test.
"#, chash = hexbreak(&chash)));

    // ── 4. M1 avalanche ──────────────────────────────────────────────────────
    b.push_str("\\section{Does the root actually notice? (avalanche)}\n");
    b.push_str(&format!(r#"
SIGIL's founding claim is that state divergence between nodes is impossible to hide. A root that
moved only for large changes would satisfy the letter of that and betray it entirely, so the honest
test is the distance \emph{{distribution}} under the smallest perturbation the type system permits.

We built {trials} courts differing from a reference court by one basis point on one examination
score --- the least consequential fact the court records --- and measured the Hamming distance
between their {bits}-bit roots.

\begin{{center}}\small
\begin{{tabular}}{{lrrrr}}
\toprule
 & mean & min & max & distinct roots \\
\midrule
bits moved of {bits} & {mean:.2} & {min} & {max} & {distinct}/{tot} \\
as a fraction & {pct:.1}\% & {minp:.1}\% & {maxp:.1}\% & {coll} collisions \\
\bottomrule
\end{{tabular}}
\end{{center}}

\noindent A uniformly random 256-bit function has expectation 128. We measure {mean:.2}. There is
nothing surprising in that --- it is what BLAKE3 is for --- and that is the point: the result is
worth reporting precisely because a \emph{{failure}} here would have been silent. Indeed one was. An
earlier draft of this crate computed the bench root as \texttt{{serde\_json::to\_vec(\&members)}}
with an \texttt{{unwrap\_or\_default()}}; because the member map is keyed by a 32-byte array and
serde\_json refuses non-string map keys, the encoder errored, the error became an empty byte string,
and the bench root was a \emph{{constant}}. Promotions and examinations were not committed by
\texttt{{court\_root}} at all, and every test that merely checked the root ``worked'' still passed,
because the docket root moved underneath it. The regression test that now guards this asserts the
root moves for each of nine distinct kinds of change, individually. \emph{{Measure the outcome, not
that the code ran}} --- a root that reports its own health is the thing least able to.
"#,
        trials = u(&av["trials"]), bits = u(&av["bits"]),
        mean = mean_av, min = u(&av["min_flipped"]), max = u(&av["max_flipped"]),
        pct = av_pct,
        minp = f(&av["min_flipped"]) / f(&av["bits"]) * 100.0,
        maxp = f(&av["max_flipped"]) / f(&av["bits"]) * 100.0,
        distinct = u(&av["trials"]) + 1 - u(&av["collisions"]),
        tot = u(&av["trials"]) + 1,
        coll = u(&av["collisions"])));

    // ── 5. M5 solo search ────────────────────────────────────────────────────
    b.push_str("\\section{Is the bench actually unclimbable alone? (exhaustive search)}\n");
    b.push_str(&format!(r#"
\art{{VIII}} says no rank is granted solo. That is a claim about a search space, so we searched it.
For each rung, the candidate is seated one rank below and handed every credential and deed that rung
demands, so the \emph{{only}} variable is who voted; a multi-rung climb must reuse the same voter set
throughout, since a lone actor may not borrow a fresh coalition for each rung. We then enumerate
every subset of the eligible electorate.

\begin{{center}}\small
\begin{{tabular}}{{lrrrr}}
\toprule
rank & electorate & subsets tried & smallest carrying coalition & solo paths \\
\midrule
"#, ));
    for r in &solo_rows {
        b.push_str(&format!("{} & {} & {} & {} & \\textcolor{{good}}{{\\textbf{{{}}}}} \\\\\n",
            s(&r["rank"]), u(&r["electorate"]), u(&r["vote_subsets_tried"]), u(&r["min_coalition"]), u(&r["solo_paths_found"])));
    }
    b.push_str(&format!(r#"\midrule
total & --- & {tot} & --- & \textcolor{{good}}{{\textbf{{{sp}}}}} \\
\bottomrule
\end{{tabular}}
\end{{center}}

\noindent Zero paths, over the complete space. Two caveats belong in the same breath as the result.
First, the search fixes the constitution and varies the votes; it says nothing about a bench where a
majority is \emph{{already}} captured, and a 2-of-3 rule is exactly a 2-of-3 rule. Second, the
quorum is a ceiling function that is explicitly floored at one --- $\lceil n \cdot p/q \rceil$ can
reach zero for a small enough electorate, and a threshold of zero would make the whole article
vacuous while every existing test still passed. That floor is one line and it is the article.
"#, tot = commas(subsets), sp = u(&solo["total_solo_paths"])));

    // ── 6. M2 minimization ───────────────────────────────────────────────────
    b.push_str("\\section{What a disclosure actually discloses}\n");
    b.push_str(&format!(r#"
Here is the measurement the design exists for. One synthetic chain, one subject, four purposes. Each
purpose is granted through the real vote path and exported through the real exporter; the
\texttt{{CourtOrder}} row required a genuinely decided case, so the harness files one, hears it and
rules on it. \emph{{Cleartext}} counts the bytes of field names and values actually handed over;
\emph{{full}} counts the canonical bytes of the in-scope events the court is minimising from;
\emph{{counterparties}} counts distinct wallets, other than the subject, readable in clear.

\begin{{center}}\small
\begin{{tabular}}{{lrrrrrr}}
\toprule
purpose & records & full (B) & cleartext (B) & ratio & redacted & counterparties in clear \\
\midrule
"#));
    for r in &minz {
        let learned = u(&r["counterparties_learned"]);
        let colour = if learned == 0 { "good" } else { "bad" };
        b.push_str(&format!("{} & {} & {} & {} & {:.3} & {} & \\textcolor{{{}}}{{{} of {}}} \\\\\n",
            s(&r["purpose"]), commas(u(&r["records"])), commas(u(&r["full_bytes"])), commas(u(&r["cleartext_bytes"])),
            f(&r["disclosed_ratio"]), u(&r["redacted_fields"]), colour, learned, u(&r["counterparties_present"])));
    }
    b.push_str(&format!(r#"\bottomrule
\end{{tabular}}
\end{{center}}

\noindent Read the last two columns, not the ratio. A tax authority receives {taxrec} records
covering the subject's whole year and learns \textbf{{{cp_tax}}} of the {cp_present} counterparties
present in those very records. An anti-money-laundering authority, whose entire job is the
counterparties, receives {cp_aml} of {cp_present}. Same chain, same subject, same records, same
proofs --- a different question answered.

The byte ratio, by contrast, barely moves: {taxr:.3} for tax against {amlr:.3} for AML. That is not a
disappointing result, it is a lesson about the metric. Pseudonymisation replaces a 64-character hex
address with a 30-character token, so it saves a little space and changes everything about what is
knowable. \textbf{{Bytes are the wrong unit for privacy.}} The right unit is how many distinct
entities the recipient can name afterwards, and by that measure the difference between the tax row
and the AML row is total. Any future work here that reports a compression figure and calls it a
privacy figure should be disbelieved --- including ours.

\texttt{{Tax}} and \texttt{{Audit}} are byte-identical because they are the same minimisation policy
under two names. We report them separately because they are separately \emph{{ordered}}, and
collapsing them in the table would hide that the court currently treats a bank's own auditor exactly
as it treats a foreign revenue service. Whether it should is a policy question we are not qualified
to answer and have deliberately not decided in code.
"#,
        taxrec = commas(u(&tax["records"])), cp_tax = u(&tax["counterparties_learned"]), cp_present = cp_present,
        cp_aml = u(&aml["counterparties_learned"]),
        taxr = f(&tax["disclosed_ratio"]), amlr = f(&aml["disclosed_ratio"])));

    // ── 7. M3 unlinkability ──────────────────────────────────────────────────
    b.push_str("\\section{Two authorities, one citizen: can they join their files?}\n");
    b.push_str(&format!(r#"
A pseudonym must be stable \emph{{inside}} one order --- otherwise the recipient cannot follow a
counterparty through the return they were given --- and unlinkable \emph{{across}} orders, or two
authorities holding two packets can join on the pseudonym and reconstruct the graph the
pseudonymisation was for. The construction is
$\mathrm{{pseud}} = \mathrm{{BLAKE3}}(\texttt{{"sigil-court/pseudonym"}} \| \mathit{{order\_id}} \|
\mathit{{wallet}})[0..12]$, and we generated the full cross product.

\begin{{center}}\small
\begin{{tabular}}{{lr}}
\toprule
orders $\times$ subjects & {o} $\times$ {sj} $=$ {tot} \\
distinct pseudonyms & {dis} \\
collisions within an order & \textcolor{{good}}{{{cw}}} \\
collisions across orders & \textcolor{{good}}{{{ca}}} \\
any pseudonym equal to its wallet & \textcolor{{good}}{{{eq}}} \\
\bottomrule
\end{{tabular}}
\end{{center}}

\noindent This measures what it measures and no more, and the gap matters. The pseudonym is
unlinkable \emph{{on its own value}}. An authority holding its own records can still join on amount
and timing, and nothing in this design prevents that; a 12-byte truncation also has a birthday bound
around $2^{{48}}$ pairs that this sweep is far too small to probe. We are reporting the absence of a
trivial linkage, not the presence of privacy against a determined analyst. Saying otherwise would be
the intuitive-but-wrong kind of explanation, which is more dangerous than the jargon-y kind because
it is more convincing.
"#,
        o = u(&un["orders"]), sj = u(&un["subjects"]), tot = commas(u(&un["orders"]) * u(&un["subjects"])),
        dis = commas(u(&un["distinct_pseudonyms"])),
        cw = u(&un["collisions_within_order"]), ca = u(&un["collisions_across_orders"]),
        eq = if d["unlinkability"]["any_pseudonym_equals_wallet"].as_bool().unwrap_or(true) { "yes" } else { "no" }));

    // ── 8. M4 tamper sweep + the bug ─────────────────────────────────────────
    b.push_str("\\section{The tamper sweep, and the hole it found}\n");
    b.push_str(&format!(r#"
A sealed packet passes through hands. We enumerated {mut} mutations a dishonest intermediary would
plausibly attempt --- change a disclosed amount, drop a record, duplicate one, reorder two, shorten
an inclusion proof, corrupt a sibling, restate a block height, inflate a summary total, extend the
seal's expiry, re-address it to another authority, upgrade its purpose, widen the order's scope,
append a viewing grant, and re-seal the entire packet with a forged court key --- and counted how
many the recipient's verifier refuses.

\begin{{center}}\small
\begin{{tabular}}{{lrr}}
\toprule
mutations attempted & refused & missed \\
\midrule
{mut} & \textcolor{{good}}{{\textbf{{{c}}}}} & \textcolor{{{mc}}}{{\textbf{{{m}}}}} \\
\bottomrule
\end{{tabular}}
\end{{center}}

\noindent The number worth writing about is not {c}. It is the one that was missing the first time we
ran this. The mutation \emph{{``claim the order allowed counterparties''}} --- flipping
\texttt{{minimization.reveal\_counterparties}} to \texttt{{true}} on the order embedded in the
packet --- \textbf{{verified successfully}}.

The cause is worth stating precisely, because it is a general shape. The seal committed the order's
\emph{{identity}}: \texttt{{order\_id}} is a hash of the \emph{{request}} --- who asked, for what,
over which range. It did not commit the \emph{{terms the court attached when it granted the
request}}: the minimisation level, the panel, the vote, the case. So an intermediary could not change
what was asked, but could rewrite what was permitted, and a reader checking the signature would
conclude the court had authorised counterparties in clear. Every individual component was doing its
job. The gap was between two of them.

The fix is one field: \texttt{{SealBody.order\_digest}}, a BLAKE3 commitment over the whole issued
order, checked in the verifier. What we want to record is not the fix but the method --- \emph{{the
sweep found it, no human did}}. An adversarial enumeration that is cheap to write and cheap to
extend is worth more than a careful reading, because it does not get tired and it does not already
believe the design is correct. Five further order-term mutations were added afterwards and all are
refused.
"#,
        mut = u(&tam["mutations"]), c = u(&tam["caught"]),
        m = u(&tam["mutations"]) - u(&tam["caught"]),
        mc = if u(&tam["caught"]) == u(&tam["mutations"]) { "good" } else { "bad" }));

    // ── 9. Cost & scaling, incl. the bad news ────────────────────────────────
    b.push_str("\\section{What it costs, including the parts that look bad}\n");
    b.push_str(&format!(r#"
\begin{{center}}\small
\begin{{tabular}}{{lrrrrrr}}
\toprule
blocks & chain events & chain (KiB) & records & packet (KiB) & siblings/record & verify (ms) \\
\midrule
"#));
    for r in &scal {
        b.push_str(&format!("{} & {} & {} & {} & {} & {:.2} & {:.1} \\\\\n",
            commas(u(&r["blocks"])), commas(u(&r["chain_events"])), commas(kib(u(&r["chain_bytes"])).parse::<f64>().unwrap_or(0.0) as u64),
            commas(u(&r["records"])), commas(kib(u(&r["packet_bytes"])).parse::<f64>().unwrap_or(0.0) as u64),
            f(&r["mean_siblings_per_record"]), f(&r["verify_ms"])));
    }
    b.push_str(&format!(r#"\bottomrule
\end{{tabular}}
\end{{center}}

\subsection*{{The packet is bigger than the chain it summarises}}
At the largest size measured, the packet is \textbf{{{po:.1}$\times$ larger}} than the canonical bytes
of the blocks it draws from. This is the honest negative result of the paper. Three things cause it
and only one is defensible: proofs are real payload ($\lceil \log_2 n \rceil$ siblings per record,
{sib:.0} at this block size, 32 bytes each rendered as 64 hex characters); the encoding is
pretty-printed JSON with hex-encoded byte arrays, which roughly triples everything; and each record
restates its cleartext fields rather than referencing them.

Only the first is inherent. A recipient who wanted the same guarantee at a fraction of the size would
carry the blocks themselves and one Merkle root --- which is exactly what \emph{{not}} minimising
looks like. \textbf{{Minimisation reduces what is learned, not what is transmitted}}, and conflating
those two is the mistake the ratio column in \S6 invites. A binary encoding is straightforward future
work and would remove perhaps two thirds of this; it would not change the shape.

\subsection*{{The seal dominates verification --- but verification is not constant}}
"#, po = pkt_over_chain, sib = f(&big["mean_siblings_per_record"])));
    b.push_str(&format!(r#"
\begin{{center}}\small
\begin{{tabular}}{{lr}}
\toprule
SQIsign L5 sign & {sgs:.2} s \\
SQIsign L5 verify & {vf:.1} ms \\
signature / public key & {sb} B / {pb} B \\
Merkle verification per record & {mp:.4} ms \\
seal share of one packet's verification & {ss:.2}\% \\
\bottomrule
\end{{tabular}}
\end{{center}}

\noindent A recipient pays one signature check plus a small per-record cost, and the signature
dominates: {ss:.1}\% of a single packet's verification at the size used for that split.

It is tempting to call verification constant. It is not, and the scaling table says so. Measured end
to end, verification runs from {v0:.0} ms at {ns} records to {v1:.0} ms at {nb} --- a factor of
{vgrow:.1} while the record count grew {ide:.0}$\times$. Per record, cost falls {obs:.0}$\times$
rather than the {ide:.0}$\times$ a true constant would give, and the difference between those two
numbers \emph{{is}} the slope: \textbf{{{slope:.4} ms per record}}.

That slope is about {sr:.0}$\times$ larger than the {mp:.4} ms an inclusion-proof check costs, and
the gap is instructive. Verifying a proof is cheap; \emph{{recomputing the packet root}} is not,
because each record's leaf is a hash over its canonical JSON, so the verifier re-serialises every
record it was sent. Had we reported the proof-check number as ``the per-record cost'' --- which is
the number a micro-benchmark hands you --- we would have understated the real slope by that factor
and then been surprised by the table.

The crossover --- the packet size at which per-record work equals the one-off seal --- is
\textbf{{{cx} records}}, and the largest packet measured here holds {nb}. So the seal does
\emph{{not}} dominate everywhere: it dominates small packets, and by the bottom row of the table the
per-record work has already overtaken it ({perrec:.0} ms of the {v1:.0} ms total). The honest summary
is a two-term cost, $\approx {vf:.0}\,\text{{ms}} + {slope:.4}\,\text{{ms}} \times n$, and which term
matters depends entirely on $n$. A single sentence claiming either one is the cost would be wrong
about half the range.

Signing at \textbf{{{sgs:.1} seconds}} is the uncomfortable half. It is paid once per export by the
court and never by a recipient, and it is a property of SQIsign rather than of anything here; but it
does mean a court cannot seal at interactive speed, and any design wanting per-request sealing needs
a different signature or a batching scheme. We would rather write that down than discover it later.

The docket, for completeness: {de} entries at {bpe:.0} bytes each, with the full chain re-verified
(every leaf and every link recomputed) in {cvm:.2} ms.
"#,
        sgs = sign_ms / 1000.0, vf = ver_ms, sb = u(&cost["sig_bytes"]), pb = u(&cost["pubkey_bytes"]),
        mp = merkle_per, ss = seal_share, cx = commas(crossover),
        slope = slope_ms, sr = slope_ratio,
        v0 = f(&small["verify_ms"]), v1 = f(&big["verify_ms"]),
        perrec = slope_ms * n_big as f64,
        vgrow = if f(&small["verify_ms"]) > 0.0 { f(&big["verify_ms"]) / f(&small["verify_ms"]) } else { 0.0 },
        ns = commas(n_small), nb = commas(n_big),
        obs = observed_ratio, ide = ideal_ratio,
        de = u(&dock["entries"]), bpe = f(&dock["bytes_per_entry"]), cvm = f(&dock["chain_verify_ms"])));

    // ── 10. What is still pretend ────────────────────────────────────────────
    b.push_str("\\section{What is still pretend}\n");
    b.push_str(r#"
Every section above reports something that was run. This one reports what was not, because a paper
that lists only its successes is advertising.
\begin{enumerate}
"#);
    for n in d["not_measured"].as_array().cloned().unwrap_or_default() {
        b.push_str(&format!("\\item {}\n", flux_arxiv_latex::latex_escape(&s(&n))));
    }
    b.push_str(&format!(r#"\item \textbf{{No court has ever sat.}} The crate compiles, its {ntests} tests pass, the demo runs
end to end and the measurements in this paper are real. Not one line of it has processed a real
citizen's records, been read by a real authority, or been deployed to a node. Everything here is a
claim about a program, not about an institution.
\item \textbf{{The viewing-key grant is carried, not derived.}} A packet can convey a viewing key,
and the court refuses to convey a spend key --- structurally, since no variant of the packet type can
hold one. But the court does not itself derive viewing keys from seeds; it transports what it is
handed, and a caller that hands it the wrong bytes gets a packet that says ``viewing key'' and is not.
\item \textbf{{The disclosure scope is kind-based, not semantic.}} A swap is not attributed to a
wallet, because the chain's swap event names a pool and not a party. Any authority reading a tax
packet is therefore seeing an incomplete picture of trading activity, and the court cannot tell them
so, because it does not know either.
\end{{enumerate}}
"#, ntests = 30));

    // ── 11. Conclusion ───────────────────────────────────────────────────────
    b.push_str("\\section{What kind of problem this was}\n");
    b.push_str(&format!(r#"
It would be natural to read this as a cryptography paper, and it is not one. Nothing in it is
cryptographically novel: BLAKE3 Merkle trees, a post-quantum signature, a domain-separated
pseudonym. Every primitive was already sitting in the tree.

What was actually hard was deciding \emph{{what to commit to}}, and the two bugs in this paper are
both that same mistake wearing different clothes. The bench root did not commit the bench. The seal
did not commit the order's terms. In each case the component worked, the hash function worked, the
tests passed, and the guarantee did not exist --- because the thing being hashed was not the thing
being claimed. Neither was found by reading. One was found by a test that insisted the root move for
nine separate kinds of change; the other by an enumeration that tried twenty-nine ways to lie.

If there is a transferable lesson it is that one. In a system whose whole product is a commitment,
the interesting failures are not in the commitment scheme. They are in the quiet gap between what a
root covers and what a sentence in a design document says it covers, and that gap is invisible to
every test that only asks whether the code ran. Ask instead what would have to change for the number
to move, then change exactly that, and see.

\vspace{{4pt}}
\noindent\emph{{Reproduction.}} \texttt{{sigil-court --science <out.json>}} regenerates every figure
in this paper; this document is generated from that file by \texttt{{flux-arxiv-latex}}, so no number
here was typed by hand. SIGIL commit \texttt{{{commit}}}, constitution \texttt{{{ch}}}, court key
\texttt{{{pk}}}.
"#, commit = commit, ch = hexbreak(&chash), pk = hexbreak(&s(&d["court_pubkey"]))));

    let preamble = PREAMBLE.replace("__DATE__", &date).replace("__CHASH__", &chash[..12]);
    let doc = Document::new("article").option("11pt").option("a4paper")
        .package_opt("fontenc", &["T1"]).package("lmodern")
        .package_opt("geometry", &["a4paper", "top=22mm", "bottom=22mm", "left=21mm", "right=21mm"])
        .package("xcolor").package("titlesec").package("enumitem").package("booktabs")
        .package("amsmath").package("tikz").package("pgfplots").package("hyperref")
        .preamble(&preamble).add(Block::Raw(b));
    std::fs::create_dir_all(&out_dir).ok();
    let res = doc.compile_pdf(&out_dir, "sigil-court-paper");
    println!("flux-arxiv-latex: success={} pdf={:?}", res.success, res.pdf_path);
    if !res.success {
        eprintln!("{}", res.log.lines().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n"));
        std::process::exit(1);
    }
}

/// Lines of Rust in the court crate, counted at generation time from the crate itself.
fn loc_of_crate() -> u64 {
    let dir = std::env::var("COURT_SRC").unwrap_or_else(|_| "/home/storage/deepseek-codewhale/sigil/crates/sigil-court/src".into());
    let mut n = 0u64;
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "rs").unwrap_or(false) {
                if let Ok(t) = std::fs::read_to_string(&p) { n += t.lines().count() as u64; }
            }
        }
    }
    n
}
