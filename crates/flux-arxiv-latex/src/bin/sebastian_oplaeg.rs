//! sebastian_oplaeg — partnership briefing for co-founder Sebastian Psilander,
//! built THROUGH flux-arxiv-latex (Document/Block API + flux-cache, tectonic→pdflatex).
//! pdflatex-compatible: fontenc T1 + lmodern render Danish æøå (no fontspec).
use flux_arxiv_latex::doc::{Block, Document};

fn main() {
    let preamble = r##"
\definecolor{ink}{HTML}{0E1116}
\definecolor{slate}{HTML}{2B3340}
\definecolor{cyanx}{HTML}{14A7C9}
\definecolor{tealx}{HTML}{0FB7A3}
\definecolor{purplex}{HTML}{7C3AED}
\definecolor{paper}{HTML}{FBFCFD}
\definecolor{soft}{HTML}{EEF4F6}
\pagecolor{paper}\color{ink}
\hypersetup{colorlinks=true,urlcolor=cyanx,linkcolor=cyanx}
\titleformat{\section}{\Large\bfseries\color{ink}}{}{0pt}{\colorbox{cyanx}{\textcolor{white}{\,\thesection\,}}\hspace{8pt}}
\titleformat{\subsection}{\large\bfseries\color{slate}}{}{0pt}{}
\titlespacing{\section}{0pt}{16pt}{8pt}
\pagestyle{fancy}\fancyhf{}
\renewcommand{\headrulewidth}{0pt}
\fancyfoot[L]{\footnotesize\color{slate}Flux \textperiodcentered{} SIGIL \textperiodcentered{} Quillon — fortroligt partnerskabs-oplæg}
\fancyfoot[R]{\footnotesize\color{slate}\thepage}
\newtcolorbox{deal}[1]{colback=soft,colframe=tealx,boxrule=1pt,arc=4pt,left=10pt,right=10pt,top=8pt,bottom=8pt,title=#1,fonttitle=\bfseries\color{white},coltitle=white,colbacktitle=tealx}
\newtcolorbox{pillar}[2]{colback=white,colframe=#2,boxrule=1.2pt,arc=3pt,left=10pt,right=10pt,top=7pt,bottom=7pt,title=#1,fonttitle=\bfseries,coltitle=white,colbacktitle=#2}
\setlist[itemize]{leftmargin=16pt,itemsep=2pt,topsep=3pt}
\newcommand{\dt}{\textcolor{cyanx}{\textbf{\textperiodcentered}}\,}
"##;

    let body = r##"
\thispagestyle{empty}
\noindent{\color{cyanx}\rule{\linewidth}{3pt}}\\[2pt]
{\footnotesize\color{slate}PARTNERSKABS-OPLÆG \quad\textbullet\quad 11. juni 2026 \quad\textbullet\quad FORTROLIGT}

\vspace{32mm}
\begin{center}
{\fontsize{29}{34}\selectfont\bfseries\color{ink}FLUX \,\textcolor{cyanx}{\textperiodcentered}\, SIGIL \,\textcolor{tealx}{\textperiodcentered}\, QUILLON}\\[12pt]
{\Large\color{slate}Tre lag, ét økosystem — bygget af én udvikler og en sværm af AI-agenter}\\[24pt]
{\large\color{ink}\textbf{En post-quantum blockchain, dens penge-graf, og motoren der bygger dem begge}}
\end{center}

\vspace{28mm}
\begin{center}
\begin{tcolorbox}[width=0.72\linewidth,colback=soft,colframe=cyanx,boxrule=1pt,arc=4pt,halign=center]
{\footnotesize\color{slate}FORBEREDT TIL}\\[2pt]
{\Large\bfseries\color{ink}Sebastian Psilander}\\[1pt]
{\small\color{slate}medstifter \& partner}\\[10pt]
{\footnotesize\color{slate}AF}\\[2pt]
{\large\bfseries\color{ink}Viktor S. Kristensen}\\[1pt]
{\small\color{slate}grundlægger \& udvikler}
\end{tcolorbox}
\end{center}
\vfill
\noindent{\color{tealx}\rule{\linewidth}{2pt}}
\newpage

\section{Sammenfatning}
Vi bygger en vertikalt integreret stak: en \textbf{AI-native udviklingsplatform (Flux)} der autonomt bygger, reviewer og udruller en \textbf{post-quantum blockchain (SIGIL)} og dens \textbf{penge- og DeFi-graf (Quillon)}. Hele stakken drives af én udvikler forstærket af en sværm af snesevis af AI-agenter — solo-tempo med et helt teams output.

\smallskip
Status i dag: testnettet kører med over \textbf{14 millioner producerede blokke}, multi-platform signerede releases er i produktion (Linux, Windows, ARM, macOS), auto-update er hash-verificeret mod en pinned nøgle, og de første \textbf{browser-noder} er online. Partnerskabet med Sebastian underbygger den infrastruktur og det samarbejde der gør solo-skala ambition holdbar — både teknisk og menneskeligt.

\section{De tre søjler}
\begin{pillar}{FLUX — motoren}{purplex}
Den AI-native udviklingsplatform der bygger alt det andet.
\begin{itemize}
\item \dt \textbf{fluxc} — egen compiler med cache og kryptografiske byggebeviser; kompilerer, krydskompilerer og distribuerer hele workspacet.
\item \dt \textbf{Sværmen} — snesevis af AI-agenter der claimer opgaver, bygger parallelt, reviewer hinanden og deployer autonomt.
\item \dt \textbf{Flux MCP} — over 100 agentiske værktøjer; AI-agenter kan bygge, teste, deploye og bevæge penge under kontrol.
\item \dt \textbf{The Two Minds} — to A100-klynger: én foreslår og handler, én er adversarisk auditor. Penge- og kode-handlinger kræver 2-af-2 godkendelse.
\end{itemize}
\end{pillar}

\smallskip
\begin{pillar}{SIGIL — blockchainen}{cyanx}
Post-quantum fra bunden — værdilaget.
\begin{itemize}
\item \dt \textbf{Post-quantum} signaturer og nøgleudveksling (ML-KEM/Kyber, ML-DSA/Dilithium, SQIsign) — fremtidssikret mod kvantecomputere.
\item \dt \textbf{21M hard cap} med tidsbaseret 4-års halving — Bitcoin-disciplin, knaphed indbygget i konsensus.
\item \dt \textbf{Verify on a potato} — zk-verifikation på ca.\ 10 ms; en fuld node kan verificere kæden på en telefon i light-monitor-tilstand.
\item \dt \textbf{Signerede auto-updates} — hver release hash-verificeres mod en pinned nøgle; en kompromitteret server kan ikke skubbe en ondsindet binær.
\item \dt \textbf{Browser-noder} via js-libp2p — en ægte node i en browser-fane; post-quantum-krypteret P2P-kanal på vej.
\item \dt \textbf{sigil-top} — node, wallet og miner i ét. Multi-platform; 4-node testnet-flåde.
\end{itemize}
\end{pillar}

\smallskip
\begin{pillar}{QUILLON GRAPH — penge-laget}{tealx}
Hvor SIGIL er værdien, er Quillon hvor den bevæger sig og arbejder.
\begin{itemize}
\item \dt \textbf{QUG} og \textbf{QUGUSD} — det native token og en USD-denomineret enhed.
\item \dt \textbf{quillon-wallet} — DEX, bank (lån, 2-af-2), BTC-bridge, Lightning, og RWA (real-world assets).
\item \dt \textbf{Agentic money} — AI-agenter kan bevæge penge under kryptografiske spend-mandater; alt gated.
\item \dt \textbf{qshare / qcredit} — NAV-bakket treasury-andel og collateraliseret kredit (op til 50\% LTV).
\end{itemize}
\end{pillar}

\newpage
\section{Hvad vi allerede har leveret}
\begin{itemize}
\item \dt \textbf{Multi-platform signerede releases i produktion} — Linux, Windows, ARM og macOS; auto-update der rent faktisk virker og er hash-verificeret.
\item \dt \textbf{Browser-noder online} — js-libp2p $\leftrightarrow$ flux-p2p bridge live; browsere bliver ægte peers og modtager blokke over gossipsub.
\item \dt \textbf{Agentic-money MCP} — AI-agenter bevæger penge under 2-af-2 mandater; det fundament fintech-aktører forsøger at bygge, kører her.
\item \dt \textbf{Autonom sværm} — dusinvis af agenter bygger, reviewer og deployer; én udvikler med et teams hastighed.
\item \dt \textbf{Release-safety-gate} — ingen binær når brugere før den faktisk er kørt og bevist stabil. Disciplin, ikke held.
\item \dt \textbf{Over 14M testnet-blokke} — kæden er kørt hårdt og er moden langt forbi proof-of-concept.
\end{itemize}

\section{Partnerskabet}
Dette er kernen. Solo-skala ambition kræver to ting Sebastian leverer: \textbf{infrastruktur} og \textbf{partnerskab}.

\smallskip
\begin{deal}{Hvad Sebastian bidrager}
\begin{itemize}
\item \dt \textbf{Halvdelen af ALLE server-omkostninger} — A100-klyngerne, flåden, hele infrastrukturen der driver sværmen og kæden.
\item \dt \textbf{Uvurderlig medstifter-sparring} — det der gør solo-udvikling holdbar, retningssikker og sjov frem for ensom.
\end{itemize}
\end{deal}

\smallskip
\begin{deal}{Hvad Sebastian får}
\begin{itemize}
\item \dt \textbf{Halvdelen af dev-fee-puljen.} Pt.: \textbf{ca.\ 50M DKK i QUG} \,+\, \textbf{110M QUGUSD}.
\item \dt Andelen vokser i takt med økosystemet — aligned incitamenter, langt ud over de løbende omkostninger.
\end{itemize}
\end{deal}

\smallskip
\noindent\textit{Kort sagt: Sebastian dækker det halve af driften og er medstifter i ånd og handling; til gengæld ejer han halvdelen af dev-fee-puljen — en betydelig og voksende andel af et økosystem der allerede kører.}

\section{Næste skridt}
\begin{itemize}
\item \dt \textbf{Mainnet-hærdning} — safety-gate, signerede releases og chain-reset self-heal som standard.
\item \dt \textbf{Browser-node-udrulning} + post-quantum P2P (Kyber-noise) — en node i enhver fane.
\item \dt \textbf{Penge-grafen i produktion} — dybere DEX, bank, RWA og agentic money.
\item \dt \textbf{GPU-mining på flåden} (gated) — mere hashkraft uden at gå på kompromis med sikkerheden.
\item \dt \textbf{Fortsat solo-dev + sværm-tempo} — finansieret og bakket op af partnerskabet.
\end{itemize}

\section{Talepunkter til mødet}
\begin{enumerate}[leftmargin=18pt,itemsep=3pt]
\item \textbf{Helheden:} tre lag, ét økosystem — motoren (Flux), værdien (SIGIL), pengene (Quillon). De fleste projekter har ét; vi har stakken.
\item \textbf{Modenhed:} ikke et whitepaper — 14M+ testnet-blokke, signerede releases på fire platforme, browser-noder live.
\item \textbf{Forsvarsværk:} post-quantum fra dag ét, signerede auto-updates, verify-on-a-potato. Sikkerhed er fundamentet, ikke en feature.
\item \textbf{Hastighed:} én udvikler + AI-sværm = et helt teams output. Det er moaten.
\item \textbf{Dit bidrag betyder noget:} det halve af driften gør sværmen og flåden mulig — og medstifter-sparringen gør det holdbart.
\item \textbf{Din andel:} halvdelen af dev-fee-puljen (ca.\ 50M DKK QUG + 110M QUGUSD) — den vokser med det vi bygger sammen.
\item \textbf{Spørg ind:} hvad vil du have mere indflydelse på? Hvor ser du den næste store gevinst? Lad os sætte de næste 90 dage sammen.
\end{enumerate}

\vfill
\noindent{\color{cyanx}\rule{\linewidth}{2pt}}\\[2pt]
{\footnotesize\color{slate}Forberedt af Viktor S. Kristensen \textperiodcentered{} til Sebastian Psilander \textperiodcentered{} 11. juni 2026 \textperiodcentered{} fortroligt}
"##;

    let doc = Document::new("article")
        .option("11pt")
        .option("a4paper")
        .package_opt("fontenc", &["T1"])
        .package("lmodern")
        .package_opt("geometry", &["a4paper", "top=22mm", "bottom=20mm", "left=20mm", "right=20mm"])
        .package("xcolor")
        .package("titlesec")
        .package("enumitem")
        .package_opt("tcolorbox", &["most"])
        .package("graphicx")
        .package("fancyhdr")
        .package("hyperref")
        .preamble(preamble)
        .add(Block::Raw(body.into()));

    let out_dir = "/home/storage/tmp/sebastian-oplaeg";
    let res = doc.compile_pdf(out_dir, "sebastian-oplaeg");
    println!("flux-arxiv-latex: success={} pdf={:?}", res.success, res.pdf_path);
    if !res.success {
        let tail = &res.log[res.log.len().saturating_sub(1800)..];
        eprintln!("--- compile log tail ---\n{}", tail);
        std::process::exit(1);
    }
}
