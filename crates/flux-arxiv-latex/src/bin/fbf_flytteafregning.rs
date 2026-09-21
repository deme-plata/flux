//! fbf_flytteafregning — juridisk notat om Frederikshavn Boligforenings
//! flytteafregning for Sæbygårdvej 38, 8, 9300 Sæby (lejemål 1 35 3255 6),
//! bygget GENNEM flux-arxiv-latex (Document/Block API, tectonic→pdflatex).
//!
//! Alt talmateriale er tastet fra de to PDF'er i
//! `SigilGraph bank/frederikshavn boligforening/` og regnes efter i Rust
//! (øre-heltal) — notatet trykker aldrig et tal, programmet ikke selv har lagt sammen.
//! Lovteksten (almenlejeloven) er hentet 2026-09-21 fra danskelove.dk
//! (konsolideret tekst; officiel kilde retsinformation.dk, LBK 928/2019 m. senere ændringer).
//! pdflatex-kompatibel: fontenc T1 + lmodern (æøå uden fontspec).
use flux_arxiv_latex::doc::{Block, Document};
use flux_arxiv_latex::latex_escape;

// ───────────────────────── talmateriale (øre) ─────────────────────────

/// Én linje i afregningen. `oere` er beløbet i øre, positivt = udlejers krav.
#[derive(Clone, Copy)]
struct Post {
    label: &'static str,
    leverandoer: &'static str,
    dato: &'static str,
    oere: i64,
}

const INDSKUD: i64 = 3_290_000;

const NORMAL_ISTANDSAETTELSE: [Post; 2] = [
    Post { label: "Normal istandsættelse (faktura 105679, rekv. 129018)", leverandoer: "FBF Malerafd.", dato: "03-06-2026", oere: 478_750 },
    Post { label: "Normal istandsættelse (faktura 105680, rekv. 129019)", leverandoer: "FBF Malerafd.", dato: "03-06-2026", oere: 94_750 },
];
/// "Fradrag for istandsættelse 66,00 %" — § 26, stk. 2: udlejer overtager 1 pct./md.
const FRADRAG_PCT: i64 = 66;

const MISLIGHOLDELSE: [Post; 4] = [
    Post { label: "Udkald til opluk af lejlighed og postkasse + udskiftning af cylinder (faktura 9387, rekv. 128896)", leverandoer: "HNR Låseservice ApS", dato: "12-05-2026", oere: 178_250 },
    Post { label: "Tømning af lejemål til genbrugspladsen, bil x 2 mand 1,5 t (faktura 10220, rekv. 128895)", leverandoer: "Tage Johansens Flytteforretning", dato: "07-05-2026", oere: 169_875 },
    Post { label: "Rengøring", leverandoer: "FBF Rengøringsafd.", dato: "(ingen faktura vedlagt)", oere: 96_000 },
    Post { label: "Polishbehandling 2X, 28,54 m², køkken+stue+værelse 1 (faktura 2026017, rekv. 129388)", leverandoer: "Hørby Gulv I/S", dato: "31-05-2026", oere: 200_600 },
];

const ANDEN_REGNING: [Post; 1] = [
    Post { label: "Advokatomkostninger: fogedgebyr 750 + befordring 118,20 + salær 3.000 + moms 750 (faktura 109105, sag 98828)", leverandoer: "Advodan Aalborg", dato: "10-06-2026", oere: 461_820 },
];

/// Huslejeblokken som FBF selv opgør den (side 2).
const SKYLDIG_HUSLEJE: i64 = 2_650_547;
const HUSLEJERESTANCE_REST: i64 = 598_412;
const AFREGNING_VARME: i64 = -133_939;
const AFREGNING_VAND: i64 = -192_586;

/// FBF's egen bundlinje: "Vores tilgodehavende −9.339,69".
const FBF_BUNDLINJE: i64 = -933_969;

/// Huslejelinjerne side 2–3, i den rækkefølge de står (uden den særskilte
/// restancelinje 5.984,12 på 01-05-2026, som FBF fører for sig).
const HUSLEJELINJER: [(&str, &str, i64); 35] = [
    ("01-01-2026", "Leje Ældreboliger", 835_600), ("01-01-2026", "A/c Varme", 38_500), ("01-01-2026", "A/c Vand", 44_000),
    ("01-01-2026", "Huslejerestance (afdrag)", 50_000), ("01-01-2026", "Boligstøtte", -329_700), ("01-01-2026", "Påkravsgebyr", 32_900), ("01-01-2026", "Vaskeriafregning", 3_700),
    ("01-02-2026", "Leje Ældreboliger", 835_600), ("01-02-2026", "A/c Varme", 38_500), ("01-02-2026", "A/c Vand", 44_000),
    ("01-02-2026", "Huslejerestance (afdrag)", 50_000), ("01-02-2026", "Boligstøtte", -329_700), ("01-02-2026", "Påkravsgebyr", 32_900), ("01-02-2026", "Vaskeriafregning", 5_625),
    ("01-03-2026", "Leje Ældreboliger", 835_600), ("01-03-2026", "A/c Varme", 38_500), ("01-03-2026", "A/c Vand", 44_000),
    ("01-03-2026", "Huslejerestance (afdrag)", 50_000), ("01-03-2026", "Boligstøtte", -329_700), ("01-03-2026", "Påkravsgebyr", 32_900), ("01-03-2026", "Vaskeriafregning", 1_000),
    ("01-04-2026", "Leje Ældreboliger", 835_600), ("01-04-2026", "A/c Varme", 38_500), ("01-04-2026", "A/c Vand", 44_000),
    ("01-04-2026", "Huslejerestance (afdrag)", 50_000), ("01-04-2026", "Boligstøtte", -329_700), ("01-04-2026", "Påkravsgebyr", 32_900),
    ("01-05-2026", "Leje Ældreboliger", 835_600), ("01-05-2026", "A/c Varme", 38_500), ("01-05-2026", "A/c Vand", 44_000),
    ("01-05-2026", "Huslejerestance (afdrag)", 50_000), ("01-05-2026", "Boligstøtte", -327_600), ("01-05-2026", "Afregning varme", -222_506),
    ("01-05-2026", "Afregning vand", -417_422), ("01-05-2026", "Påkravsgebyr", 32_900),
];
const PAAKRAVSGEBYR_OERE: i64 = 32_900;

// ───────────────────────── lovtekst (verbatim, hentet 2026-09-21) ─────────────────────────

const LOV: &[(&str, &str)] = &[
    ("§ 25, stk. 3", "Det kan ikke ved fraflytning forlanges, at boligen afleveres i en bedre stand end den, hvori den blev overtaget."),
    ("§ 25, stk. 4", "Lejeren skal uanset reglerne i §§ 26 og 27 afholde samtlige udgifter som følge af misligholdelse, hvorved det lejede er forringet eller skadet som følge af fejlagtig brug, fejlagtig vedligeholdelse eller uforsvarlig adfærd af lejeren, medlemmer af dennes husstand eller andre, som lejeren har givet adgang til boligen."),
    ("§ 26, stk. 1", "Har udlejeren efter § 25, stk. 1, truffet beslutning herom, sørger lejeren for og afholder udgiften til at vedligeholde boligen indvendigt med hvidtning, maling, tapetsering og gulvbehandling i boperioden."),
    ("§ 26, stk. 2", "Ved fraflytning af boligen gennemføres for lejerens regning en normalistandsættelse, der omfatter nødvendig hvidtning, maling og tapetsering af vægge og lofter samt rengøring. Udlejeren kan beslutte, at lejeren i stedet betaler et af udlejeren fastsat hertil svarende normalistandsættelsesbeløb. Udlejeren overtager i løbet af en periode på højst 10 år fra lejerens overtagelse af lejligheden gradvis lejerens udgift til normalistandsættelsen eller betaling af normalistandsættelsesbeløbet. Ved fraflytning inden udløbet af den fastsatte periode betaler lejeren kun den andel, som på det tidspunkt, hvor lejemålet ophører, ikke er overtaget af udlejeren."),
    ("§ 27", "Har udlejeren efter § 25, stk. 1, truffet beslutning herom, vedligeholder udlejeren boligen indvendigt med hvidtning, maling, tapetsering og gulvbehandling i boperioden. De nødvendige midler tilvejebringes ved lejerens indbetaling af et beløb til en vedligeholdelseskonto for boligen. Beløbet fastsættes af udlejer til et årligt beløb pr. m² bruttoetageareal. Lejeren kan forlange, at der udføres vedligeholdelse af boligen med hvidtning, maling, tapetsering og gulvbehandling, når det er nødvendigt og udgifterne kan dækkes af boligens vedligeholdelseskonto."),
    ("§ 88, stk. 2", "Fraflytter lejeren inden opsigelsesperiodens udløb, skal udlejeren bestræbe sig på at genudleje det lejede. Hvad udlejeren indvinder eller burde have indvundet ved genudlejning, skal fragå i udlejerens krav over for lejeren."),
    ("§ 90, stk. 2", "Udlejeren kan kun hæve lejeaftalen som følge af for sen betaling, hvis lejeren ikke har berigtiget restancen senest 14 dage efter, at skriftligt påkrav herom er kommet frem til lejeren. Udlejerens påkrav kan tidligst afgives efter 3. hverdag efter sidste rettidige betalingsdag og skal udtrykkeligt angive, at lejeforholdet kan ophæves, hvis lejerestancen ikke betales inden fristens udløb. [...] Som gebyr for påkravet kan udlejeren kræve 250 kr. Det i 5. pkt. nævnte beløb er opgjort i 2009-niveau og reguleres én gang årligt efter udviklingen i Danmarks Statistiks nettoprisindeks [...]. Gebyret er pligtig pengeydelse i lejeforholdet."),
    ("§ 92, stk. 1", "Når udlejeren hæver lejeaftalen, skal lejeren straks fraflytte og betale leje m.v. for tiden, indtil lejeren kunne flytte med sædvanligt varsel, jf. § 88. Lejeren skal endvidere erstatte udlejeren ethvert tab, herunder lejetab og omkostningerne ved lejerens udsættelse af det lejede."),
    ("§ 92, stk. 2", "Indgiver udlejeren anmodning til fogedretten om lejerens udsættelse af det lejede på grund af betalingsmisligholdelse, skal udlejeren senest samtidig med anmodningens indgivelse til fogedretten underrette kommunen om, at en sag om lejerestance er indgivet til fogedretten. Underretningen til kommunen skal være skriftlig og indeholde oplysning om lejerens navn og adresse."),
    ("§ 92, stk. 3", "Udlejeren skal bestræbe sig på at genudleje det lejede. Hvad udlejeren indvinder eller burde have indvundet ved genudlejning i det i stk. 1 nævnte tidsrum, skal fragå i udlejerens krav over for lejeren."),
    ("§ 93, stk. 2", "Lejeren skal senest 8 dage før fraflytningen opgive den adresse, som meddelelser, herunder krav efter § 94, kan sendes til."),
    ("§ 94, stk. 1", "Ved fraflytning gennemføres et syn af boligen, hvorunder omfanget af lejerens forpligtelser efter § 25, stk. 4, og § 26 fastlægges. Synet foretages senest 2 uger efter, at udlejeren er blevet bekendt med, at fraflytning har fundet sted. Den fraflyttende lejer indkaldes skriftligt med mindst 1 uges varsel. Udlejeren og lejeren kan dog aftale et kortere varsel, når lejeforholdet er opsagt eller ophævet. Syn kan undlades, såfremt der ikke agtes fremsat krav imod lejeren om betaling af istandsættelsesudgifter."),
    ("§ 94, stk. 2", "Underretter udlejeren ikke senest 2 uger efter synet lejeren skriftligt om istandsættelsesarbejdernes omfang, den anslåede udgift og lejerens andel heraf, bortfalder udlejerens krav mod lejeren, medmindre denne er fraflyttet uden at give udlejeren oplysning om den fremtidige adresse. Overskridelser, der forhøjer lejerens samlede andel af den anslåede udgift med mere end 10 pct., er lejeren uvedkommende."),
    ("§ 96, stk. 1", "I kommuner med almene boliger nedsættes et eller flere beboerklagenævn til afgørelse af tvister efter denne lov."),
    ("§ 102, stk. 1", "Indbringelse af sager for beboerklagenævnet skal ske skriftligt. Den nødvendige dokumentation skal vedlægges. Ved indbringelse af sager skal betales et beløb på 100 kr. for hver sag. Beløbet er fastsat i 1998-niveau og reguleres én gang årligt [...]."),
    ("§ 105, stk. 1", "Beboerklagenævnet skal træffe afgørelse senest 4 uger fra det tidspunkt, hvor nævnet har modtaget svar efter § 102, stk. 2, eller efter § 103, stk. 4 [...]."),
    ("§ 106, stk. 1", "Beboerklagenævnets afgørelse kan af hver af parterne indbringes for boligretten. Indbringelse må ske senest 4 uger efter, at underretning om nævnets afgørelse er meddelt parterne."),
];

// ───────────────────────── hjælpere ─────────────────────────

fn dkk(oere: i64) -> String {
    let neg = oere < 0;
    let a = oere.abs();
    let kr = a / 100;
    let o = a % 100;
    let s = kr.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push('.');
        }
        out.push(c);
    }
    format!("{}{},{:02}", if neg { "\\textminus" } else { "" }, out, o)
}

fn sum(p: &[Post]) -> i64 {
    p.iter().map(|x| x.oere).sum()
}

fn main() {
    // ── regn efter ──
    let normal_brutto = sum(&NORMAL_ISTANDSAETTELSE);
    let fradrag = normal_brutto * FRADRAG_PCT / 100; // 5.735,00 · 66 % = 3.785,10
    let normal_lejer = normal_brutto - fradrag;
    let mislig = sum(&MISLIGHOLDELSE);
    let anden = sum(&ANDEN_REGNING);
    let mislig_anden = mislig + anden;
    let husleje_blok = SKYLDIG_HUSLEJE + HUSLEJERESTANCE_REST + AFREGNING_VARME + AFREGNING_VAND;
    let bundlinje = INDSKUD - normal_lejer - mislig_anden - husleje_blok;
    assert_eq!(fradrag, 378_510, "fradraget skal give FBF's 3.785,10");
    assert_eq!(normal_lejer, 194_990);
    assert_eq!(mislig, 644_725);
    assert_eq!(mislig_anden, 1_106_545);
    assert_eq!(husleje_blok, 2_922_434);
    assert_eq!(bundlinje, FBF_BUNDLINJE, "vores efterregning skal ramme FBF's bundlinje præcist");

    let huslejelinjer_sum: i64 = HUSLEJELINJER.iter().map(|l| l.2).sum();
    let uforklaret = huslejelinjer_sum - SKYLDIG_HUSLEJE; // linjerne siger 27.289,97; FBF siger 26.505,47
    let antal_paakrav = HUSLEJELINJER.iter().filter(|l| l.1 == "Påkravsgebyr").count() as i64;
    let paakrav_sum = antal_paakrav * PAAKRAVSGEBYR_OERE;

    // ── scenarier (hvad bundlinjen bliver, hvis en post falder) ──
    let rengoering = MISLIGHOLDELSE[2].oere;
    let rengoering_som_normal = rengoering - rengoering * FRADRAG_PCT / 100;
    let gulv = MISLIGHOLDELSE[3].oere;
    let scen: Vec<(&str, i64)> = vec![
        ("Som FBF har opgjort den", bundlinje),
        ("Advokatomkostninger bortfalder (§ 92, stk. 1 kræver gyldig ophævelse; udsættelsen blev tilbagekaldt)", bundlinje + anden),
        ("Gulvpolish bortfalder (gulvbehandling er vedligeholdelse i boperioden, § 26, stk. 1 — ikke normalistandsættelse; kun misligholdelse efter § 25, stk. 4 kan kræves)", bundlinje + gulv),
        ("Rengøring flyttes til normalistandsættelse med 66 pct. fradrag (§ 26, stk. 2 nævner rengøring udtrykkeligt)", bundlinje + (rengoering - rengoering_som_normal)),
        ("Hele misligholdelsen + advokat bortfalder (intet syn/ingen rapport dokumenterer misligholdelse)", bundlinje + mislig_anden),
        ("Alt, lejer ikke selv har valgt, bortfalder: normalistandsættelse + misligholdelse + advokat (§ 94, stk. 2 bortfald — lejer oplyser: ikke indkaldt til syn)", bundlinje + normal_lejer + mislig_anden),
        ("Som ovenfor, og afdelingen viser sig at være B-ordning (§ 27): samme resultat, men vedligeholdelseskontoens saldo skal desuden oplyses", bundlinje + normal_lejer + mislig_anden),
    ];

    // ── LaTeX ──
    let preamble = r##"
\definecolor{ink}{HTML}{0E1116}
\definecolor{slate}{HTML}{2B3340}
\definecolor{navy}{HTML}{1F3A5F}
\definecolor{redx}{HTML}{B3261E}
\definecolor{greenx}{HTML}{1B7F4B}
\definecolor{amber}{HTML}{B7791F}
\definecolor{paper}{HTML}{FFFFFF}
\definecolor{soft}{HTML}{F1F4F8}
\pagecolor{paper}\color{ink}
\hypersetup{colorlinks=true,urlcolor=navy,linkcolor=navy}
\titleformat{\section}{\Large\bfseries\color{navy}}{\thesection}{8pt}{}
\titleformat{\subsection}{\large\bfseries\color{slate}}{\thesubsection}{6pt}{}
\titlespacing{\section}{0pt}{14pt}{6pt}
\pagestyle{fancy}\fancyhf{}
\renewcommand{\headrulewidth}{0pt}
\fancyfoot[L]{\footnotesize\color{slate}Notat: flytteafregning Sæbygårdvej 38, 8 \textperiodcentered{} lejemål 1 35 3255 6 \textperiodcentered{} 21. september 2026}
\fancyfoot[R]{\footnotesize\color{slate}\thepage}
\newtcolorbox{svar}[1]{colback=soft,colframe=navy,boxrule=1pt,arc=3pt,left=9pt,right=9pt,top=7pt,bottom=7pt,title={#1},fonttitle=\bfseries,coltitle=white,colbacktitle=navy}
\newtcolorbox{lov}[1]{colback=white,colframe=slate,boxrule=0.6pt,arc=2pt,left=8pt,right=8pt,top=5pt,bottom=5pt,title={#1},fonttitle=\bfseries\small,coltitle=white,colbacktitle=slate}
\setlist[itemize]{leftmargin=16pt,itemsep=2pt,topsep=3pt}
\setlist[enumerate]{leftmargin=18pt,itemsep=2pt,topsep=3pt}
\newcommand{\ok}{\textcolor{greenx}{\textbf{sikkert}}}
\newcommand{\pl}{\textcolor{amber}{\textbf{plausibelt}}}
\newcommand{\fa}{\textcolor{redx}{\textbf{kræver fakta}}}
"##;

    let mut b = String::new();
    // forside
    b.push_str(r##"
\thispagestyle{empty}
\noindent{\color{navy}\rule{\linewidth}{3pt}}\\[2pt]
{\footnotesize\color{slate}JURIDISK NOTAT \quad\textbullet\quad 21. september 2026 \quad\textbullet\quad genereret af flux-arxiv-latex (\texttt{fbf\_flytteafregning})}

\vspace{22mm}
\begin{center}
{\fontsize{24}{30}\selectfont\bfseries\color{ink}Flytteafregningen fra\\Frederikshavn Boligforening}\\[10pt]
{\Large\color{slate}Sæbygårdvej 38, 8, 9300 Sæby — afd. 35 Mariested (ældreboliger)\\lejemål 1 35 3255 6}\\[18pt]
{\large\color{ink}Hvad regningen består af, hvad almenlejeloven siger om hver post,\\og hvad der skal ske nu}
\end{center}
\vspace{16mm}
\begin{center}
\begin{tcolorbox}[width=0.78\linewidth,colback=soft,colframe=navy,boxrule=1pt,arc=3pt,halign=center]
{\footnotesize\color{slate}LEJER}\\[2pt]{\large\bfseries Viktor Sandstrøm Kristensen}\\[8pt]
{\footnotesize\color{slate}UDLEJER}\\[2pt]{\large\bfseries Frederikshavn Boligforening (CVR 44839016)}\\[8pt]
{\footnotesize\color{slate}KILDER}\\[2pt]{\small Flytteafregning 15-06-2026 (12 sider inkl. 5 fakturaer) \textperiodcentered{} Rykker 1 af 09-07-2026 \textperiodcentered{} Almenlejeloven (hentet 21-09-2026)}
\end{tcolorbox}
\end{center}
\vfill
{\small\color{slate}Alle beløb i notatet er regnet efter i programmet (øre-heltal) og rammer boligforeningens egen bundlinje på øren. Lovtekst er gengivet ordret. Vurderinger er mærket \ok{} / \pl{} / \fa{} — det sidste betyder, at svaret afhænger af oplysninger, kun lejeren har.}
\newpage
"##);

    // 1 Sammenfatning
    b.push_str(&format!(r##"
\section{{Sammenfatning}}
\begin{{svar}}{{Det korte svar}}
Boligforeningen siger, du skylder \textbf{{kr. {bund}}}. Det tal er summen af \emph{{tre helt forskellige ting}}, og de skal skilles ad, før man kan svare:
\begin{{enumerate}}
\item \textbf{{Husleje m.v. januar–maj 2026: kr. {hus}}} (efter modregning af varme/vand). Det er den tunge post, og det er den, der er sværest at anfægte: skyldig leje er skyldig leje. Men den kan \emph{{kontrolleres}} — huslejelinjerne i afregningen summerer til kr. {hl}, ikke kr. {sh}; de kr. {ufo}, der mangler, må være betalinger eller krediteringer, som ikke er vist. Kræv et fuldt kontoudtog.
\item \textbf{{Normalistandsættelse: kr. {nl}}} (maling kr. {nb} med 66 pct. fradrag). Fradraget er præcis den mekanisme, du tænker på: efter § 26, stk. 2 overtager boligforeningen 1 pct. af malerregningen for hver måned, du har boet der, og efter 66 måneder er 66 pct. deres. \emph{{Hvis}} afdelingen i stedet kører B-ordning (§ 27, vedligeholdelseskonto), betaler du slet ikke normalistandsættelse. Det står i vedligeholdelsesreglementet og lejekontrakten — kræv dem.
\item \textbf{{Misligholdelse + advokat: kr. {ma}}}. Det er de poster, du ikke har valgt, ikke er blevet spurgt om, og som loven stiller strenge formkrav til: syn med skriftlig indkaldelse (§ 94, stk. 1), skriftlig underretning inden 2 uger om omfang, anslået udgift og din andel (§ 94, stk. 2) — ellers \emph{{bortfalder}} kravet. Gulvpolish og rengøring er desuden kategoriseret forkert, og advokatregningen forudsætter en gyldig ophævelse efter § 90, stk. 2.
\end{{enumerate}}
\textbf{{Fortegnet på regningen afgøres af punkt 2 og 3.}} Står huslejen ved magt, men falder alt det, du ikke har valgt (kr. {alt}), skylder \emph{{de dig}} kr. {plus}. Falder kun misligholdelse og advokat, skylder de dig kr. {plus2}. Falder kun advokatregningen, skylder du stadig kr. {min_adv}.
\end{{svar}}

\begin{{svar}}{{Oplyst af lejeren, 21. september 2026}}
\begin{{itemize}}
\item \textbf{{Nøglerne blev lagt på køkkenbordet}} i lejligheden ved fraflytningen 06-05-2026. Boligforeningens eget flyttefirma var inde i lejligheden 07-05-2026 og havde dermed både adgang og nøglerne fra den dag.
\item \textbf{{Ingen indkaldelse til fraflytningssyn er modtaget}} — hverken skriftligt med en uges varsel (§ 94, stk. 1) eller på anden måde. Lejeren har ikke set en fraflytningsrapport.
\end{{itemize}}
Det flytter to poster fra "kræver fakta" til "plausibelt/sikkert" nedenfor: låsesmeden og hele § 94-spørgsmålet.
\end{{svar}}

\noindent Der er to ting, du kan gøre med det samme, og de koster tilsammen under 200 kr.: (a) et skriftligt krav om dokumentation og indsigelse til boligforeningen (bilag A), og (b) en klage til Frederikshavn Kommune — både til \textbf{{beboerklagenævnet}} (§ 96; gebyr 100 kr. i 1998-niveau, dvs. ca. 170 kr. i dag) og til kommunen som \textbf{{tilsynsmyndighed}} for almene boligorganisationer (almenboligloven kap. 12). Kommunen fik desuden efter § 92, stk. 2 besked om udsættelsessagen, mens den løb — spørg dem, hvad de gjorde med den besked.
"##,
        bund = dkk(-bundlinje), hus = dkk(husleje_blok), hl = dkk(huslejelinjer_sum), sh = dkk(SKYLDIG_HUSLEJE), ufo = dkk(uforklaret),
        nl = dkk(normal_lejer), nb = dkk(normal_brutto), ma = dkk(mislig_anden), alt = dkk(normal_lejer + mislig_anden),
        plus = dkk(bundlinje + normal_lejer + mislig_anden), plus2 = dkk(bundlinje + mislig_anden), min_adv = dkk(-(bundlinje + anden)),
    ));

    // 2 Regningen
    b.push_str(r##"
\section{Regningen, som boligforeningen har opgjort den}
Tallene nedenfor er tastet fra flytteafregningen af 15. juni 2026 (side 2) og de fem vedlagte fakturaer, og programmet lægger dem sammen igen. Efterregningen rammer boligforeningens bundlinje præcist.

\begin{longtable}{@{}p{0.62\linewidth}rr@{}}
\toprule
\textbf{Post} & \textbf{kr.} & \textbf{sum} \\
\midrule
\endhead
"##);
    b.push_str(&format!("Indskud & & \\textcolor{{greenx}}{{{}}} \\\\\n\\midrule\n", dkk(INDSKUD)));
    for p in NORMAL_ISTANDSAETTELSE.iter() {
        b.push_str(&format!("{} — {} ({}) & {} & \\\\\n", latex_escape(p.label), latex_escape(p.leverandoer), p.dato, dkk(p.oere)));
    }
    b.push_str(&format!("Normal istandsættelse i alt & {} & \\\\\n", dkk(normal_brutto)));
    b.push_str(&format!("Fradrag for istandsættelse 66 pct. (§ 26, stk. 2) & {} & \\\\\n", dkk(-fradrag)));
    b.push_str(&format!("\\quad heraf lejerens andel & & \\textcolor{{redx}}{{{}}} \\\\\n\\midrule\n", dkk(-normal_lejer)));
    for p in MISLIGHOLDELSE.iter() {
        b.push_str(&format!("Misligholdelse: {} — {} ({}) & {} & \\\\\n", latex_escape(p.label), latex_escape(p.leverandoer), p.dato, dkk(p.oere)));
    }
    for p in ANDEN_REGNING.iter() {
        b.push_str(&format!("Anden regning: {} — {} ({}) & {} & \\\\\n", latex_escape(p.label), latex_escape(p.leverandoer), p.dato, dkk(p.oere)));
    }
    b.push_str(&format!("Misligholdelse + anden regning i alt & & \\textcolor{{redx}}{{{}}} \\\\\n\\midrule\n", dkk(-mislig_anden)));
    b.push_str(&format!("Skyldig husleje m.v. 01-01–01-05-2026 (FBF's tal) & {} & \\\\\n", dkk(SKYLDIG_HUSLEJE)));
    b.push_str(&format!("Huslejerestance – rest (linje 01-05-2026) & {} & \\\\\n", dkk(HUSLEJERESTANCE_REST)));
    b.push_str(&format!("Afregning varme & {} & \\\\\n", dkk(AFREGNING_VARME)));
    b.push_str(&format!("Afregning vand & {} & \\\\\n", dkk(AFREGNING_VAND)));
    b.push_str(&format!("Husleje m.v. i alt & & \\textcolor{{redx}}{{{}}} \\\\\n\\midrule\n", dkk(-husleje_blok)));
    b.push_str(&format!("\\textbf{{Boligforeningens tilgodehavende (bundlinje)}} & & \\textbf{{{}}} \\\\\n", dkk(bundlinje)));
    b.push_str("\\bottomrule\n\\end{longtable}\n");

    b.push_str(&format!(r##"
\subsection{{Tre ting, der springer i øjnene i selve regnestykket}}
\begin{{itemize}}
\item \textbf{{Huslejelinjerne stemmer ikke med totalen.}} Linjerne på side 2–3 (uden den særskilte restancelinje på kr. {rest}) summerer til \textbf{{kr. {hl}}}; afregningen skriver \textbf{{kr. {sh}}}. Differencen på \textbf{{kr. {ufo}}} er ikke vist — det må være betalinger eller krediteringer. Det er ikke i sig selv en fejl, men det beviser, at afregningen ikke er en fuld kontooversigt. Kræv den.
\item \textbf{{Fem påkravsgebyrer på kr. {pg} = kr. {pgs}.}} Efter § 90, stk. 2 kan et gebyr kun kræves for et \emph{{skriftligt}} påkrav, afgivet tidligst 3. hverdag efter sidste rettidige betalingsdag, som udtrykkeligt nævner ophævelse. Ét gebyr pr. måned i fem måneder betyder fem sådanne breve. Kræv kopi af alle fem. (Beløbet er 250 kr. i 2009-niveau, indeksreguleret; om 329 kr. er 2026-satsen, skal kontrolleres.)
\item \textbf{{Rengøring på kr. {ren}}} står under \emph{{misligholdelse}} (100 pct. til lejer) — men § 26, stk. 2 nævner rengøring udtrykkeligt som en del af \emph{{normalistandsættelsen}}, hvor du kun betaler 34 pct. Der er heller ingen faktura for den. Som normalistandsættelse ville din andel være kr. {ren34}.
\end{{itemize}}
"##, rest = dkk(HUSLEJERESTANCE_REST), hl = dkk(huslejelinjer_sum), sh = dkk(SKYLDIG_HUSLEJE), ufo = dkk(uforklaret),
        pg = dkk(PAAKRAVSGEBYR_OERE), pgs = dkk(paakrav_sum), ren = dkk(rengoering), ren34 = dkk(rengoering_som_normal)));

    // 3 Loven
    b.push_str(r##"
\section{Hvad loven siger — ordret}
Frederikshavn Boligforening er en almen boligorganisation, og afd. 35 er almene ældreboliger. Derfor gælder \textbf{almenlejeloven} (lov om leje af almene boliger, LBK nr. 928 af 4. september 2019 med senere ændringer), \emph{ikke} den private lejelov. Teksten nedenfor er hentet ordret den 21. september 2026 (danskelove.dk, konsolideret tekst; officiel kilde retsinformation.dk). Klammer [...] markerer udeladte sætninger uden betydning for sagen.
"##);
    for (par, txt) in LOV.iter() {
        b.push_str(&format!("\\begin{{lov}}{{Almenlejeloven {}}}\n\\small {}\n\\end{{lov}}\n", latex_escape(par), latex_escape(txt)));
    }

    // 4 Post for post
    b.push_str(&format!(r##"
\section{{Post for post: hvad de kan kræve, og hvad de skal bevise}}
Mærkning: \ok{{}} = følger direkte af lovteksten og tallene; \pl{{}} = stærkt argument, men afhænger af en vurdering (beboerklagenævn/boligret); \fa{{}} = svaret afhænger af oplysninger, kun du har (se afsnit 6).

\subsection{{Formkravene — nøglen til hele istandsættelsesblokken}}
§ 94 er bygget som en \emph{{fælde for udlejeren}}, ikke for lejeren: der skal holdes syn senest 2 uger efter, at udlejer vidste, du var flyttet (fraflytningsdato 06-05-2026 $\Rightarrow$ senest 20-05-2026); du skal indkaldes \emph{{skriftligt med mindst 1 uges varsel}}; og senest 2 uger efter synet skal du have skriftlig besked om \emph{{arbejdernes omfang, den anslåede udgift og din andel}}. Mangler ét af leddene, \textbf{{bortfalder kravet}} (§ 94, stk. 2) — hele normalistandsættelsen og hele misligholdelsen, på én gang. Afregningen henviser til et "tidligere fremsendt budget over forventet istandsættelsesudgift"; om det budget findes og hvornår det kom, er \fa{{}}; \textbf{{at du ikke blev indkaldt til syn, er oplyst af dig}} — kan boligforeningen ikke fremlægge en skriftlig indkaldelse og en fraflytningsrapport, er der ikke holdt et gyldigt syn, og så er der intet at underrette om inden for § 94, stk. 2's frist: kravet bortfalder (\pl{{}}, fordi nævnet skal lægge din oplysning til grund over for deres manglende papir). Der er én undtagelse, som boligforeningen vil gribe efter: bortfaldet gælder ikke, hvis du flyttede \emph{{uden at oplyse din nye adresse}} (§ 93, stk. 2). De havde din e-mail (afregningen er sendt til den), så argumentet er svagt, men det skal imødegås: skriv, hvornår og hvordan du gav adresse/kontakt.

\subsection{{De enkelte poster}}
\begin{{longtable}}{{@{{}}p{{0.30\linewidth}}rp{{0.52\linewidth}}@{{}}}}
\toprule
\textbf{{Post}} & \textbf{{kr.}} & \textbf{{Vurdering}} \\
\midrule
\endhead
Normalistandsættelse, maling (66 pct. fradrag) & {nl} & \ok{{}} Fradraget er § 26, stk. 2 — de 66 måneder er allerede krediteret. \pl{{}} Falder helt, hvis afdelingen er B-ordning (§ 27) — så findes en vedligeholdelseskonto, og dens saldo skal oplyses. \pl{{}} Falder helt ved formmangel (§ 94, stk. 2). Kræv: vedligeholdelsesreglement, lejekontrakt, indflytningsrapport (§ 25, stk. 3: ikke bedre stand end ved overtagelsen). \\
Rengøring (ført som misligholdelse) & {ren} & \ok{{}} Rengøring er normalistandsættelse efter § 26, stk. 2 og skal have 66 pct. fradrag (din andel højst kr. {ren34}), medmindre en fraflytningsrapport dokumenterer misligholdelse efter § 25, stk. 4. Ingen faktura vedlagt. \\
Gulvpolish 2X, 28,54 m² & {gulv} & \ok{{}} Gulvbehandling er vedligeholdelse \emph{{i boperioden}} (§ 26, stk. 1) og indgår \emph{{ikke}} i normalistandsættelsen ved fraflytning (§ 26, stk. 2 nævner kun vægge, lofter og rengøring). Kan kun kræves som misligholdelse (§ 25, stk. 4), dvs. skade ved fejlagtig brug/uforsvarlig adfærd — det skal stå i fraflytningsrapporten med begrundelse. En polishbehandling er normal klargøring til næste lejer. \\
Låsesmed: opluk + ny cylinder (12-05-2026) & {laas} & \pl{{}} Nøglerne lå på køkkenbordet fra 06-05, og FBF's eget flyttefirma var inde 07-05 — fra den dag havde de adgang og nøgler. Et "udkald til opluk" 12-05, fem dage \emph{{efter}}, og et skift "til HV cylinder" (hovednøgle-cylinder, dvs. boligforeningens eget låsesystem) er udlejers egen drift, ikke skade forvoldt af lejer (§ 25, stk. 4). Fakturaen er rekvireret af FBF (Jens Rusgaard) til den udsættelse, advokaten selv skriver blev \emph{{tilbagekaldt}}. Svaghed: nøgler lagt i lejligheden er ikke en kvitteret aflevering — skriv præcis hvor og hvornår. \\
Flyttefirma: tømning til genbrugspladsen (07-05-2026) & {flyt} & \fa{{}} Kan kun kræves, hvis du efterlod ting, og lejligheden skulle ryddes. Men: dine ejendele blev kørt på genbrugspladsen dagen efter fraflytningsdatoen, \emph{{uden fogedforretning}} (den blev tilbagekaldt). Hvad var der, og fik du besked? Det kan være et \emph{{modkrav}}, ikke en regning. \\
Advokat (Advodan): fogedgebyr, salær m.m. & {adv} & \pl{{}} Hjemlen er § 92, stk. 1 ("omkostningerne ved lejerens udsættelse") — men den forudsætter en \emph{{gyldig ophævelse}}, dvs. et påkrav efter § 90, stk. 2 med 14 dages frist og udtrykkelig ophævelsestrussel, og derefter en ophævelsesskrivelse. Udsættelsen blev tilbagekaldt før den begyndte. Kræv: påkrav, ophævelse, fogedrettens sagsnr., tilbagekaldelsen, og advokatens specifikation. Salæret (3.000 + moms) er advokatens regning til FBF; om hele beløbet er dit "tab" at bære, er en rimelighedsvurdering. \\
Påkravsgebyr $\times$ {npk} & {pgs} & \pl{{}} Ét pr. gyldigt skriftligt påkrav (§ 90, stk. 2). Kræv alle fem breve. \\
Husleje m.v. jan–maj (netto) & {hus} & \ok{{}} Leje for tiden frem til frigørelsesdatoen 01-06-2026 er som udgangspunkt skyldig (§ 92, stk. 1). \pl{{}} Men: (a) kr. {ufo} er ikke forklaret — kræv kontoudtog; (b) § 92, stk. 3: blev lejligheden genudlejet før 1. juni? Så skal lejen fragå; (c) var du tvangsindlagt i (dele af) perioden, er det relevant for kommunen (afsnit 5), ikke for om lejen skyldes. \\
Varme/vand-afregninger & {vv} & \ok{{}} I din favør; kontrollér mod forbrugsregnskabet. \\
\bottomrule
\end{{longtable}}
"##, nl = dkk(normal_lejer), ren = dkk(rengoering), ren34 = dkk(rengoering_som_normal), gulv = dkk(gulv),
        laas = dkk(MISLIGHOLDELSE[0].oere), flyt = dkk(MISLIGHOLDELSE[1].oere), adv = dkk(anden), npk = antal_paakrav, pgs = dkk(paakrav_sum),
        hus = dkk(husleje_blok), ufo = dkk(uforklaret), vv = dkk(-(AFREGNING_VARME + AFREGNING_VAND))));

    // 5 Scenarier
    b.push_str(r##"
\section{Scenarier: hvad bundlinjen bliver}
Positivt tal = boligforeningen skylder \emph{dig}; negativt = du skylder dem. Huslejen står ved magt i alle scenarier — det er de øvrige poster, der flytter fortegnet.

\begin{longtable}{@{}p{0.74\linewidth}r@{}}
\toprule
\textbf{Scenarie} & \textbf{bundlinje, kr.} \\
\midrule
\endhead
"##);
    for (label, v) in scen.iter() {
        let col = if *v >= 0 { "greenx" } else { "redx" };
        b.push_str(&format!("{} & \\textcolor{{{}}}{{\\textbf{{{}}}}} \\\\\n", latex_escape(label), col, dkk(*v)));
    }
    b.push_str("\\bottomrule\n\\end{longtable}\n");

    // 6 Frister, procedure, kommunen
    b.push_str(r##"
\section{Frister, procedure — og kommunen}
\subsection{Hvor sagen står i dag (21. september 2026)}
\begin{itemize}
\item Afregning sendt 15-06-2026 med boligforeningens \emph{egen} indsigelsesfrist på 14 dage (29-06-2026). Den frist er ikke en lovbestemt afskæring af dine indsigelser — den binder boligforeningens \emph{egen} sagsbehandling, ikke beboerklagenævnet.
\item Rykker 1 af 09-07-2026: 10 dage, derefter "inkasso med yderligere omkostninger". Har du modtaget noget fra et inkassobureau siden 19-07-2026, er det \fa{} — inkassolovens § 10 kræver et skriftligt påkrav med mindst 10 dages frist, før der kan pålægges inkassoomkostninger, og renteloven sætter loft over dem.
\item Intet i sagen er forældet; fordringer af denne art forældes efter 3 år.
\end{itemize}

\subsection{Beboerklagenævnet i Frederikshavn Kommune}
Tvister om istandsættelse ved fraflytning afgøres af beboerklagenævnet (§ 96, jf. § 100). Sagen indbringes skriftligt med dokumentation og et gebyr på 100 kr. i 1998-niveau (indeksreguleret, ca. 170 kr.; kommunen oplyser den præcise sats) — § 102. Nævnet skal vejlede parterne (§ 104, stk. 5), kan besigtige (§ 104) og skal afgøre sagen inden 4 uger efter modpartens svar (§ 105). Svarer boligforeningen ikke, kan nævnet lægge din fremstilling til grund (§ 105, stk. 2). Afgørelsen kan indbringes for boligretten inden 4 uger (§ 106). \textbf{Det er det billigste og hurtigste sted at få de 13.015,35 kr. prøvet.} Nævnet tager stilling til istandsættelse og misligholdelse; selve huslejerestancen og advokatomkostningerne er et pengekrav, som i sidste ende hører under fogedret/boligret — men nævnets afgørelse om resten flytter fortegnet på hele afregningen.

\subsection{Kommunen som tilsyn — og som den, der fik besked}
To selvstændige spor hos Frederikshavn Kommune:
\begin{enumerate}
\item \textbf{Tilsyn.} Kommunalbestyrelsen fører tilsyn med almene boligorganisationer (almenboligloven kap. 12). En henvendelse til tilsynet om, at afd. 35 opkræver gulvbehandling og rengøring som misligholdelse uden dokumenteret syn, er en \emph{principiel} klage — den rammer alle fraflyttere i afdelingen, ikke kun dig.
\item \textbf{§ 92, stk. 2-underretningen.} Da boligforeningen sendte udsættelsesbegæring til fogedretten, \emph{skulle} den samtidig skriftligt underrette kommunen med dit navn og adresse. Advokatens faktura bekræfter, at det skete ("orientering til kommunen"). Kommunen kan yde hjælp til huslejerestance for udsættelsestruede (lov om aktiv socialpolitik § 81 a) og skal vurdere behovet, når den får sådan en besked — og du var i marts 2026 tvangsindlagt (sag ved Retten i Hjørring). Spørg kommunen skriftligt: \emph{Hvornår modtog I underretningen, hvem behandlede den, og hvad besluttede I?} Svaret er enten en hjælp, I burde have givet, eller en sagsbehandlingsfejl — begge dele hører til i sagen om, hvem der skal bære omkostningerne.
\end{enumerate}
Aktindsigt i begge spor er gratis (forvaltningsloven § 9 / offentlighedsloven § 7).

\section{Spørgsmål, kun du kan svare på}
Notatets \fa{}-poster afgøres af disse ti svar. Skriv dem ned, før brevene sendes:
\begin{enumerate}
\item \textcolor{greenx}{\textbf{Besvaret:}} nøglerne lå på køkkenbordet ved fraflytningen 06-05-2026. (Skriv gerne klokkeslæt, og om andre så det.)
\item \textcolor{greenx}{\textbf{Besvaret:}} ingen indkaldelse til syn modtaget, intet syn med lejer, ingen fraflytningsrapport set.
\item Har du modtaget en fraflytningsrapport? Har du modtaget "budget over forventet istandsættelsesudgift" — med dato?
\item Gav du boligforeningen din nye adresse (eller e-mail/telefon) senest 8 dage før fraflytning — hvordan?
\item Hvad stod der i lejligheden den 06-05-2026? Fik du besked, før det blev kørt på genbrugspladsen?
\item Står der A-ordning eller B-ordning i din lejekontrakt? Har du fået vedligeholdelsesreglementet? (Overtagelse ca. november 2020, da 66 måneder er krediteret.)
\item Hvilke betalinger gjorde du selv i januar–maj 2026? (Boligstøtten på ca. 3.297 kr./md. gik direkte til FBF.)
\item Hvornår modtog du påkrav og ophævelse? Var du indlagt, da de kom frem?
\item Har du modtaget noget fra et inkassobureau eller fogedretten efter 19-07-2026?
\item Blev lejligheden genudlejet før 1. juni 2026 (nabo, opslag, ny beboer)?
\end{enumerate}

\section{Det, notatet ikke påstår}
\begin{itemize}
\item Notatet er ikke juridisk rådgivning fra en advokat, og beboerklagenævnet kan vægte fakta anderledes. Retshjælp: Frederikshavn Kommunes borgerrådgiver, Lejernes LO (medlemskab), og fri proces/retshjælpsforsikring ved boligretssag.
\item Vedligeholdelsesbekendtgørelsen for almene boliger (BEK nr. 640 af 15. juni 2006) præciserer § 94 (rapportens indhold, 14-dages-frister). Dens ordlyd kunne ikke hentes maskinelt den 21-09-2026 og er derfor \emph{ikke} citeret; § 94 i selve loven bærer argumentet alene.
\item Satserne for påkravsgebyr (2026) og nævnsgebyr (2026) er indeksregulerede og ikke verificeret her; de ændrer ikke konklusionerne.
\item Om huslejen for januar–maj i sidste ende skal bæres af dig, af kommunen (§ 81 a) eller nedsættes for genudlejning, afgøres af fakta i afsnit 6 — notatet lægger \emph{konservativt} til grund, at hele beløbet står.
\end{itemize}
"##);

    // Bilag A: brev
    b.push_str(r##"
\newpage
\section*{Bilag A — Indsigelse og krav om dokumentation (udkast til brev)}
\addcontentsline{toc}{section}{Bilag A — Indsigelse og krav om dokumentation}
\begin{lov}{Til Frederikshavn Boligforening, Økonomiafdelingen — vedr. flytteafregning af 15-06-2026, lejemål 1 35 3255 6}
\small
Jeg gør hermed indsigelse mod flytteafregningen af 15. juni 2026 for Sæbygårdvej 38, 8, 9300 Sæby, og mod rykkeren af 9. juli 2026, og anmoder om, at inkasso stilles i bero, indtil nedenstående er besvaret.

\textbf{1. Dokumentation, jeg beder om inden 14 dage:}
(a) kopi af skriftlig indkaldelse til fraflytningssyn og af fraflytningsrapporten (almenlejeloven § 94, stk. 1);
(b) kopi af den skriftlige underretning om istandsættelsesarbejdernes omfang, anslået udgift og min andel, med afsendelsesdato (§ 94, stk. 2);
(c) afdelingens vedligeholdelsesreglement og min lejekontrakt, herunder om afdelingen er A- eller B-ordning (§§ 25–27), samt indflytningsrapporten;
(d) fuldt kontoudtog for lejemålet 01-01-2026 til dato med alle betalinger og krediteringer — afregningens huslejelinjer summerer til 27.289,97 kr., ikke de anførte 26.505,47 kr.;
(e) kopi af samtlige påkrav (5 stk. à 329 kr.), ophævelsesskrivelsen, udsættelsesbegæringen til fogedretten med sagsnummer, tilbagekaldelsen og underretningen til Frederikshavn Kommune (§ 92, stk. 2);
(f) faktura og begrundelse for "Rengøring" (960 kr.), og oplysning om, hvornår lejligheden blev genudlejet (§ 92, stk. 3);
(g) dokumentation for, hvad der befandt sig i lejligheden den 07-05-2026, og hvilket varsel jeg fik, før mine ejendele blev kørt på genbrugspladsen.

\textbf{Faktiske forhold:} Jeg fraflyttede 06-05-2026 og lagde nøglerne på køkkenbordet. Boligforeningens flyttefirma var i lejligheden 07-05-2026 og havde dermed adgang og nøgler. Jeg er ikke blevet indkaldt til fraflytningssyn og har ikke modtaget nogen fraflytningsrapport.

\textbf{2. Indsigelser:}
Rengøring er normalistandsættelse (§ 26, stk. 2) og kan højst kræves med 34 pct. Gulvbehandling er vedligeholdelse i boperioden (§ 26, stk. 1) og indgår ikke i normalistandsættelsen; den kan kun kræves som dokumenteret misligholdelse (§ 25, stk. 4). Låsesmedens "opluk" 12-05-2026 og skift til HV-cylinder skete fem dage efter, at boligforeningen selv var inde i lejligheden, og vedrører boligforeningens eget låsesystem. Låsesmed og flyttefirma hører til en udsættelse, der blev tilbagekaldt; jeg har ikke bestilt, godkendt eller fået forelagt nogen af arbejderne eller beløbene. Advokatomkostninger forudsætter gyldig ophævelse efter § 90, stk. 2. Kan dokumentationen under pkt. 1 (a)–(b) ikke fremlægges, bortfalder istandsættelseskravet efter § 94, stk. 2, og afregningen skal opgøres på ny.

Besvares henvendelsen ikke fyldestgørende, indbringer jeg sagen for beboerklagenævnet i Frederikshavn Kommune og orienterer kommunens tilsyn med almene boligorganisationer.

Med venlig hilsen\\ Viktor Sandstrøm Kristensen
\end{lov}
"##);

    let doc = Document::new("article")
        .option("11pt")
        .option("a4paper")
        .package_opt("fontenc", &["T1"])
        .package("lmodern")
        .package_opt("geometry", &["a4paper", "top=22mm", "bottom=22mm", "left=20mm", "right=20mm"])
        .package("xcolor")
        .package("titlesec")
        .package("enumitem")
        .package_opt("tcolorbox", &["most"])
        .package("booktabs")
        .package("longtable")
        .package("fancyhdr")
        .package("hyperref")
        .preamble(preamble)
        .add(Block::Raw(b));

    let out_dir = std::env::var("FBF_OUT_DIR").unwrap_or_else(|_| "/home/storage/sigil-scratch/retssag-fbf".to_string());
    let res = doc.compile_pdf(&out_dir, "fbf-flytteafregning-notat");
    println!(
        "flux-arxiv-latex: success={} pdf={:?} · bundlinje={} · alt-ikke-valgt={} · plus-scenarie={} · uforklaret-husleje={}",
        res.success, res.pdf_path, dkk(bundlinje), dkk(normal_lejer + mislig_anden), dkk(bundlinje + normal_lejer + mislig_anden), dkk(uforklaret)
    );
    if !res.success {
        let tail = &res.log[res.log.len().saturating_sub(2500)..];
        eprintln!("--- compile log tail ---\n{}", tail);
        std::process::exit(1);
    }
}
