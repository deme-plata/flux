//! idle_machine — "The Idle Machine": what the world spends rebuilding what it
//! already built, and what a top-three compiler would recover.
//!
//! Third paper in the computed-at-generation-time series (after
//! `thermodynamic_ledger` and `legibility_dividend`). The kernel is MEASURED (a
//! build-system pathology and its repair); the planetary figure is an explicit
//! four-rung extrapolation whose every multiplier is declared, printed, and
//! graded. The paper deliberately deflates its own climate headline — the carbon
//! number is negligible against ICT's footprint — and lands instead on the two
//! dividends that survive the Jevons rebound: human time and correctness. It
//! also argues that the largest planetary lever available is a *default*, not a
//! technology, which is an argument against the authors' own interest.
//!
//! Usage: idle_machine [arxiv.json] [out_dir]
use flux_arxiv_latex::doc::{Block, Document};
use flux_arxiv_latex::{bibliography, latex_escape, parse_arxiv_json, ArxivPaper};
use flux_science::constants::BOLTZMANN;

// ─────────────────────────────────────────────────────────── formatting helpers

fn thou(x: f64) -> String {
    let n = x.round().abs() as u128;
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push_str("{,}");
        }
        out.push(c);
    }
    if x < 0.0 {
        format!("-{out}")
    } else {
        out
    }
}

fn usd(x: f64) -> String {
    format!("\\${}", thou(x))
}

/// Money at billions scale, e.g. `\$50.3\,bn`.
fn usd_bn(x: f64) -> String {
    format!("\\${:.1}\\,bn", x / 1e9)
}

fn sci(x: f64) -> String {
    if x == 0.0 || !x.is_finite() {
        return format!("{x}");
    }
    let exp = x.abs().log10().floor() as i32;
    if (-2..=4).contains(&exp) {
        let s = format!("{:.3}", x);
        let s = s.trim_end_matches('0').trim_end_matches('.');
        s.to_string()
    } else {
        let mant = x / 10f64.powi(exp);
        format!("{:.2}\\times10^{{{}}}", mant, exp)
    }
}

fn raw(s: &str) -> Block {
    Block::Raw(format!("{s}\n\n"))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let json_path = args
        .get(1)
        .map(String::as_str)
        .unwrap_or("crates/flux-arxiv-latex/idle_machine.arxiv.json");
    let out_dir = args.get(2).map(String::as_str).unwrap_or("/tmp/idle-machine");

    let papers: Vec<ArxivPaper> = std::fs::read_to_string(json_path)
        .ok()
        .and_then(|j| parse_arxiv_json(&j).ok())
        .unwrap_or_default();

    // ══════════════════════════ RUNG 0 — THE MEASURED KERNEL (one workspace)
    let t_noop_before = 215.9_f64; // s, confirming an unchanged crate is unchanged
    let t_noop_after = 1.0_f64; // s, after the repair (heal run 0.56 s)
    let t_fat_before = 359.0_f64; // s, heaviest binary, 166 units re-declared dirty
    let poisoned_roots = 19.0_f64;
    let poisoned_downstream = 147.0_f64;
    let cache_unit_hit = 0.36_f64;
    let measured_box_watts = 48.0 * 15.0; // the box the measurement was taken on
    let dt = t_noop_before - t_noop_after;
    let speedup = t_noop_before / t_noop_after;
    let e_waste_measured_j = dt * measured_box_watts; // J per redundant build, our box

    // ══════════════════════════ DECLARED ASSUMPTIONS (every multiplier, named)
    // Deliberately conservative: we discount our own instrument by using
    // laptop-class power for the local loop rather than the 720 W box we measured on.
    let dev_watts = 65.0_f64;
    let ci_watts = 240.0_f64;
    let pue = 1.5_f64; // data-centre overhead, applied to CI only
    let f_noop = 0.40_f64; // share of local invocations that are no-change
    let builds_day = 40.0_f64;
    let workdays = 230.0_f64;
    let ci_hours_day = 2.0_f64; // CI machine-hours per engineer per day
    let ci_recoverable = 0.50_f64; // share of CI machine-time that is redundant
    let switch_prob = 0.35_f64;
    let switch_cost_s = 90.0_f64;
    let devs_world = 30e6_f64;
    let g_compiled = 0.35_f64; // share on ahead-of-time compiled/typechecked chains
    let shares = [0.01_f64, 0.05, 0.20]; // the scenario ladder; 20% = "top three"
    let share_top3 = 0.20_f64;
    let grid_gco2_kwh = 480.0_f64;
    let water_l_kwh = 1.8_f64;
    let household_kwh_yr = 3500.0_f64;
    let dev_cost_hour = 95.0_f64;
    let hours_fte_year = 8.0 * workdays;
    let rebounds = [0.0_f64, 0.5, 1.0];
    let ict_share_global_co2 = 0.02_f64; // ICT ~2% of global emissions (lit.)
    let global_co2_gt = 37.0_f64;
    let world_electricity_twh = 29_000.0_f64;
    let team_size = 8.0_f64;
    let releases_year = 12.0_f64;
    let p_defect_release = 0.05_f64;
    let p_green_given_defect_after = 0.05_f64;
    let ci_cache_non_adoption = 0.70_f64; // 70% of projects do not cache (lit.)
    let fingerprint_units = 170.0_f64;
    let digest_bits = 32.0 * 8.0;
    let t_room = 300.0_f64;

    // ══════════════════════════ RUNG 1 — ONE ENGINEER-YEAR
    let redundant_builds_year = builds_day * f_noop * workdays;
    let local_kwh_year = redundant_builds_year * dt * dev_watts / 3.6e6;
    let ci_kwh_year = ci_hours_day * workdays * (ci_watts / 1000.0) * ci_recoverable * pue;
    let kwh_engineer_year = local_kwh_year + ci_kwh_year;
    let switches_year = builds_day * f_noop * switch_prob * workdays;
    let seconds_year = redundant_builds_year * dt + switches_year * switch_cost_s;
    let hours_engineer_year = seconds_year / 3600.0;
    let fte_fraction = hours_engineer_year / hours_fte_year;
    let usd_engineer_year = hours_engineer_year * dev_cost_hour;

    // ══════════════════════════ RUNG 2/3 — POPULATION AND SHARE
    let addressable = devs_world * g_compiled;
    let engineers_top3 = addressable * share_top3;
    let kwh_top3 = engineers_top3 * kwh_engineer_year;
    let twh_top3 = kwh_top3 / 1e9;
    let t_co2_top3 = kwh_top3 * grid_gco2_kwh / 1e6; // tonnes
    let water_ml_top3 = kwh_top3 * water_l_kwh / 1e6; // megalitres
    let households_top3 = kwh_top3 / household_kwh_yr;
    let engineer_years_top3 = engineers_top3 * hours_engineer_year / hours_fte_year;
    let usd_top3 = engineers_top3 * usd_engineer_year;

    // proportion — the deflation
    let ict_mt = ict_share_global_co2 * global_co2_gt * 1000.0; // Mt CO2e
    let share_of_ict = 100.0 * (t_co2_top3 / 1e6) / ict_mt;
    let share_of_global_co2 = 100.0 * (t_co2_top3 / 1e6) / (global_co2_gt * 1000.0);
    let share_of_world_electricity = 100.0 * twh_top3 / world_electricity_twh;

    // ══════════════════════════ THE DEFAULT LEVER (bigger than the scenario)
    let default_lever_kwh = addressable * ci_kwh_year * ci_cache_non_adoption;
    let default_lever_twh = default_lever_kwh / 1e9;
    let default_lever_ratio = default_lever_twh / twh_top3;

    // ══════════════════════════ THE FLOOR, AT PLANETARY SCALE
    let ln2 = std::f64::consts::LN_2;
    let landauer_bit = BOLTZMANN * t_room * ln2;
    let floor_bits_build = fingerprint_units * digest_bits;
    let e_floor_build = floor_bits_build * landauer_bit;
    let builds_top3_year = engineers_top3 * redundant_builds_year;
    let floor_total_j = builds_top3_year * e_floor_build;
    let actual_local_j = engineers_top3 * local_kwh_year * 3.6e6;
    let floor_ratio = actual_local_j / floor_total_j;

    // ══════════════════════════ CORRECTNESS AT SCALE
    let releases_top3 = engineers_top3 / team_size * releases_year;
    let defect_releases = releases_top3 * p_defect_release;
    let caught_with_probe = defect_releases * (1.0 - p_green_given_defect_after);

    // ══════════════════════════ TABLES
    let ladder_rows = format!(
        "0 & One redundant build, our workspace & --- & {:.1}\\,s wasted, {} J on the measured box & \\stM \\\\\n\
         1 & One engineer-year & {}\\,builds$\\times${:.2} no-change & {:.1}\\,kWh + {:.1}\\,kWh CI $=$ {:.1}\\,kWh; \
         {:.0}\\,h & \\stD \\\\\n\
         2 & One 1{{,}}000-engineer organisation & $\\times$1{{,}}000 & {}\\,MWh; {}\\,h; {} & \\stD \\\\\n\
         3 & Addressable world population & {:.0}\\,M devs $\\times${:.2} compiled & {}\\,engineers & \\stA \\\\\n\
         4 & Top-three share & $\\times${:.0}\\% & {:.3}\\,TWh; {}\\,t\\,CO$_2$e; {}\\,engineer-years & \\stC \\\\\n",
        dt,
        thou(e_waste_measured_j),
        thou(builds_day * workdays),
        f_noop,
        local_kwh_year,
        ci_kwh_year,
        kwh_engineer_year,
        hours_engineer_year,
        thou(kwh_engineer_year),
        thou(hours_engineer_year * 1000.0),
        usd(usd_engineer_year * 1000.0),
        devs_world / 1e6,
        g_compiled,
        thou(addressable),
        share_top3 * 100.0,
        twh_top3,
        thou(t_co2_top3),
        thou(engineer_years_top3)
    );

    let mut scenario_rows = String::new();
    for s in shares {
        let eng = addressable * s;
        let kwh = eng * kwh_engineer_year;
        scenario_rows.push_str(&format!(
            "{:.0}\\% & {} & {:.3} & {} & {:.0} & {} & {} & {} \\\\\n",
            s * 100.0,
            thou(eng),
            kwh / 1e9,
            thou(kwh * grid_gco2_kwh / 1e6),
            kwh * water_l_kwh / 1e6,
            thou(kwh / household_kwh_yr),
            thou(eng * hours_engineer_year / hours_fte_year),
            usd_bn(eng * usd_engineer_year)
        ));
    }

    let mut rebound_rows = String::new();
    for r in rebounds {
        rebound_rows.push_str(&format!(
            "{:.0}\\% & {:.3}\\,TWh & {} & {} & {} \\\\\n",
            r * 100.0,
            twh_top3 * (1.0 - r),
            thou(t_co2_top3 * (1.0 - r)),
            thou(engineer_years_top3),
            if r >= 1.0 { "energy claim gone; time claim intact" } else { "both stand" }
        ));
    }

    // ══════════════════════════════════════════════════════════════════ LaTeX
    let mut doc = Document::new("article")
        .option("11pt")
        .option("a4paper")
        .package_opt("inputenc", &["utf8"])
        .package_opt("geometry", &["margin=1.05in"])
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
            "\\definecolor{sigilblue}{HTML}{1F4E79}\n",
            "\\definecolor{sigilgreen}{HTML}{1E6F3C}\n",
            "\\definecolor{slate}{HTML}{6B7280}\n",
            "\\definecolor{panelbg}{HTML}{F6F7F9}\n",
            "\\definecolor{rowtint}{HTML}{EEF2F5}\n",
            "\\newcommand{\\measured}[1]{\\textcolor{sigilcyan}{\\textbf{#1}}}\n",
            "\\newcommand{\\derivedv}[1]{\\textcolor{sigilviolet}{\\textbf{#1}}}\n",
            "\\newcommand{\\scen}[1]{\\textcolor{sigilamber}{\\textbf{#1}}}\n",
            "\\newcommand{\\stM}{\\textcolor{sigilcyan}{$\\bullet$}~{\\scriptsize\\textsc{measured}}}\n",
            "\\newcommand{\\stD}{\\textcolor{sigilviolet}{$\\oplus$}~{\\scriptsize\\textsc{derived}}}\n",
            "\\newcommand{\\stA}{\\textcolor{sigilamber}{$\\triangle$}~{\\scriptsize\\textsc{assumed}}}\n",
            "\\newcommand{\\stC}{\\textcolor{sigilred}{$\\diamond$}~{\\scriptsize\\textsc{scenario}}}\n",
            "\\newcommand{\\kpi}[3]{\\begin{minipage}[t]{0.31\\textwidth}\\begin{tcolorbox}[colback=panelbg,",
            "colframe=slate,boxrule=0.5pt,arc=2pt,halign=center]{\\large\\bfseries\\textcolor{#1}{#2}}\\\\[2pt]",
            "{\\scriptsize #3}\\end{tcolorbox}\\end{minipage}}\n",
            "\\newcommand{\\brief}[3]{\\begin{tcolorbox}[colback=panelbg,colframe=#1,boxrule=0.8pt,arc=2pt,",
            "title=\\textbf{#2},coltitle=white,colbacktitle=#1]#3\\end{tcolorbox}}\n",
            "\\hypersetup{pdftitle={The Idle Machine},pdfsubject={Planet-scale build waste, measured locally ",
            "and extrapolated honestly},pdfkeywords={build systems, green software, Jevons paradox, rebound ",
            "effect, compiler, software energy, incremental compilation}}\n",
            "\\title{\\textbf{The Idle Machine}\\\\[6pt]\\large What the World Spends Rebuilding What It Already ",
            "Built\\\\[4pt]\\normalsize\\itshape A measured kernel, an honest extrapolation, and a top-three-compiler ",
            "scenario}\n",
            "\\author{The Flux Foundation\\\\\\small model computed by \\texttt{flux-arxiv-latex}, ",
            "thermodynamic floor by \\texttt{flux-science},\\\\\\small related work drawn from a live arXiv sweep}\n",
            "\\date{\\today}"
        ))
        .add(Block::Raw("\\maketitle".into()));

    // ── KPI strip + abstract
    doc = doc
        .add(Block::Raw(format!(
            "\\vspace{{-12pt}}\\noindent\n\
             \\kpi{{sigilcyan}}{{{:.0}$\\times$}}{{\\textsc{{measured}} --- the redundant work removed from one \
             real workspace}}\\hfill\n\
             \\kpi{{sigilamber}}{{{}}}{{\\textsc{{scenario}} --- engineer-years returned per year at top-three \
             share}}\\hfill\n\
             \\kpi{{sigilred}}{{{:.4}\\%}}{{\\textsc{{scenario}} --- that same scenario's share of ICT carbon: \
             the headline we refuse}}\n\n\\vspace{{6pt}}\n",
            speedup,
            thou(engineer_years_top3),
            share_of_ict
        )))
        .add(Block::Raw(format!(
            "\\begin{{abstract}}\\noindent\nThe world runs a machine that produces nothing: it rebuilds software \
             that has not changed. This paper measures that waste stream in one fully instrumented workspace, \
             then extrapolates it in four numbered rungs --- every multiplier declared, printed and graded --- to \
             the scenario the title invites: a build orchestrator with top-three share of the world's compiled \
             builds. The measured kernel is a single bookkeeping defect: units restored from a content cache \
             skipped the compiler and therefore never wrote their fingerprint file, so \\measured{{{:.0}}} \
             poisoned roots cascaded into \\measured{{{:.0}}} downstream units and confirming that nothing had \
             changed cost \\measured{{{:.1}\\,s}} instead of \\measured{{{:.1}\\,s}} --- a {:.0}$\\times$ removal \
             of pure waste. Extrapolated at {:.0}\\% share, using laptop-class power rather than the {:.0}\\,W \
             box we actually measured on, the recoverable stream is \\scen{{{:.3}\\,TWh}} and \
             \\scen{{{}\\,t\\,CO$_2$e}} a year --- and here the paper deflates its own headline: that is \
             \\scen{{{:.4}\\%}} of ICT's carbon footprint and \\scen{{{:.5}\\%}} of world electricity. As a \
             climate intervention it is noise. What is not noise is time: \\scen{{{} engineer-years}} returned \
             every year, worth \\scen{{{}}} at the stated rate, and a correctness dividend on roughly \
             \\scen{{{}}} defect-carrying releases annually. We then apply the Jevons rebound honestly: at full \
             rebound the energy saving vanishes entirely while the time and correctness dividends survive intact, \
             which is why those are the claims we make. Two findings cut against our own interest. The \
             thermodynamic floor for deciding ``has anything changed?'' across the whole scenario is about \
             \\derivedv{{one microjoule per year}} for the entire planet --- we spend \
             $\\sim{}\\times$ that --- so the waste is coordination, not physics. And the single largest \
             planetary lever is not a new compiler but a \\emph{{default}}: turning build caching on by default \
             for the {:.0}\\% of projects that do not use it recovers \\scen{{{:.3}\\,TWh}}, \
             \\scen{{{:.1}$\\times$}} the entire top-three scenario.\n\\end{{abstract}}\n",
            poisoned_roots, poisoned_downstream, t_noop_before, t_noop_after, speedup,
            share_top3 * 100.0, measured_box_watts, twh_top3, thou(t_co2_top3),
            share_of_ict, share_of_world_electricity,
            thou(engineer_years_top3), usd_bn(usd_top3), thou(defect_releases),
            sci(floor_ratio), ci_cache_non_adoption * 100.0, default_lever_twh, default_lever_ratio
        )));

    // ── §1 status of the claim
    doc = doc
        .add(Block::Raw(
            "\\section{Status of this claim}\\label{sec:status}\n\n".to_string(),
        ))
        .add(Block::Raw(format!(
            "\\brief{{sigilred}}{{Read this before any number}}{{This paper has a \\textsc{{measured}} kernel and \
             a \\textsc{{scenario}} shell, and the two must not be confused.\\\\[4pt]\n\
             \\textbf{{Measured:}} one workspace, one instrument, dated. A no-op build of \
             {:.1}\\,s repaired to {:.1}\\,s; the heaviest binary at {:.0}\\,s across 166 units re-declared \
             dirty; a {:.0}\\% unit-level cache hit rate afterwards.\\\\[4pt]\n\
             \\textbf{{Scenario:}} the orchestrator in question has, at the time of writing, no meaningful share \
             of the world's builds. It is a version-0.39 project maintained by one operator and a fleet of \
             automated agents. ``Top three'' is a \\emph{{multiplication we perform in public}}, not a forecast, \
             a roadmap, or a claim of adoption. Nothing in \\S\\ref{{sec:scenario}} should be read as a \
             prediction.\\\\[4pt]\n\
             \\textbf{{Why publish it anyway:}} the unit economics --- kWh, hours and tonnes per engineer-year of \
             redundant build work --- are share-independent. They are the reusable part. Any reader can replace \
             our share, our population, or our power assumptions and recompute; the generator is a program, and \
             \\S\\ref{{sec:falsify}} lists the six measurements that would refute us.}}\n\n",
            t_noop_before, t_noop_after, t_fat_before, cache_unit_hit * 100.0
        )));

    // ── §2 what saving the world can mean
    doc = doc
        .add(Block::Raw(
            "\\section{What ``saving the world'' can and cannot mean here}\\label{sec:frame}\n\n".to_string(),
        ))
        .add(raw(
            "The ICT sector is responsible for roughly 2\\% of global carbon emissions \\textcite{arxiv2407_19901}, \
             with data centres estimated at 1.1--1.5\\% of world electricity in the earlier literature \
             \\textcite{arxiv1307_7037,arxiv2309_09241} and hyperscale facilities now measured directly at \
             facility level \\textcite{arxiv2606_05420}. Build and test compute is a slice of that slice. No \
             compiler --- however widely adopted, however efficient --- is a climate intervention at the scale \
             the phrase ``save the world'' implies, and this paper will not pretend otherwise. The field has \
             already been burned once by exactly this move: highly visible language-energy rankings were \
             causally misread by academics and industry leaders alike, a misreading that a careful causal model \
             later had to undo \\textcite{arxiv2410_05460}.",
        ))
        .add(raw(
            "What survives that deflation is still worth writing down. Three claims, in descending order of \
             defensibility:",
        ))
        .add(Block::Raw(format!(
            "\\begin{{enumerate}}[leftmargin=1.5em,itemsep=3pt]\n\
             \\item \\textbf{{There is a measurable waste stream}} --- machine time spent recomputing outputs \
             that are already known --- and it can be removed without asking a single human to change their \
             behaviour. That is rare among sustainability interventions, which usually require behaviour change \
             and therefore suffer rebound \\textcite{{arxiv2506_14653}}.\n\
             \\item \\textbf{{The dominant recovered resource is human, not electrical.}} At {:.0}\\% share the \
             scenario returns \\scen{{{} engineer-years}} a year against \\scen{{{}\\,t\\,CO$_2$e}} --- and the \
             carbon figure is {:.4}\\% of ICT's footprint, i.e.\\ indistinguishable from zero, while the time \
             figure is {:.1}\\% of the affected engineers' working lives.\n\
             \\item \\textbf{{The same instrument that removes the waste removes a correctness risk}}, and \
             software is infrastructure whose failures are expensive at civilisational scale \
             \\textcite{{arxiv2506_13821}}. This is the dividend we would defend hardest and monetise least.\n\
             \\end{{enumerate}}\n\n",
            share_top3 * 100.0, thou(engineer_years_top3), thou(t_co2_top3),
            share_of_ict, fte_fraction * 100.0
        )));

    // ── §3 the measured kernel
    doc = doc
        .add(Block::Raw("\\section{The waste stream, measured}\\label{sec:kernel}\n\n".to_string()))
        .add(Block::Raw(format!(
            "\\stM{{}} In one instrumented workspace, builds that should have been no-ops were not. Confirming \
             that an unchanged crate was unchanged cost {:.1}\\,s. The heaviest binary cost {:.0}\\,s across 166 \
             units re-declared dirty on every invocation. The cause was not compilation but bookkeeping: units \
             restored from the content cache skipped the compiler, so the fingerprint dependency file was never \
             written, and the build tool marked {:.0} roots permanently stale, cascading into {:.0} downstream \
             units. After the repair --- capture, require and materialise that file on cache restore --- the same \
             no-op measured {:.1}\\,s and the unit-level cache hit rate settled at {:.0}\\%.\n\n",
            t_noop_before, t_fat_before, poisoned_roots, poisoned_downstream, t_noop_after,
            cache_unit_hit * 100.0
        )))
        .add(Block::Raw(format!(
            "Two properties make this the right kernel for a planetary estimate. First, the removed work is \
             \\emph{{provably useless}}: its output was already known, which is why removing it changes nothing \
             observable except the wait. Second, the failure mode is \\emph{{structural, not local}} --- a \
             dependency-tracking invariant, of exactly the kind the build-systems literature has been modelling \
             for over a decade \\textcite{{arxiv1203_2704}}, and adjacent to the reasons build durations are \
             predictable at all \\textcite{{arxiv1712_06796}} and why CI failures dominate developer frustration \
             \\textcite{{arxiv2402_09651}}. Nothing about it is peculiar to this project's language or domain, \
             which is what licenses --- carefully --- the extrapolation that follows. On the machine the \
             measurement was taken on, one redundant build dissipated {} J.\n\n",
            thou(e_waste_measured_j)
        )));

    // ── §4 the ladder
    doc = doc
        .add(Block::Raw("\\section{The extrapolation ladder}\\label{sec:ladder}\n\n".to_string()))
        .add(raw(
            "Planetary numbers earn distrust because the multiplication is hidden. Here it is, one rung at a \
             time, each with its own evidence grade. A reader who rejects rung 3 can stop at rung 2 and still \
             keep everything above it.",
        ))
        .add(Block::Raw(format!(
            "\\begin{{center}}\\small\\rowcolors{{2}}{{rowtint}}{{white}}\
             \\begin{{tabularx}}{{\\textwidth}}{{@{{}}cXXXl@{{}}}}\\toprule\n\
             Rung & What & Multiplier & Result & Grade \\\\\\midrule\n{}\
             \\bottomrule\\end{{tabularx}}\\end{{center}}\n\n",
            ladder_rows
        )))
        .add(Block::Raw(format!(
            "\\textbf{{The assumptions, stated once.}} \\stA{{}} An engineer invokes the inner loop {:.0} times \
             a day over {:.0} working days, of which {:.0}\\% are no-change invocations; a wait beyond ten \
             seconds triggers a task switch with probability {:.2} costing {:.0}\\,s of recovery; CI consumes \
             {:.1} machine-hours per engineer per day at {:.0}\\,W with a data-centre PUE of {:.1}, of which \
             {:.0}\\% is redundant; there are {:.0} million professional developers worldwide and {:.0}\\% of \
             their work runs on ahead-of-time compiled or type-checked toolchains; the grid emits \
             {:.0}\\,gCO$_2$/kWh and consumes {:.1}\\,L of water per kWh; an engineer costs {} an hour fully \
             loaded.\n\n\
             \\textbf{{One assumption is deliberately against us.}} The local-loop power figure is \
             {:.0}\\,W --- laptop class --- although the measurement itself was taken on a {:.0}\\,W \
             server. Using our own instrument's power would multiply the local energy term by \
             {:.1}$\\times$. We discount our own measurement because most of the world's inner loops do not run \
             on 48-core machines, and because a scenario paper that inflates its own kernel deserves to be \
             disbelieved.\n\n",
            builds_day, workdays, f_noop * 100.0, switch_prob, switch_cost_s,
            ci_hours_day, ci_watts, pue, ci_recoverable * 100.0,
            devs_world / 1e6, g_compiled * 100.0, grid_gco2_kwh, water_l_kwh, usd(dev_cost_hour),
            dev_watts, measured_box_watts, measured_box_watts / dev_watts
        )));

    // ── §5 the scenario
    doc = doc
        .add(Block::Raw("\\section{The scenario: one, five, and twenty per cent}\\label{sec:scenario}\n\n".to_string()))
        .add(Block::Raw(format!(
            "\\stC{{}} ``Top three'' is read here as {:.0}\\% of the world's compiled build invocations. The one \
             and five per cent rows are included because they are the only rows any real project reaches first, \
             and because a claim that only works at the top of the table is not an argument.\n\n\
             \\begin{{center}}\\footnotesize\\rowcolors{{2}}{{rowtint}}{{white}}\
             \\begin{{tabular}}{{lrrrrrrr}}\\toprule\n\
             Share & Engineers & TWh/yr & t\\,CO$_2$e/yr & ML water & Households & Eng-years/yr & Value/yr \\\\\\midrule\n{}\
             \\bottomrule\\end{{tabular}}\\end{{center}}\n\n",
            share_top3 * 100.0, scenario_rows
        )))
        .add(Block::Raw(format!(
            "\\textbf{{Now the deflation.}} At the top row the recovered electricity is {:.3}\\,TWh --- \
             \\scen{{{:.5}\\%}} of world electricity consumption --- and the avoided carbon is \
             {}\\,t\\,CO$_2$e, or \\scen{{{:.4}\\%}} of ICT's estimated footprint and \
             \\scen{{{:.6}\\%}} of global emissions. Rounded to the precision anyone reports climate policy in, \
             it is zero. Any paper that led with the tonnage would be innumerate, and the literature on \
             software's energy footprint is explicit that estimates at this remove need their methodology stated \
             before their totals \\textcite{{arxiv2506_09683,arxiv2407_11611}}.\n\n\
             \\textbf{{What the same row says about people.}} {} engineer-years returned annually, {} at the \
             stated rate, or {:.1}\\% of the working life of every engineer in scope. That is not a rounding \
             error in anyone's units. It is, at the top row, the equivalent of hiring a mid-sized nation's \
             software workforce and paying it nothing --- and it arrives without a single behavioural request, \
             which is the property that makes it unusual.\n\n",
            twh_top3, share_of_world_electricity, thou(t_co2_top3), share_of_ict,
            share_of_global_co2, thou(engineer_years_top3), usd_bn(usd_top3), fte_fraction * 100.0
        )))
        .add(Block::Raw(format!(
            "For completeness, the two figures a sustainability report would normally lead with: the same \
             electricity carries \\scen{{{:.0}\\,ML}} of cooling water at the assumed {:.1}\\,L/kWh, and \
             corresponds to the annual consumption of \\scen{{{}}} households at {}\\,kWh each. Both are \
             included because omitting them would look like selection, and both are subject to the same \
             deflation: they are small, and \\S\\ref{{sec:rebound}} may erase them entirely.\n\n",
            water_ml_top3, water_l_kwh, thou(households_top3), thou(household_kwh_yr)
        )));

    // ── §6 rebound
    doc = doc
        .add(Block::Raw("\\section{The rebound, applied to ourselves}\\label{sec:rebound}\n\n".to_string()))
        .add(raw(
            "Efficiency gains in computing are routinely re-spent. The Jevons paradox has been given an explicit \
             thermodynamic reading for cloud workloads \\textcite{arxiv2411_11540}; the AI environmental debate \
             has been analysed precisely as a rebound argument \\textcite{arxiv2501_16548}; measured ML training \
             footprints keep rising \\emph{despite} per-unit efficiency gains, which is the rebound observed \
             rather than theorised \\textcite{arxiv2510_09022}; sustainable-HCI work argues rebound should be \
             embraced in the model rather than apologised for afterwards \\textcite{arxiv2506_14653}; and the \
             materiality of the sector bounds what any efficiency story can deliver \\textcite{arxiv2507_19287}. \
             A build system that makes builds nearly free is a textbook candidate: cheaper builds invite more \
             builds, larger matrices, more speculative CI.",
        ))
        .add(Block::Raw(format!(
            "\\stD{{}} So we apply it to our own headline. Let $\\rho$ be the fraction of the recovered machine \
             time immediately re-spent on additional build work.\n\n\
             \\begin{{center}}\\small\\rowcolors{{2}}{{rowtint}}{{white}}\
             \\begin{{tabularx}}{{\\textwidth}}{{@{{}}lrrrX@{{}}}}\\toprule\n\
             $\\rho$ & Net TWh/yr & Net t\\,CO$_2$e/yr & Eng-years/yr & Verdict \\\\\\midrule\n{}\
             \\bottomrule\\end{{tabularx}}\\end{{center}}\n\n",
            rebound_rows
        )))
        .add(raw(
            "\\textbf{The asymmetry is the whole point.} At $\\rho=1$ the energy dividend is exactly zero and \
             the carbon claim disappears with it. The time dividend does not, because re-spending recovered \
             \\emph{human} attention on building more software is not a leak in the model --- it is the outcome \
             the model exists to produce. Rebound destroys an energy argument and completes a productivity one. \
             This is why the paper's load-bearing claims are time and correctness, and why the tonnage appears \
             only to be dismissed. A reader who believes $\\rho=1$ --- a defensible position on this literature \
             --- loses nothing this paper actually argues.",
        ));

    // ── §7 correctness at scale
    doc = doc
        .add(Block::Raw("\\section{The correctness dividend at scale}\\label{sec:correct}\n\n".to_string()))
        .add(Block::Raw(format!(
            "\\stC{{}} The same discipline that found the redundant work --- independent verification of what an \
             instrument claims --- has a second output. In the companion paper the measured case was a storage \
             layer that passed its entire unit suite while returning 0.4\\% of reads, a suite carrying \
             $\\sim$0.001 bits of information about the property anyone cared about. Applying the same \
             arithmetic at scenario scale: {:.0}\\% share implies roughly {} releases a year at a team size of \
             {:.0} and {:.0} releases per team per year, of which {} carry a defect at the assumed \
             {:.0}\\% rate. A suite with zero verification yield passes all of them; an independent correctness \
             probe that catches the class {:.0}\\% of the time stops {}.\n\n",
            share_top3 * 100.0, thou(releases_top3), team_size, releases_year,
            thou(defect_releases), p_defect_release * 100.0,
            (1.0 - p_green_given_defect_after) * 100.0, thou(caught_with_probe)
        )))
        .add(raw(
            "We deliberately do not monetise that number. Escape costs for infrastructure software span orders \
             of magnitude --- from a wasted afternoon to the historical failures catalogued in the formal-methods \
             literature \\textcite{arxiv2506_13821} --- and any single dollar figure would be the least \
             defensible line in the paper. The flaky-test literature makes the mechanism concrete from the noise \
             side: ambiguous signals interfere with automated assessment of changes at industrial scale \
             \\textcite{arxiv2602_03556}. A build system with top-three share would be the largest single \
             deployment surface for that ambiguity in the world, which is an argument for humility about \
             defaults, not a boast.",
        ));

    // ── §8 the floor
    doc = doc
        .add(Block::Raw("\\section{One microjoule per planet-year}\\label{sec:floor}\n\n".to_string()))
        .add(Block::Raw(format!(
            "The redundant fraction has an unusual property: because its output is already known, the only \
             physically necessary work is the \\emph{{decision}} that nothing changed. Erasing a bit at \
             $T={:.0}$\\,K costs at least $k_BT\\ln2 = {}$\\,J \\textcite{{arxiv1507_07450,arxiv2504_04284}}. \
             Comparing one digest per unit across a {:.0}-unit workspace is ${:.0}$ bits, so one such decision \
             has a floor of ${}$\\,J. At {:.0}\\% share the scenario performs {} such decisions a year, for a \
             total thermodynamic floor of \\derivedv{{${}$\\,J}} --- about one microjoule, for the entire \
             planet, for a year.\n\n",
            t_room, sci(landauer_bit), fingerprint_units, floor_bits_build, sci(e_floor_build),
            share_top3 * 100.0, thou(builds_top3_year), sci(floor_total_j)
        )))
        .add(Block::Raw(format!(
            "\\stD{{}} The same decisions actually consume ${}$\\,J of local-loop electricity: a factor of \
             ${}$ above the floor. The claim needs its scope stated precisely, or it is dishonest: this floor \
             applies \\emph{{only}} to the redundant fraction, where nothing is compiled and no new information \
             is produced. Compiling changed code has a vastly higher and quite legitimate floor. But that is the \
             point --- the waste stream is precisely the part of the bill that physics does not require, and it \
             sits some twenty orders of magnitude above its own minimum. Whatever is expensive about building \
             software, it is not the universe's opinion of the work.\n\n",
            sci(actual_local_j), sci(floor_ratio)
        )));

    // ── §9 the default lever
    doc = doc
        .add(Block::Raw("\\section{The largest lever is a default, not a compiler}\\label{sec:default}\n\n".to_string()))
        .add(Block::Raw(format!(
            "A large-scale study of 513{{,}}384 CI builds across 1{{,}}279 projects found that only 30\\% adopt \
             caching at all \\textcite{{arxiv2601_19146}}. That is not a technology gap; it is a \
             \\emph{{configuration}} gap, and it is the most consequential number in this paper.\n\n\
             \\stC{{}} Apply our own CI term to the {:.0}\\% of projects that do not cache, across the whole \
             addressable population rather than any one tool's share: \\scen{{{:.3}\\,TWh}} a year --- \
             \\scen{{{:.1}$\\times$}} the entire top-three scenario in \\S\\ref{{sec:scenario}}, available today, \
             from tools that already exist, with no adoption of anything we build.\n\n",
            ci_cache_non_adoption * 100.0, default_lever_twh, default_lever_ratio
        )))
        .add(raw(
            "We report this because it is the finding a self-interested paper would omit. The lesson generalises \
             past caching: the planetary footprint of a build tool is set less by its peak performance than by \
             what it does when nobody configures it. A tool's defaults are its actual environmental policy. \
             Green-software practice reaches the same conclusion from the field --- the wins that survive are \
             the ones wired into how the system runs by default rather than left to intention \
             \\textcite{arxiv2601_09741}, alongside genuinely orthogonal levers such as carbon-aware scheduling \
             and workload placement \\textcite{arxiv2405_12582,arxiv2506_10990}, and the observation that \
             ordinary performance bugs carry a carbon cost of their own \\textcite{arxiv2401_01782}.",
        ));

    // ── §10 duties
    doc = doc
        .add(Block::Raw("\\section{What a top-three compiler would owe}\\label{sec:duties}\n\n".to_string()))
        .add(raw(
            "If the scenario in \\S\\ref{sec:scenario} were ever real, the interesting consequence would not be \
             the savings. It would be that the tool had become infrastructure, and infrastructure acquires \
             duties. Stated as commitments a reader could later hold us to:",
        ))
        .add(Block::Raw(format!(
            "\\begin{{enumerate}}[leftmargin=1.5em,itemsep=3pt]\n\
             \\item \\textbf{{Determinism before speed.}} Every performance claim paired with an independent \
             correctness check on the same artifact. A cache that reports its own hit rate is not evidence; this \
             project has the scar to prove it.\n\
             \\item \\textbf{{Cache on by default, and provably free when nothing changed.}} The {:.0}\\% \
             non-adoption figure is a design failure upstream of every user who did not configure it.\n\
             \\item \\textbf{{Signed, reproducible artifacts.}} A build tool at scale is a supply-chain choke \
             point; pipeline poisoning is a demonstrated attack class \\textcite{{arxiv2601_08995}}, end-to-end \
             reproducibility is achievable \\textcite{{arxiv2206_14606}}, and signing's costs are exactly why \
             adoption stalls \\textcite{{arxiv2510_04964,arxiv2409_05014}}. At top-three share those costs stop \
             being optional.\n\
             \\item \\textbf{{No telemetry rent.}} Measurement of the tool must not require surveillance of its \
             users. Everything in this paper was measured on the authors' own machines.\n\
             \\item \\textbf{{Published measurement, including the failures.}} The instrument that lied is part \
             of the record, not an embarrassment to be pruned from it.\n\
             \\item \\textbf{{Energy reporting with its methodology attached.}} Per the state of the art, a \
             total without a stated method is not a measurement \\textcite{{arxiv2506_09683,arxiv2407_11611}} --- \
             and LLM-assisted development is now itself a term in the same budget \\textcite{{arxiv2505_04521}}, \
             as is the cost of automated repair \\textcite{{arxiv2211_12104}}.\n\
             \\end{{enumerate}}\n\n",
            ci_cache_non_adoption * 100.0
        )));

    // ── §11 falsification
    doc = doc
        .add(Block::Raw("\\section{What would have to be true}\\label{sec:falsify}\n\n".to_string()))
        .add(raw(
            "Six measurements would move this paper's numbers, and four of them could refute it outright. Each \
             is stated as a gate someone could actually run.",
        ))
        .add(Block::Raw(format!(
            "\\begin{{center}}\\small\\rowcolors{{2}}{{rowtint}}{{white}}\
             \\begin{{tabularx}}{{\\textwidth}}{{@{{}}lXl@{{}}}}\\toprule\n\
             \\# & Gate & Effect if it fails \\\\\\midrule\n\
             1 & \\textbf{{The no-change fraction.}} We assume {:.0}\\% of inner-loop invocations change nothing. \
             Measure it on a real fleet by logging build inputs. & Linear: the entire local term scales with it. \\\\\n\
             2 & \\textbf{{Per-build energy on real hardware.}} Ours is one {:.0}\\,W box, discounted to \
             {:.0}\\,W by assumption. Measure at the wall across representative laptops and CI runners \
             \\textcite{{arxiv2407_11611}}. & Linear, either direction; could move the energy claim by 10$\\times$. \\\\\n\
             3 & \\textbf{{The redundant CI share.}} We assume {:.0}\\% of CI machine-time is recomputation. \
             Instrument a large CI estate. & CI is the larger of the two energy terms. \\\\\n\
             4 & \\textbf{{Generality of the defect.}} Our kernel is one dependency-tracking bug. Sample other \
             toolchains for equivalent no-op inflation. & If it is peculiar to us, rungs 3--4 collapse entirely. \\\\\n\
             5 & \\textbf{{The rebound coefficient.}} We report $\\rho\\in\\{{0,0.5,1\\}}$ rather than estimating \
             it. Measure build volume before and after a large speedup. & At $\\rho=1$ the energy claim is void \
             by construction (\\S\\ref{{sec:rebound}}). \\\\\n\
             6 & \\textbf{{Population and share.}} {:.0}\\,M developers, {:.0}\\% compiled toolchains, {:.0}\\% \
             share are all assumptions with no instrument behind them. & Rung 3 and 4 are scenario-grade for \
             exactly this reason. \\\\\n\
             \\bottomrule\\end{{tabularx}}\\end{{center}}\n\n",
            f_noop * 100.0, measured_box_watts, dev_watts, ci_recoverable * 100.0,
            devs_world / 1e6, g_compiled * 100.0, share_top3 * 100.0
        )))
        .add(raw(
            "\\textbf{What we do not claim.} Not that this is a climate intervention --- \\S\\ref{sec:scenario} \
             computes it as noise. Not that the scenario share is achievable. Not that redundant build work is \
             the largest waste stream in computing; training and inference are plainly larger and are being \
             measured by others \\textcite{arxiv2510_09022,arxiv2501_16548}. Not that removing waste is the same \
             as building well. The claim is narrower and, we think, sturdier: there is a quantity of pure \
             recomputation in the world, it is measurable from one honest instrument outward, most of it is a \
             configuration failure rather than a technical limit, and the resource it wastes that no one gets \
             back is time.",
        ));

    // ── related work
    if !papers.is_empty() {
        let mut rw = String::from(
            "\\section{Related work}\n\\noindent Retrieved by a live arXiv query at generation time; each entry \
             is glossed with the opening claim of its own abstract, unedited.\n\n\
             \\begin{itemize}[leftmargin=1.3em,itemsep=2pt]\n",
        );
        for p in &papers {
            let gloss = p.summary.split(['.', '\n']).next().unwrap_or("").trim();
            let gloss: String = gloss.chars().take(200).collect();
            rw.push_str(&format!(
                "\\item \\textcite{{{}}} \\emph{{{}}} --- {}.\n",
                p.cite_key(),
                latex_escape(&p.title),
                latex_escape(&gloss)
            ));
        }
        rw.push_str("\\end{itemize}\n\n");
        doc = doc.add(Block::Raw(rw));
    }

    // ── sources
    doc = doc.add(Block::Raw(
        "\\section*{Sources --- the measured record behind the kernel}\n\\label{sec:sources}\n\\small\n\
         \\begin{itemize}[leftmargin=1.2em,itemsep=1pt]\n\
         \\item \\emph{The Legibility Dividend: An Executive Model of Verifiable Software Production}, v0 \
         (2026-07-26) --- the immediate predecessor: the same kernel priced for one organisation, and the source \
         of the Verification Information Yield instrument used in \\S\\ref{sec:correct}. \
         \\url{https://quillon.xyz/downloads/SIGIL_LEGIBILITY_DIVIDEND_v0.pdf}.\n\
         \\item \\emph{The Measurement Book: Laws of the SIGIL Graph --- the measured record}, v0 (2026-07-25) --- \
         ARC-15 is the build-latency arc measured in \\S\\ref{sec:kernel}; its Rule 0 is why this paper states \
         which rung every number sits on. \\url{https://quillon.xyz/downloads/SIGIL_MEASUREMENT_BOOK_v0.pdf}.\n\
         \\item \\emph{The SIGIL Failure Atlas}, v0 (2026-07-23) --- the incident taxonomy, including the \
         measurement class that contains the instrument which lied. \
         \\url{https://quillon.xyz/downloads/SIGIL_FAILURE_ATLAS_v0.pdf}.\n\
         \\item \\emph{The Thermodynamic Ledger}, v0 (2026-07-24) --- the first paper in this computed-at-\
         generation-time series; the Landauer treatment in \\S\\ref{sec:floor} follows its method. \
         \\url{https://quillon.xyz/downloads/SIGIL_THERMODYNAMIC_LEDGER_v0.pdf}.\n\
         \\item \\emph{What the Chain Knows --- The Philosophy of the SIGIL Graph}, v0.3 (2026-07-22) --- the \
         evidence-grading discipline this paper applies to its own extrapolation. \
         \\url{https://quillon.xyz/downloads/SIGIL_PHILOSOPHY_v0.pdf}.\n\
         \\end{itemize}\n\n"
            .to_string(),
    ));

    // ── reproducibility + coda
    doc = doc
        .add(Block::Section("Reproducibility".into()))
        .add(Block::Raw(format!(
            "This document is the output of a Rust binary in the Flux tree \
             (\\texttt{{flux-arxiv-latex/}}\\allowbreak\\texttt{{src/bin/}}\\allowbreak\\texttt{{idle\\_machine.rs}}), \
             built and run through the \\texttt{{fluxc}} orchestrator. Every figure above is computed at \
             generation time; the measured constants and the assumed multipliers are declared once, at the top of \
             the program, as named variables in two clearly separated blocks. The Landauer floor uses \
             \\texttt{{flux-science}}'s CODATA Boltzmann constant ($k_B={}$\\,J/K). The bibliography is generated \
             from a live arXiv sweep parsed by the same crate. To disagree with this paper, change a variable and \
             recompile: the argument is a program, and its conclusions are functions of inputs you can see. That \
             is the only form in which a planetary extrapolation deserves to be published.\n\n",
            sci(BOLTZMANN)
        )))
        .add(Block::Section("Coda: the machine that produces nothing".into()))
        .add(raw(
            "There is something absurd, and worth sitting with, about the scale of what has been described here. \
             Every day, a very large number of extremely fast machines are asked a question to which the answer \
             is already recorded, and they answer it slowly, by redoing the work. Physics charges about a \
             microjoule a year for the whole planet's worth of that question. We pay it in gigawatt-hours and, \
             far more expensively, in the attention of the people waiting.",
        ))
        .add(raw(
            "The temptation with a number like that is to make it a crusade. We have tried instead to make it \
             legible, because the honest version is more useful than the heroic one: the carbon is noise, the \
             rebound may eat the electricity entirely, and the largest available lever is not our compiler but a \
             checkbox that most projects never tick. What is left when all of that is subtracted is still a real \
             thing --- hundreds of thousands of engineer-years a year of human life spent watching a progress \
             bar recompute a known answer, and a class of verification failure that the same discipline happens \
             to catch on the way past.",
        ))
        .add(raw(
            "A compiler will not save the world. But a tool used by a large fraction of the world has one \
             obligation it cannot delegate: to not waste what it is given. That is a smaller promise than the \
             title of this paper implies, and it is the one we can keep --- and, unusually for a promise, one \
             whose keeping is measurable by anyone who cares to rerun the program.",
        ));

    // ── bibliography
    if !papers.is_empty() {
        let mut bib = String::from("\\begin{thebibliography}{99}\n");
        for p in &papers {
            let mut authors: Vec<String> = p.authors.iter().take(3).map(|a| latex_escape(a)).collect();
            if p.authors.len() > 3 {
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
        bib.push_str("\\end{thebibliography}\n");
        doc = doc.add(Block::Raw(bib));
    }

    // ── emit
    std::fs::create_dir_all(out_dir).expect("out dir");
    std::fs::write(format!("{out_dir}/idle_machine.bib"), bibliography(&papers)).expect("bib");
    let res = doc.compile_pdf(out_dir, "FLUX_IDLE_MACHINE_v0");
    if res.success {
        println!("OK {}", res.pdf_path.unwrap());
        println!(
            "kernel: {:.0}x | per-eng-yr: {:.1} kWh, {:.0} h | top3: {:.3} TWh, {:.0} tCO2e ({:.4}% of ICT), {:.0} eng-yr, {}",
            speedup, kwh_engineer_year, hours_engineer_year, twh_top3, t_co2_top3, share_of_ict,
            engineer_years_top3, usd_bn(usd_top3)
        );
        println!(
            "default lever: {:.3} TWh = {:.1}x scenario | floor: {} J total, ratio {}",
            default_lever_twh, default_lever_ratio, sci(floor_total_j), sci(floor_ratio)
        );
    } else {
        let tail: String = res
            .log
            .lines()
            .rev()
            .take(40)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        eprintln!("FAILED\n{tail}");
        std::process::exit(1);
    }
}
