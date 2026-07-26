//! legibility_dividend — "The Legibility Dividend": an executive-grade science
//! paper on the *business* value of measured, verifiable software production.
//!
//! Same dogfood contract as `thermodynamic_ledger`: every quantitative claim is
//! COMPUTED at document-generation time. Here the model is economic rather than
//! physical — cost of build latency, the information content of a green test
//! run (Verification Information Yield), a Pareto allocator over a real failure
//! taxonomy, and a provenance/audit ledger — anchored on measured numbers from
//! the SIGIL/Flux record, with the thermodynamic floor supplied by
//! `flux-science` so the reader can see how much of engineering cost is physics
//! (almost none) and how much is coordination (almost all).
//!
//! Usage: legibility_dividend [arxiv.json] [out_dir]
use flux_arxiv_latex::doc::{Block, Document};
use flux_arxiv_latex::{bibliography, latex_escape, parse_arxiv_json, related_work_section, ArxivPaper};
use flux_science::constants::BOLTZMANN;

// ─────────────────────────────────────────────────────────── formatting helpers

/// Thousands-separated integer-ish money, e.g. `1{,}234`.
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

/// `\$1{,}234` — LaTeX-safe currency.
fn usd(x: f64) -> String {
    format!("\\${}", thou(x))
}

/// Compact scientific notation for math mode.
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

fn para(s: String) -> Block {
    Block::Raw(format!("{s}\n\n"))
}
fn raw(s: &str) -> Block {
    Block::Raw(format!("{s}\n\n"))
}

// ──────────────────────────────────────────────────────────── the economic model

/// Shannon mutual information $I(D;G)$ in bits between "a defect is present"
/// and "the suite reported green". This is the paper's central instrument: a
/// test suite is an *information channel*, and a green run that a broken build
/// also produces carries zero bits.
fn mutual_information_bits(p_defect: f64, p_green_given_defect: f64, p_green_given_clean: f64) -> f64 {
    let pd = p_defect;
    let pc = 1.0 - pd;
    // joint distribution over (defect?, green?)
    let cells = [
        (pd * p_green_given_defect, pd, pd * p_green_given_defect + pc * p_green_given_clean),
        (
            pd * (1.0 - p_green_given_defect),
            pd,
            pd * (1.0 - p_green_given_defect) + pc * (1.0 - p_green_given_clean),
        ),
        (pc * p_green_given_clean, pc, pd * p_green_given_defect + pc * p_green_given_clean),
        (
            pc * (1.0 - p_green_given_clean),
            pc,
            pd * (1.0 - p_green_given_defect) + pc * (1.0 - p_green_given_clean),
        ),
    ];
    let mut i = 0.0;
    for (joint, p_row, p_col) in cells {
        if joint > 0.0 && p_row > 0.0 && p_col > 0.0 {
            i += joint * (joint / (p_row * p_col)).log2();
        }
    }
    i.max(0.0)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let json_path = args
        .get(1)
        .map(String::as_str)
        .unwrap_or("crates/flux-arxiv-latex/legibility_dividend.arxiv.json");
    let out_dir = args.get(2).map(String::as_str).unwrap_or("/tmp/legibility-dividend");

    let papers: Vec<ArxivPaper> = std::fs::read_to_string(json_path)
        .ok()
        .and_then(|j| parse_arxiv_json(&j).ok())
        .unwrap_or_default();

    // ══════════════════ MEASURED INPUTS (SIGIL/Flux record, 2026-05 → 2026-07)
    // Build-system pathology and its repair (Measurement Book ARC-15).
    let t_noop_before = 215.9_f64; // s, "no-op" check of an unchanged crate
    let t_noop_after = 1.0_f64; // s, re-verified next day (heal run: 0.56 s)
    let t_fat_before = 359.0_f64; // s, fattest binary, 166 dirty units
    let poisoned_roots = 19.0_f64;
    let poisoned_downstream = 147.0_f64;
    let t_incremental_after = 8.9_f64; // s, 3 edited crates incl. the fat bin
    let cache_unit_hit = 0.36_f64; // measured unit-level cache hit rate

    // Failure taxonomy (Failure Atlas v0): 71 incidents, ten classes.
    let classes: [(&str, f64); 10] = [
        ("Silent divergence", 12.0),
        ("Protocol wedge", 10.0),
        ("Stale state", 9.0),
        ("Resource exhaustion", 8.0),
        ("Bad default", 7.0),
        ("Measurement (the instrument lied)", 7.0),
        ("Economic (value created/destroyed)", 6.0),
        ("Key management", 6.0),
        ("Toolchain", 5.0),
        ("Serialization panic", 1.0),
    ];
    let incidents_total: f64 = classes.iter().map(|c| c.1).sum();
    let sev_catastrophic = 15.0_f64;
    let sev_major = 44.0_f64;
    let sev_moderate = 12.0_f64;
    let measurement_class = 7.0_f64;
    let stale_state_class = 9.0_f64;

    // The false-green anchor: a storage layer passed its whole unit suite while
    // an independent probe measured 0.4% of reads returning data.
    let suite_green = 105.0_f64;
    let probe_read_success = 0.004_f64;

    // Verification asset (fold proof): constant-size proof, bounded verify.
    let proof_bytes = 2568.0_f64;
    let verify_ms_full = 342.0_f64; // worst of the 280–342 ms band at 100k blocks
    let verify_us_tip = 3.0_f64;

    // Rule 0 anchor: microbench vs live, four orders apart.
    let microbench_rate = 10_000_000.0_f64; // records/s, flat store
    let live_rate = 800.0_f64; // blocks/s, live node at the time
    let rule0_gap = microbench_rate / live_rate;

    // ══════════════════ DECLARED BUSINESS ASSUMPTIONS (not measured — stated)
    let dev_cost_hour = 95.0_f64; // fully loaded engineer cost, USD/h
    let auditor_cost_hour = 150.0_f64;
    let workdays = 230.0_f64;
    let builds_day = 40.0_f64; // central case
    let builds_sensitivity = [10.0_f64, 20.0, 40.0, 80.0];
    let team_sizes = [1.0_f64, 10.0, 100.0, 1000.0];
    let switch_threshold_s = 10.0_f64;
    let switch_prob = 0.35_f64; // fraction of >10 s waits that trigger a task switch
    let switch_cost_s = 90.0_f64; // recovery cost of one switch
    let releases_year = 12.0_f64;
    let p_defect_release = 0.05_f64;
    let c_escape = 1_000_000.0_f64; // cost of one escaped data-integrity defect
    let p_green_given_defect_before = 1.0_f64; // MEASURED in the false-green case
    let p_green_given_defect_after = 0.05_f64; // with an independent correctness probe
    let p_green_given_clean = 0.98_f64; // residual flakiness on a clean tree
    let probe_hours_year = 80.0_f64; // cost of building/running the probe
    let atlas_hours = 60.0_f64; // cost of writing the failure taxonomy
    let depinfo_hours = 40.0_f64; // cost of the build-latency investigation
    let provenance_hours_year = 120.0_f64; // cost of signing + commitment records
    let audit_hours_saved = 200.0_f64; // external audit hours displaced per year
    let p_supply_chain = 0.02_f64; // annual probability of a supply-chain incident
    let c_supply_chain = 5_000_000.0_f64;
    // Deliberately pessimistic: the record holds 15 catastrophic incidents, but we
    // credit the taxonomy with preventing only one repeat every three years.
    let cat_repeats_avoided_year = 1.0_f64 / 3.0;
    let compute_cost_hour = 0.10_f64; // cloud core-hour, USD

    // Machine + energy assumptions.
    let cores = 48.0_f64;
    let watts_core = 15.0_f64;
    let box_watts = cores * watts_core;
    let kwh_price = 0.30_f64;
    let fingerprint_units = 170.0_f64; // workspace units to answer "did anything change?"
    let digest_bits = 32.0 * 8.0; // one BLAKE3 digest per unit
    let t_room = 300.0_f64;

    // ══════════════════ DIVIDEND 1 — build latency as a wage bill
    let dt = t_noop_before - t_noop_after;
    let direct_s_day = builds_day * dt;
    // waits above the task-switch threshold also cost a recovery
    let switches_before = if t_noop_before > switch_threshold_s { builds_day * switch_prob } else { 0.0 };
    let switches_after = if t_noop_after > switch_threshold_s { builds_day * switch_prob } else { 0.0 };
    let switch_s_day = (switches_before - switches_after) * switch_cost_s;
    let total_s_day = direct_s_day + switch_s_day;
    let hours_day = total_s_day / 3600.0;
    let hours_dev_year = hours_day * workdays;
    let usd_dev_year = hours_dev_year * dev_cost_hour;
    let fte_equiv = hours_dev_year / (8.0 * workdays); // recovered FTE per engineer
    let speedup_noop = t_noop_before / t_noop_after;
    let d1_cost = depinfo_hours * dev_cost_hour;
    let d1_payback_days = d1_cost / (usd_dev_year / workdays);
    let l_d1 = usd_dev_year / d1_cost;

    // energy side of the same dividend
    let e_noop_before = t_noop_before * box_watts; // J
    let e_noop_after = t_noop_after * box_watts;
    let kwh_saved_dev_year = (e_noop_before - e_noop_after) * builds_day * workdays / 3.6e6;
    let energy_usd_dev_year = kwh_saved_dev_year * kwh_price;

    // ══════════════════ DIVIDEND 2 — Verification Information Yield (VIY)
    let viy_before = mutual_information_bits(p_defect_release, p_green_given_defect_before, p_green_given_clean);
    let viy_after = mutual_information_bits(p_defect_release, p_green_given_defect_after, p_green_given_clean);
    let ev_loss_before = p_defect_release * p_green_given_defect_before * c_escape;
    let ev_loss_after = p_defect_release * p_green_given_defect_after * c_escape;
    let ev_delta_release = ev_loss_before - ev_loss_after;
    let ev_delta_year = ev_delta_release * releases_year;
    let d2_cost = probe_hours_year * dev_cost_hour;
    let l_d2 = ev_delta_year / d2_cost;
    let breakeven_hours_release = ev_delta_release / dev_cost_hour;
    let usd_per_bit = ev_delta_release / (viy_after - viy_before).max(1e-12);
    let d2_payback_days = d2_cost / (ev_delta_year / 365.0);

    // ══════════════════ DIVIDEND 3 — the taxonomy as a budget allocator
    let mut cum = 0.0_f64;
    let mut pareto_rows = String::new();
    let mut top3 = 0.0_f64;
    let mut top5 = 0.0_f64;
    for (i, (name, n)) in classes.iter().enumerate() {
        cum += n;
        if i < 3 {
            top3 += n;
        }
        if i < 5 {
            top5 += n;
        }
        pareto_rows.push_str(&format!(
            "{} & {} & {:.1}\\% & {:.1}\\% \\\\\n",
            latex_escape(name),
            n.round() as i64,
            100.0 * n / incidents_total,
            100.0 * cum / incidents_total
        ));
    }
    let top3_share = 100.0 * top3 / incidents_total;
    let top5_share = 100.0 * top5 / incidents_total;
    let cat_share = 100.0 * sev_catastrophic / incidents_total;
    let instrument_share = 100.0 * measurement_class / incidents_total;
    let two_guard_share = 100.0 * (measurement_class + stale_state_class) / incidents_total;
    let value_d3 = cat_repeats_avoided_year * c_escape;
    let d3_cost = atlas_hours * dev_cost_hour;
    let l_d3 = value_d3 / d3_cost;

    // ══════════════════ DIVIDEND 4 — provenance as an audit asset
    let audit_value = audit_hours_saved * auditor_cost_hour;
    let supply_chain_ev = p_supply_chain * c_supply_chain * 0.5; // signing addresses half the vector
    let value_d4 = audit_value + supply_chain_ev;
    let d4_cost = provenance_hours_year * dev_cost_hour;
    let l_d4 = value_d4 / d4_cost;
    let verify_cost_per_check = (verify_ms_full / 1000.0 / 3600.0) * compute_cost_hour; // cloud core-hour
    let checks_per_dollar = 1.0 / verify_cost_per_check.max(1e-12);

    // ══════════════════ THE PHYSICAL FLOOR (flux-science)
    let ln2 = std::f64::consts::LN_2;
    let landauer_bit = BOLTZMANN * t_room * ln2;
    let floor_bits = fingerprint_units * digest_bits;
    let e_floor = floor_bits * landauer_bit;
    let ratio_before = e_noop_before / e_floor;
    let ratio_after = e_noop_after / e_floor;
    let coordination_share = 100.0 * (1.0 - 1.0 / ratio_after);
    let doublings_left = ratio_after.log2();

    // ══════════════════ PORTFOLIO ROLL-UP
    let portfolio_value = usd_dev_year + ev_delta_year + value_d3 + value_d4;
    let portfolio_cost = d1_cost + d2_cost + d3_cost + d4_cost;
    let l_portfolio = portfolio_value / portfolio_cost;
    let portfolio_hours = depinfo_hours + probe_hours_year + atlas_hours + provenance_hours_year;
    let portfolio_payback_days = portfolio_cost / (portfolio_value / 365.0);

    // team-scaled latency table
    let mut team_rows = String::new();
    for t in team_sizes {
        team_rows.push_str(&format!(
            "{} & {} & {} & {:.1} \\\\\n",
            thou(t),
            thou(hours_dev_year * t),
            usd(usd_dev_year * t),
            fte_equiv * t
        ));
    }
    // build-frequency sensitivity
    let mut sens_rows = String::new();
    for b in builds_sensitivity {
        let d_s = b * dt + (b * switch_prob) * switch_cost_s;
        let h = d_s / 3600.0 * workdays;
        sens_rows.push_str(&format!(
            "{} & {:.2} & {} & {} \\\\\n",
            b.round() as i64,
            d_s / 3600.0,
            thou(h),
            usd(h * dev_cost_hour)
        ));
    }
    // dividend roll-up table
    let dividend_rows = format!(
        "D1 --- build latency & {} & {} & {:.0}$\\times$ & {:.1} \\\\\n\
         D2 --- verification yield & {} & {} & {:.0}$\\times$ & {:.1} \\\\\n\
         D3 --- failure taxonomy & {} & {} & {:.0}$\\times$ & --- \\\\\n\
         D4 --- provenance/audit & {} & {} & {:.0}$\\times$ & --- \\\\\n\
         \\midrule\n\
         \\textbf{{Portfolio}} & \\textbf{{{}}} & \\textbf{{{}}} & \\textbf{{{:.0}$\\times$}} & \\textbf{{{:.1}}} \\\\\n",
        usd(usd_dev_year),
        usd(d1_cost),
        l_d1,
        d1_payback_days,
        usd(ev_delta_year),
        usd(d2_cost),
        l_d2,
        d2_payback_days,
        usd(value_d3),
        usd(d3_cost),
        l_d3,
        usd(value_d4),
        usd(d4_cost),
        l_d4,
        usd(portfolio_value),
        usd(portfolio_cost),
        l_portfolio,
        portfolio_payback_days
    );

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
            "\\newcommand{\\stM}{\\textcolor{sigilcyan}{$\\bullet$}~{\\scriptsize\\textsc{measured}}}\n",
            "\\newcommand{\\stD}{\\textcolor{sigilviolet}{$\\oplus$}~{\\scriptsize\\textsc{derived}}}\n",
            "\\newcommand{\\stA}{\\textcolor{sigilamber}{$\\triangle$}~{\\scriptsize\\textsc{assumed}}}\n",
            "\\newcommand{\\kpi}[3]{\\begin{minipage}[t]{0.31\\textwidth}\\begin{tcolorbox}[colback=panelbg,",
            "colframe=slate,boxrule=0.5pt,arc=2pt,halign=center]{\\large\\bfseries\\textcolor{#1}{#2}}\\\\[2pt]",
            "{\\scriptsize #3}\\end{tcolorbox}\\end{minipage}}\n",
            "\\newcommand{\\brief}[3]{\\begin{tcolorbox}[colback=panelbg,colframe=#1,boxrule=0.8pt,arc=2pt,",
            "title=\\textbf{#2},coltitle=white,colbacktitle=#1]#3\\end{tcolorbox}}\n",
            "\\hypersetup{pdftitle={The Legibility Dividend},pdfsubject={An executive model of verifiable ",
            "software production},pdfkeywords={software economics, verification, provenance, build systems, ",
            "measurement, agentic engineering}}\n",
            "\\title{\\textbf{The Legibility Dividend}\\\\[6pt]\\large An Executive Model of Verifiable Software ",
            "Production\\\\[4pt]\\normalsize\\itshape Measured evidence from an instrumented, agent-operated ",
            "engineering system}\n",
            "\\author{The Flux Foundation\\\\\\small model computed by \\texttt{flux-arxiv-latex}, ",
            "thermodynamic floor by \\texttt{flux-science},\\\\\\small related work drawn from a live arXiv sweep}\n",
            "\\date{\\today}"
        ))
        .add(Block::Raw("\\maketitle".into()));

    // ── KPI strip + abstract
    doc = doc
        .add(Block::Raw(format!(
            "\\vspace{{-12pt}}\\noindent\n\\kpi{{sigilcyan}}{{{:.0}$\\times$}}{{\\textsc{{measured}} --- inner-loop \
             latency collapse ({:.1}\\,s $\\to$ {:.1}\\,s)}}\\hfill\n\
             \\kpi{{sigilviolet}}{{{}}}{{\\textsc{{derived}} --- recovered engineer-cost per head, per year}}\\hfill\n\
             \\kpi{{sigilamber}}{{{:.0}$\\times$}}{{\\textsc{{derived}} --- return on the four-part measurement \
             portfolio}}\n\n\\vspace{{6pt}}\n",
            speedup_noop, t_noop_before, t_noop_after, usd(usd_dev_year), l_portfolio
        )))
        .add(Block::Raw(format!(
            "\\begin{{abstract}}\\noindent\nSoftware organisations are asked to fund verification --- tests, \
             provenance, measurement, post-mortems --- out of a budget that rewards features. The usual defence \
             is moral (``quality matters''), and it loses. This paper makes the financial case instead, and it \
             makes it from a real instrumented record rather than a survey: a small, fully measured engineering \
             system whose build system, test instruments, incident taxonomy and artifact provenance were all \
             quantified over three months. Every figure below is \\emph{{computed at document-generation time}} \
             from those measurements plus explicitly declared cost assumptions; nothing is quoted from a vendor \
             deck. Four results. (1) A single build-system defect --- a fingerprint file that cache-restored \
             units never wrote --- inflated ``no-op'' rebuilds to \\measured{{{:.1}\\,s}}; repairing it \
             ({:.0}$\\times$) is worth \\derivedv{{{}}} per engineer per year at the stated rates, paying back in \
             \\derivedv{{{:.1}}} working days. (2) A test suite is an information channel, and one measured suite \
             carried \\measured{{{:.0}}} passing tests while an independent probe found \\measured{{{:.1}\\%}} of \
             reads returning data --- a \\emph{{Verification Information Yield}} of \\derivedv{{{:.4}\\,bits}}, i.e.\\ \
             a green build that meant nothing. Restoring an independent correctness probe buys \
             \\derivedv{{{:.2}\\,bits}} at \\derivedv{{{}}} of avoided expected loss per bit. (3) Seventy-one real \
             incidents cluster into ten classes; three classes cover \\measured{{{:.1}\\%}} of them, which turns a \
             qualitative ``improve quality'' mandate into a ranked budget. (4) Against the Landauer floor, the \
             work of deciding whether anything changed costs \\derivedv{{${}$}}$\\times$ the thermodynamic minimum \
             even \\emph{{after}} the repair: essentially all engineering cost is coordination, not physics, and \
             the savings pool is nowhere near exhausted. We close with a decision rule (the Legibility Ratio), a \
             90-day program, and an explicit list of what remains assumed rather than measured.\n\\end{{abstract}}\n",
            t_noop_before,
            speedup_noop,
            usd(usd_dev_year),
            d1_payback_days,
            suite_green,
            probe_read_success * 100.0,
            viy_before,
            viy_after,
            usd(usd_per_bit),
            top3_share,
            sci(ratio_after)
        )));

    // ── §1 Executive summary
    doc = doc
        .add(Block::Section("Executive summary".into()))
        .add(raw(
            "This section is the whole paper for a reader with four minutes. Each finding names its evidence \
             grade: \\stM{} means taken from an instrument on a real system, \\stD{} means computed in this \
             document from measured inputs, \\stA{} means a declared assumption a reader may replace.",
        ))
        .add(Block::Raw(format!(
            "\\brief{{sigilblue}}{{Findings}}{{\\begin{{enumerate}}[leftmargin=1.5em,itemsep=3pt]\n\
             \\item \\textbf{{The inner loop was the largest single cost centre, and nobody had priced it.}} \
             \\stM{{}} An unchanged crate took {:.1}\\,s to confirm it was unchanged; the heaviest binary took \
             {:.0}\\,s across {:.0} dirty units. Root cause: {:.0} poisoned fingerprint roots cascading into \
             {:.0} downstream units. After repair: {:.1}\\,s no-op, {:.1}\\,s for a three-crate edit, {:.0}\\% \
             unit cache hit rate.\n\
             \\item \\textbf{{A green test suite can carry zero information.}} \\stM{{}} {:.0} passing tests \
             coexisted with {:.1}\\% of storage reads returning data. \\stD{{}} Verification Information Yield \
             $=$ {:.4}\\,bits. A suite that passes whether or not the system works is not a control; it is a \
             cost.\n\
             \\item \\textbf{{Fast components beside slow systems are bug reports, not victories.}} \\stM{{}} A \
             component measured {:.0}$\\times$ faster than the live system it served --- the gap lay in code \
             nobody had benchmarked.\n\
             \\item \\textbf{{Failures are not random; they are a short list.}} \\stM{{}} {:.0} incidents, ten \
             classes, top three $=$ {:.1}\\%, top five $=$ {:.1}\\%. {:.0} of the {:.0} were the measuring \
             instrument itself being wrong.\n\
             \\item \\textbf{{Provenance is an audit asset with a computable yield.}} \\stM{{}} A constant \
             {:.0}-byte proof verifies a whole history in {:.0}\\,ms, and an incremental check costs \
             {:.0}\\,$\\mu$s --- roughly {} verifications per dollar of machine time.\n\
             \\item \\textbf{{Physics is not the constraint.}} \\stD{{}} Post-repair, answering ``did anything \
             change?'' still costs ${}\\times$ the Landauer floor; {:.1}\\% of the cost is coordination. There \
             are {:.0} further doublings of efficiency available before thermodynamics objects.\n\
             \\end{{enumerate}}}}\n",
            t_noop_before, t_fat_before, 166.0, poisoned_roots, poisoned_downstream,
            t_noop_after, t_incremental_after, cache_unit_hit * 100.0,
            suite_green, probe_read_success * 100.0, viy_before,
            rule0_gap,
            incidents_total, top3_share, top5_share, measurement_class, incidents_total,
            proof_bytes, verify_ms_full, verify_us_tip, thou(checks_per_dollar),
            sci(ratio_after), coordination_share, doublings_left
        )))
        .add(Block::Raw(format!(
            "\\brief{{sigilgreen}}{{Three decisions this paper supports}}{{\\begin{{enumerate}}[leftmargin=1.5em,itemsep=3pt]\n\
             \\item \\textbf{{Fund the inner loop before the feature list.}} At the stated rates the latency \
             repair returns {:.0}$\\times$ its cost and pays back in {:.1}\\,working days --- the highest-yield \
             line item in the portfolio, and the one no roadmap contains.\n\
             \\item \\textbf{{Require an independent correctness probe beside every performance or health \
             claim.}} Cost ceiling: {:.0} engineer-hours per release still breaks even ({} of avoided expected \
             loss per release at the stated escape cost).\n\
             \\item \\textbf{{Make evidence grades a reporting standard.}} Every number in a status report \
             carries \\textsc{{measured}}, \\textsc{{derived}} or \\textsc{{assumed}}. This is free, and it is \
             the control that would have caught {:.0}\\% of the incident record ($=$ the measurement and \
             stale-state classes).\n\
             \\end{{enumerate}}}}\n",
            l_d1, d1_payback_days, breakeven_hours_release, usd(ev_delta_release), two_guard_share
        )));

    // ── §2 the business problem
    doc = doc
        .add(Block::Section("Why legibility is a financial instrument".into()))
        .add(para(format!(
            "The literature on software cost is unambiguous that poor quality is expensive at civilisational \
             scale \\textcite{{arxiv2506_13821}}, and equally unambiguous that the mechanisms which would reduce \
             it are under-adopted. Two examples frame this paper. First, build performance: a large-scale study \
             of {} builds across {} projects found that only {}\\% adopt caching at all \
             \\textcite{{arxiv2601_19146}}, and a case study of a flagship infrastructure project documents the \
             cost of \\emph{{downgrading}} a build system as a deliberate trade \\textcite{{arxiv2510_20041}}. \
             Second, verification signal: an industrial study of a major database system reports flaky tests \
             giving ``an ambiguous signal about the quality of the code'' and interfering with automated \
             assessment \\textcite{{arxiv2602_03556}}, a finding replicated across languages and resource \
             configurations \\textcite{{arxiv2208_14799,arxiv2310_12132}}.",
            "513{,}384", "1{,}279", "30"
        )))
        .add(raw(
            "Both are usually discussed as engineering hygiene. They are better understood as \\emph{unpriced \
             liabilities}. The reason they stay unpriced is structural: an organisation cannot put a number on a \
             cost it does not measure, and the measurement itself competes for the same budget. That circularity \
             --- you must spend to learn what spending would save --- is precisely what a business case has to \
             break. Technical-debt research has reached the same conclusion from the other direction: \
             prioritisation fails when technical and business framings are misaligned \\textcite{arxiv1908_01347}, \
             management tooling stays unadopted because its time and cost are perceived as high \
             \\textcite{arxiv2502_03153}, and the effort penalty of unpaid debt is measurable when anyone \
             bothers to measure it \\textcite{arxiv2502_16277}.",
        ))
        .add(raw(
            "This paper contributes the missing artifact: a worked, reproducible valuation from a system where \
             the measurements actually exist. The system is small and unusual --- one operator, a fleet of \
             automated agents, a public chain and its build orchestrator --- and \\S\\ref{sec:validity} is \
             explicit about what that costs in generalisability. What it buys, in exchange, is a record where \
             every claim was already graded before this paper existed.",
        ));

    // ── §3 the instrument
    doc = doc
        .add(Block::Raw("\\section{The instrument: evidence grades as a management control}\\label{sec:instrument}\n\n".into()))
        .add(raw(
            "The engineering record this paper draws on carries two standing rules. They are cheap, they are \
             unusual, and they are the reason the numbers below are worth anything.",
        ))
        .add(raw(
            "\\textbf{Rule 0 --- a fast component beside a slow system is a bug report, not a victory.} Every \
             claim states which rung of the ladder it sits on: microbenchmark, in-process harness, or live \
             system. The canonical violation in the record is a store measured at ten million records per second \
             while the live system it served ran at eight hundred blocks per second --- four orders of magnitude \
             apart, with the true bottleneck in a component nobody had benchmarked.",
        ))
        .add(raw(
            "\\textbf{Rule 1 --- grade the claim, not the claimant.} Each number carries an evidence grade. This \
             paper uses three: \\textsc{measured} (an instrument on a real system), \\textsc{derived} (computed \
             here from measured inputs and declared assumptions), \\textsc{assumed} (a stated input a reader may \
             replace). The full record uses seven, adding simulated, staged, live and conjectural.",
        ))
        .add(Block::Raw(format!(
            "The reason a board should care is that the record also contains a chapter on \\emph{{instruments \
             that lied}} --- four documented cases where the measuring tool reported success without doing the \
             work: a combined test tool reporting ``0 passed, 0 failed'' when the test binary failed to compile \
             (readable as ``no failures''); a stale-binary selection that measured three-week-old code; silently \
             dropped test filters, so an expensive benchmark that appeared to run in 0\\,s had not run; and a \
             cache reporting its own hit rate as victory while an independent probe measured \
             \\measured{{{:.1}\\%}} of reads returning data. Seven of the {:.0} recorded incidents \
             ({:.1}\\%) are of this kind. \\emph{{An organisation's dashboard is part of its attack surface.}}\n\n",
            probe_read_success * 100.0, incidents_total, instrument_share
        )))
        .add(Block::Raw(format!(
            "\\brief{{sigilviolet}}{{The one-page reporting standard}}{{Every quantitative line in a status \
             report or board pack carries: (a) the number; (b) its evidence grade; (c) the instrument that \
             produced it; (d) the independent check that would falsify it. Cost: zero incremental spend, one \
             column. Expected effect on this record: the measurement and stale-state classes together are \
             {:.0}\\% of all incidents ({:.0} of {:.0}).}}\n\n",
            two_guard_share,
            measurement_class + stale_state_class,
            incidents_total
        )));

    // ── §4 Dividend 1
    doc = doc
        .add(Block::Raw("\\section{Dividend 1 --- the inner loop is a wage bill}\\label{sec:d1}\n\n".into()))
        .add(Block::Raw(format!(
            "\\textbf{{The measurement.}} \\stM{{}} Builds that should have been no-ops were not. Confirming that \
             an unchanged crate was unchanged cost {:.1}\\,s; the heaviest binary cost {:.0}\\,s across 166 \
             units that were re-declared dirty on every invocation. The cause was not compilation but \
             bookkeeping: units restored from the content cache skipped the compiler, so the fingerprint \
             dependency file was never written, and the build tool marked {:.0} roots permanently stale, \
             cascading into {:.0} downstream units. After the repair --- capture, require and materialise that \
             file on cache restore --- the same no-op measured {:.1}\\,s (heal run {:.2}\\,s), a three-crate \
             incremental edit including the heavy binary measured {:.1}\\,s, and the unit-level cache hit rate \
             settled at {:.0}\\%. Two long-held beliefs died in the same measurement: that a compiler-wrapper \
             environment variable was fragmenting build identity, and that the shared cache was empty (a \
             symlink that a disk-usage probe reported as zero, with 17\\,GB behind it).\n\n",
            t_noop_before, t_fat_before, poisoned_roots, poisoned_downstream,
            t_noop_after, 0.56, t_incremental_after, cache_unit_hit * 100.0
        )))
        .add(Block::Raw(format!(
            "\\textbf{{The model.}} \\stA{{}} An engineer costs {} per hour fully loaded and works {:.0} days a \
             year. \\stA{{}} They invoke the inner loop {:.0} times a day. \\stA{{}} A wait longer than \
             {:.0}\\,s triggers a task switch with probability {:.2}, and each switch costs {:.0}\\,s of \
             recovery. \\stD{{}} Direct saving: {:.0} invocations $\\times$ {:.1}\\,s $=$ {:.2}\\,h/day. Switch \
             saving: {:.2}\\,h/day. Total {:.2}\\,h/day $=$ {}\\,h/engineer/year $=$ {} $=$ {:.2} recovered \
             full-time equivalents per engineer.\n\n",
            usd(dev_cost_hour), workdays, builds_day, switch_threshold_s, switch_prob, switch_cost_s,
            builds_day, dt, direct_s_day / 3600.0, switch_s_day / 3600.0, hours_day,
            thou(hours_dev_year), usd(usd_dev_year), fte_equiv
        )))
        .add(Block::Raw(format!(
            "\\begin{{center}}\\small\\rowcolors{{2}}{{rowtint}}{{white}}\
             \\begin{{tabular}}{{lrrr}}\\toprule\n\
             Engineers & Hours/year recovered & Cost recovered & FTE equivalent \\\\\\midrule\n{}\
             \\bottomrule\\end{{tabular}}\\end{{center}}\n\n\
             \\noindent The linearity is an assumption, not a measurement: at large team sizes shared build \
             infrastructure contends, which is exactly the trade the Kubernetes build-system study documents \
             \\textcite{{arxiv2510_20041}}, and cache adoption is itself the exception rather than the rule \
             \\textcite{{arxiv2601_19146}}. Read the {:.0}-engineer row as an upper bound on a real \
             organisation.\n\n",
            team_rows, 1000.0
        )))
        .add(Block::Raw(format!(
            "\\textbf{{Sensitivity.}} The single most contestable input is invocation frequency. \\stD{{}}\n\n\
             \\begin{{center}}\\small\\rowcolors{{2}}{{rowtint}}{{white}}\
             \\begin{{tabular}}{{rrrr}}\\toprule\n\
             Invocations/day & Hours/day & Hours/year & Cost/year (1 engineer) \\\\\\midrule\n{}\
             \\bottomrule\\end{{tabular}}\\end{{center}}\n\n\
             \\noindent Even the conservative row --- ten invocations a day --- recovers more than the cost of \
             the investigation that found the defect ({:.0} engineer-hours, {}). \\stD{{}} Payback: {:.1} working \
             days. Legibility Ratio: {:.0}$\\times$. The machine-energy side is real but small at this scale: \
             {:.0}\\,kWh and {} per engineer per year.\n\n",
            sens_rows, depinfo_hours, usd(d1_cost), d1_payback_days, l_d1,
            kwh_saved_dev_year, usd(energy_usd_dev_year)
        )));

    // ── §5 Dividend 2 — VIY
    doc = doc
        .add(Block::Raw("\\section{Dividend 2 --- Verification Information Yield}\\label{sec:d2}\n\n".into()))
        .add(raw(
            "The second dividend needs a new instrument, because the industry standard one --- ``the suite is \
             green'' --- is not a measurement. It is a claim about a claim.",
        ))
        .add(Block::Raw(format!(
            "\\textbf{{The measurement.}} \\stM{{}} A storage layer in this record passed {:.0} of {:.0} unit \
             tests for its entire service life while destroying data at scale. An independent probe --- one that \
             asserted the value came back rather than timing the call --- measured {:.1}\\% of reads returning \
             data. The suite was not lying about its own execution; it was answering a different question than \
             anyone believed.\n\n",
            suite_green, suite_green, probe_read_success * 100.0
        )))
        .add(raw(
            "\\textbf{The instrument.} Treat a test run as a communication channel. Let $D$ be the event that a \
             release contains a defect of the class in question and $G$ the event that the suite reports green. \
             The suite's value is the mutual information",
        ))
        .add(raw(
            "\\[ \\mathrm{VIY} \\;=\\; I(D;G) \\;=\\; \\sum_{d\\in\\{0,1\\}}\\sum_{g\\in\\{0,1\\}} \
             P(d,g)\\,\\log_2\\frac{P(d,g)}{P(d)P(g)} \\quad\\text{bits per run.} \\]",
        ))
        .add(Block::Raw(format!(
            "\\stA{{}} With $P(D)={:.2}$ per release, the measured false-green case has \
             $P(G\\,|\\,D)={:.2}$ --- the suite passes even when the defect is present --- and \
             $P(G\\,|\\,\\lnot D)={:.2}$ on a clean tree. \\stD{{}} Then $\\mathrm{{VIY}}={:.4}$\\,bits --- over two orders of magnitude below what a working probe yields, and \
             indistinguishable from zero (the residual is an artifact of the assumed clean-tree flake rate, \
             which perversely makes green \\emph{{slightly more}} likely when the defect is present). The \
             green build carried \\emph{{no usable information}} about the property anyone cared about. Adding an \
             independent correctness probe that catches the defect class with $P(G\\,|\\,D)={:.2}$ lifts the \
             yield to \\derivedv{{{:.4}\\,bits}} per run.\n\n",
            p_defect_release, p_green_given_defect_before, p_green_given_clean, viy_before,
            p_green_given_defect_after, viy_after
        )))
        .add(Block::Raw(format!(
            "\\textbf{{The money.}} \\stA{{}} An escaped defect of this class costs {}; there are {:.0} releases \
             a year. \\stD{{}} Expected loss before: $P(D)\\times P(G|D)\\times C = {}$ per release. After: {}. \
             The probe therefore buys {} per release, {} per year, at a cost of {:.0} engineer-hours per year \
             ({}). Legibility Ratio {:.0}$\\times$; payback {:.1} days. Two framings a board can act on:\n\n\
             \\begin{{itemize}}[leftmargin=1.4em,itemsep=2pt]\n\
             \\item \\textbf{{Break-even budget:}} up to \\derivedv{{{:.0} engineer-hours per release}} may be \
             spent on independent verification of this one property before it stops paying.\n\
             \\item \\textbf{{Price per bit:}} the first {:.3}\\,bits of verification information are worth \
             \\derivedv{{{}}} each. Information, not effort, is the unit that prices correctly.\n\
             \\end{{itemize}}\n\n",
            usd(c_escape), releases_year, usd(ev_loss_before), usd(ev_loss_after),
            usd(ev_delta_release), usd(ev_delta_year), probe_hours_year, usd(d2_cost),
            l_d2, d2_payback_days, breakeven_hours_release, viy_after, usd(usd_per_bit)
        )))
        .add(raw(
            "The flaky-test literature is the same problem seen from the noise side: non-deterministic tests \
             degrade the channel until teams stop trusting it \\textcite{arxiv2208_14799}, industrial studies \
             tie the ambiguity directly to slowed release assessment \\textcite{arxiv2602_03556}, resource \
             contention alone moves failure rates \\textcite{arxiv2310_12132}, reproduction is hard enough to \
             need dedicated datasets \\textcite{arxiv2605_21677}, and repair is now an automation target at \
             industrial scale \\textcite{arxiv2511_14002}. VIY makes the shared quantity explicit: flakiness and \
             false-green are both \\emph{information losses}, and both should be funded against the same number.",
        ));

    // ── §6 Dividend 3
    doc = doc
        .add(Block::Raw("\\section{Dividend 3 --- the failure taxonomy as a budget allocator}\\label{sec:d3}\n\n".into()))
        .add(Block::Raw(format!(
            "\\textbf{{The measurement.}} \\stM{{}} {:.0} distinct failures from one system's own record, each \
             carrying its file, its number and the height at which it was found; severity {:.0} catastrophic, \
             {:.0} major, {:.0} moderate. They fall into ten classes, and the distribution is steep.\n\n\
             \\begin{{center}}\\small\\rowcolors{{2}}{{rowtint}}{{white}}\
             \\begin{{tabularx}}{{\\textwidth}}{{@{{}}Xrrr@{{}}}}\\toprule\n\
             Class & \\# & Share & Cumulative \\\\\\midrule\n{}\
             \\bottomrule\\end{{tabularx}}\\end{{center}}\n\n",
            incidents_total, sev_catastrophic, sev_major, sev_moderate, pareto_rows
        )))
        .add(Block::Raw(format!(
            "\\textbf{{Why an executive should read a failure table.}} \\stD{{}} Three classes cover \
             {:.1}\\% of all incidents and five cover {:.1}\\%. That converts an unfundable mandate \
             (``improve quality'') into a ranked allocation with named owners. {:.1}\\% of the record was \
             catastrophic --- funds, consensus or fleet at stake --- so the tail is not academic. And the \
             single most uncomfortable row is \\emph{{measurement}} at {:.0} incidents ({:.1}\\%): one \
             failure in ten was the organisation's own instrument.\n\n\
             \\stA{{}} Credit the taxonomy with preventing just \\emph{{one}} catastrophic repeat every three \
             years --- {:.2} per year, against a record that already contains fifteen --- and at {} per \
             catastrophic escape it is worth \\derivedv{{{}}} a year against an authoring cost of {:.0} \
             engineer-hours ({}): a Legibility Ratio of {:.0}$\\times$. The assumption is the rate; the \
             measurement is the distribution. A reader who believes only half the effect still sees \
             {:.0}$\\times$.\n\n",
            top3_share, top5_share, cat_share, measurement_class, instrument_share,
            cat_repeats_avoided_year, usd(c_escape), usd(value_d3), atlas_hours, usd(d3_cost),
            l_d3, l_d3 / 2.0
        )))
        .add(raw(
            "The mechanism is not moral either. A public taxonomy makes a class of failure \\emph{recognisable \
             before it is catastrophic}, and in this record the majority of classes produced a structural guard \
             --- an invariant, a refusal, a changed default --- rather than a patch. That is the conversion worth \
             funding: incidents into shape.",
        ));

    // ── §7 Dividend 4
    doc = doc
        .add(Block::Raw("\\section{Dividend 4 --- provenance as an audit asset}\\label{sec:d4}\n\n".into()))
        .add(Block::Raw(format!(
            "\\textbf{{The measurement.}} \\stM{{}} Artifacts in this system carry signed provenance, and the \
             system's history carries a constant-size proof: {:.0}\\,bytes regardless of chain length, full \
             verification in {:.0}\\,ms at one hundred thousand blocks, and an incremental check against an \
             already-verified state in {:.0}\\,$\\mu$s with zero data downloaded. \\stD{{}} At the stated \
             machine-time proxy that is roughly {} full verifications per dollar.\n\n",
            proof_bytes, verify_ms_full, verify_us_tip, thou(checks_per_dollar)
        )))
        .add(raw(
            "\\textbf{Why this is a business asset and not a security hobby.} The supply-chain literature is \
             clear that provenance frameworks work and equally clear that adoption stalls on cost: a study of \
             SLSA deployment finds interest without uptake \\textcite{arxiv2409_05014}; an analysis of software \
             signing states plainly that it ``imposes tooling and operational costs to implement in practice'' \
             \\textcite{arxiv2510_04964}; the reproducible-build tradition demonstrates the end-to-end version \
             is achievable \\textcite{arxiv2206_14606}, and hardware-rooted variants push the trust boundary \
             further \\textcite{arxiv2106_09843}. Meanwhile the build pipeline itself is now a first-class \
             attack surface \\textcite{arxiv2601_08995}. The adoption gap is therefore a \\emph{pricing} gap, \
             and pricing is a board's job.",
        ))
        .add(Block::Raw(format!(
            "\\stA{{}} Signed artifacts plus written commitment records displace {:.0} external audit hours a \
             year at {} per hour, and address half of an annual supply-chain incident risk of {:.0}\\% at {}. \
             \\stD{{}} Value {} against {:.0} engineer-hours ({}); Legibility Ratio {:.0}$\\times$. The \
             defensible core of that claim is the cheap-verification measurement: when a check costs \
             {:.0}\\,$\\mu$s, ``verify continuously'' stops being a slogan and becomes a line item that rounds \
             to zero.\n\n",
            audit_hours_saved, usd(auditor_cost_hour), p_supply_chain * 100.0, usd(c_supply_chain),
            usd(value_d4), provenance_hours_year, usd(d4_cost), l_d4, verify_us_tip
        )));

    // ── §8 physical floor
    doc = doc
        .add(Block::Raw("\\section{The floor: how much of engineering cost is physics?}\\label{sec:floor}\n\n".into()))
        .add(Block::Raw(format!(
            "One number keeps executives honest about how much headroom remains. Erasing a bit at temperature \
             $T$ costs at least $k_BT\\ln 2$; at $T={:.0}$\\,K that is ${}$\\,J. Deciding whether a workspace of \
             {:.0} units has changed requires, at minimum, comparing one digest per unit --- ${:.0}$\\,bits --- \
             so thermodynamics prices the entire question at $E_{{\\min}}={}$\\,J.\n\n",
            t_room, sci(landauer_bit), fingerprint_units, floor_bits, sci(e_floor)
        )))
        .add(Block::Raw(format!(
            "\\stD{{}} Before the repair, that question consumed {:.1}\\,s on a {:.0}-core machine drawing \
             {:.0}\\,W $=$ ${}$\\,J: a factor of ${}$ above the floor. After the repair it consumes ${}$\\,J --- \
             still ${}$ above the floor, i.e.\\ {:.4}\\% of the energy is coordination overhead and \
             {:.0} further doublings of efficiency remain physically available. \\emph{{The constraint is never \
             the universe; it is agreement.}} That is the strategic point of the whole paper: a cost that is \
             {:.4}\\% organisational is a cost that responds to organisational instruments --- measurement, \
             grading, provenance --- and not to hardware procurement.\n\n",
            t_noop_before, cores, box_watts, sci(e_noop_before), sci(ratio_before),
            sci(e_noop_after), sci(ratio_after), coordination_share, doublings_left,
            coordination_share
        )))
        .add(raw(
            "The floor itself is not folklore: experimental work approaches $k_BT\\ln2$ in real devices \
             \\textcite{arxiv1507_07450}, and adiabatic logic families are explicitly engineered against it \
             \\textcite{arxiv2504_04284}. We cite them to fix the denominator, not to suggest anyone should \
             build a build system out of superconductors.",
        ));

    // ── §9 the decision rule
    doc = doc
        .add(Block::Raw("\\section{The decision rule: the Legibility Ratio}\\label{sec:rule}\n\n".into()))
        .add(raw(
            "Collecting the four dividends gives a single, auditable decision rule. For any proposed act of \
             measurement $m$ --- an instrument, a probe, a taxonomy, a signature --- define",
        ))
        .add(raw(
            "\\[ \\mathcal{L}(m) \\;=\\; \\frac{V_{\\text{avoided}}(m)}{C_{\\text{measure}}(m)}, \\qquad \
             \\text{fund } m \\iff \\mathcal{L}(m) > 1 \\text{ and the inputs to } V \\text{ are graded.} \\]",
        ))
        .add(Block::Raw(format!(
            "\\begin{{center}}\\small\\rowcolors{{2}}{{rowtint}}{{white}}\
             \\begin{{tabular}}{{lrrrr}}\\toprule\n\
             Dividend & Annual value & Annual cost & $\\mathcal{{L}}$ & Payback (days) \\\\\\midrule\n{}\
             \\bottomrule\\end{{tabular}}\\end{{center}}\n\n\
             \\noindent \\stD{{}} The portfolio is {:.0} engineer-hours ({}) returning {} --- \
             {:.0}$\\times$, payback {:.1}\\,days. The second column is the honest one to attack: three of the \
             four values contain an assumed severity or probability, and \\S\\ref{{sec:validity}} lists each. \
             The first column does not: those are instrument readings.\n\n",
            dividend_rows, portfolio_hours, usd(portfolio_cost), usd(portfolio_value),
            l_portfolio, portfolio_payback_days
        )))
        .add(Block::Raw(format!(
            "\\brief{{sigilamber}}{{The rule stated for a sceptic}}{{Halve every value in the table and double \
             every cost. The portfolio still returns {:.0}$\\times$. A conclusion that survives a factor of four \
             against itself is a decision, not a forecast.}}\n\n",
            l_portfolio / 4.0
        )));

    // ── §10 90-day program
    doc = doc
        .add(Block::Raw("\\section{A 90-day program, with measurement gates}\\label{sec:program}\n\n".into()))
        .add(raw(
            "Each item names what would prove it worked. An item without a falsifiable gate is not in the \
             program.",
        ))
        .add(Block::Raw(format!(
            "\\begin{{center}}\\small\\rowcolors{{2}}{{rowtint}}{{white}}\
             \\begin{{tabularx}}{{\\textwidth}}{{@{{}}lXl@{{}}}}\\toprule\n\
             Window & Action and measurement gate & Grade at start \\\\\\midrule\n\
             Days 1--15 & \\textbf{{Instrument the inner loop.}} Record the latency distribution of the \
             no-change build across the fleet. Gate: a published histogram, and the 90th percentile named. If \
             the 90th percentile exceeds {:.0}\\,s, the defect class in \\S\\ref{{sec:d1}} is present. & \\stA \\\\\n\
             Days 1--30 & \\textbf{{Pair every health claim with an independent probe.}} For each dashboard \
             that reports its own success, add one external assertion of the property. Gate: VIY $>0$ \
             demonstrated by a deliberately broken build that the suite must fail. & \\stA \\\\\n\
             Days 15--45 & \\textbf{{Adopt evidence grades in reporting.}} One extra column in every status \
             number. Gate: a board pack in which every figure carries a grade and an instrument. & \\stA \\\\\n\
             Days 30--60 & \\textbf{{Write the taxonomy.}} Classify the last two years of incidents; publish \
             internally. Gate: three classes covering $\\geq${:.0}\\% of incidents, each with a named owner and \
             one structural guard shipped. & \\stA \\\\\n\
             Days 45--90 & \\textbf{{Sign the artifacts.}} Provenance on every released binary plus a written \
             commitment record for every external promise. Gate: an auditor reproduces one release end-to-end \
             without asking the team a question. & \\stA \\\\\n\
             \\bottomrule\\end{{tabularx}}\\end{{center}}\n\n",
            switch_threshold_s, top3_share.floor()
        )))
        .add(raw(
            "The ordering is deliberate: latency first because it funds the rest, probes second because they \
             protect every subsequent measurement, grading third because it is free, taxonomy fourth because it \
             needs the previous three to be trustworthy, provenance last because it is the only item whose value \
             is realised outside the organisation.",
        ));

    // ── §11 validity
    doc = doc
        .add(Block::Raw("\\section{Threats to validity, and what is still assumed}\\label{sec:validity}\n\n".into()))
        .add(raw(
            "This paper's own standard forbids ending on the strong numbers. The following are the reasons a \
             careful reader should discount them, stated before anyone else has to find them.",
        ))
        .add(Block::Raw(format!(
            "\\begin{{enumerate}}[leftmargin=1.5em,itemsep=3pt]\n\
             \\item \\textbf{{$N=1$, and the one is unusual.}} The measured system is a single engineering \
             household: one operator, a fleet of automated agents on shared infrastructure, one public chain and \
             its build orchestrator. It is not a sample of industry. Its incident record is unusually complete \
             \\emph{{because}} it is small. The distribution of failure classes should be re-measured, not \
             imported.\n\
             \\item \\textbf{{Every dollar is derived, none is invoiced.}} No figure in this paper came from an \
             accounting system. Labour is priced at an assumed {} per hour, escapes at an assumed {}, \
             supply-chain incidents at an assumed {} with an assumed {:.0}\\% annual probability. Replace them; \
             the model is a function, not a result.\n\
             \\item \\textbf{{Linear team scaling is wrong at the top of the table.}} The {:.0}-engineer row \
             ignores build-infrastructure contention, which the literature documents as a real trade \
             \\textcite{{arxiv2510_20041}}, and assumes cache adoption that {}\\% of projects do not have \
             \\textcite{{arxiv2601_19146}}.\n\
             \\item \\textbf{{The latency win was a defect repair, not a technology.}} {:.1}\\,s $\\to$ \
             {:.1}\\,s is the removal of a specific pathology (unwritten fingerprint files on cache restore). \
             A team without that pathology should expect a far smaller number, and the honest residuals were \
             recorded with the win: a version bump legitimately mints new unit identities, so the first one or \
             two runs after a release are slow by construction.\n\
             \\item \\textbf{{VIY's probabilities are priors, not frequencies.}} $P(D)$ and \
             $P(G\\,|\\,D)$ after the probe are assumed. The \\emph{{before}} case is not: \
             $P(G\\,|\\,D)={:.2}$ was observed --- a full suite green against a live data-loss defect.\n\
             \\item \\textbf{{Agent-operated engineering is early.}} The record was produced by a human \
             operator directing automated agents, a regime whose behaviour is actively under study \
             \\textcite{{arxiv2506_14683,arxiv2506_18824,arxiv2510_25694}}, and whose internal work-logs are \
             trust-internal rather than third-party verifiable. Treat process claims accordingly; treat the \
             instrument readings as instrument readings.\n\
             \\item \\textbf{{What this paper does not claim.}} No claim that measurement substitutes for \
             engineering judgement; no claim that the four dividends compose additively in a large organisation \
             (they are reported separately for that reason); no claim of causality between publishing a taxonomy \
             and the subsequent incident rate --- that gate is stated in \\S\\ref{{sec:program}} precisely \
             because it has not yet been passed.\n\
             \\end{{enumerate}}\n\n",
            usd(dev_cost_hour), usd(c_escape), usd(c_supply_chain), p_supply_chain * 100.0,
            1000.0, "70", t_noop_before, t_noop_after, p_green_given_defect_before
        )));

    // ── related work (generated from the arXiv sweep; ragged so long auto-built
    //    lines cannot overrun the margin)
    if !papers.is_empty() {
        // The crate's default renderer emits one hard-broken line per paper; for a
        // 12-page executive document we re-flow the same content (identical gloss
        // rule: first sentence of the abstract, truncated and escaped) as a list.
        debug_assert!(related_work_section(&papers).starts_with("\\section{Related Work}"));
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

    // ── reproducibility + coda
    doc = doc
        .add(Block::Section("Reproducibility".into()))
        .add(Block::Raw(format!(
            "This document is the output of a Rust binary in the Flux tree \
             (\\texttt{{flux-arxiv-latex/}}\\allowbreak\\texttt{{src/bin/}}\\allowbreak\\texttt{{legibility\\_dividend.rs}}), built and run through the \
             \\texttt{{fluxc}} orchestrator rather than a hand-written \\texttt{{.tex}}. Every figure above is \
             computed at generation time from two declared sources: measured constants transcribed from the \
             engineering record (each cited in place with its instrument), and the business assumptions listed \
             in \\S\\ref{{sec:validity}}, which appear once, at the top of the program, as named variables. The \
             thermodynamic floor comes from \\texttt{{flux-science}}'s CODATA Boltzmann constant \
             ($k_B={}$\\,J/K); the bibliography and related-work section are generated from a live arXiv API \
             sweep parsed by the same crate. Change an assumption, recompile, and the paper corrects itself --- \
             including this sentence's neighbours. We consider that the only honest way to publish a business \
             case: a valuation you cannot re-derive is a forecast wearing a suit.\n\n",
            sci(BOLTZMANN)
        )))
        .add(Block::Section("Coda: legibility before scale".into()))
        .add(raw(
            "The temptation, having computed a large return, is to claim a large discovery. The record does not \
             support that and the discipline of this corpus forbids it. What was discovered is smaller and more \
             useful: that an engineering organisation which writes down what it measured, on what instrument, \
             and what would prove it wrong, ends up with a balance sheet where before it had opinions. The \
             dividends in this paper are not the reward for building something clever. They are the reward for \
             being \\emph{legible} to oneself --- and legibility, unlike scale, is available on Monday morning, \
             at the cost of one extra column.",
        ))
        .add(raw(
            "That is also why the uncomfortable rows were kept. A record in which one failure in ten is the \
             instrument's own fault is not a weak record; it is the only kind that can be trusted about the \
             other nine. An executive reading a report with no such row is not reading a better system. They are \
             reading a less honest instrument.",
        ));

    // ── sources: the measured record this paper prices
    doc = doc.add(Block::Raw(
        "\\section*{Sources --- the record this paper prices}\n\\label{sec:sources}\n\\small\n\
         \\begin{itemize}[leftmargin=1.2em,itemsep=1pt]\n\
         \\item \\emph{The Idle Machine: What the World Spends Rebuilding What It Already Built}, v0 \
         (2026-07-26) --- the sequel: the same measured kernel extrapolated to planetary scale in graded \
         rungs, with the climate headline deliberately deflated and the Jevons rebound applied to its own \
         result. \\url{https://quillon.xyz/downloads/FLUX_IDLE_MACHINE_v0.pdf}.\n\
         \\item \\emph{The Measurement Book: Laws of the SIGIL Graph --- the measured record}, v0 (2026-07-25) \
         --- the instrument readings used in \\S\\ref{sec:d1}, \\S\\ref{sec:d2} and \\S\\ref{sec:d4}, including \
         ARC-15 (the build-latency arc), Rule 0, and the calibration chapter on instruments that lied. \
         \\url{https://quillon.xyz/downloads/SIGIL_MEASUREMENT_BOOK_v0.pdf}.\n\
         \\item \\emph{The SIGIL Failure Atlas}, v0 (2026-07-23) --- the 71-incident, ten-class taxonomy and \
         severity split tabulated in \\S\\ref{sec:d3}. \
         \\url{https://quillon.xyz/downloads/SIGIL_FAILURE_ATLAS_v0.pdf}.\n\
         \\item \\emph{What the Chain Knows --- The Philosophy of the SIGIL Graph}, v0.3 (2026-07-22) --- the \
         evidence-grading discipline this paper repurposes as a management control in \\S\\ref{sec:instrument}. \
         \\url{https://quillon.xyz/downloads/SIGIL_PHILOSOPHY_v0.pdf}.\n\
         \\item \\emph{The Builder's Chronicle}, v0 (2026-07-23) --- development settled on a public ledger; \
         the labour-cost side of the same record. \
         \\url{https://quillon.xyz/downloads/SIGIL_BUILDERS_CHRONICLE_v0.pdf}.\n\
         \\item \\emph{The Adversary's Companion}, v0 (2026-07-24) --- the threat model behind \\S\\ref{sec:d4}'s \
         provenance valuation. \\url{https://quillon.xyz/downloads/SIGIL_ADVERSARYS_COMPANION_v0.pdf}.\n\
         \\item \\emph{SIGIL --- The Variational Chain}, whitepaper v1.3 (2026-07-22) --- the system whose \
         operation produced every measurement above. \\url{https://quillon.xyz/downloads/SIGIL_WHITEPAPER_v1.pdf}.\n\
         \\end{itemize}\n\n"
            .to_string(),
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
    std::fs::write(format!("{out_dir}/legibility_dividend.bib"), bibliography(&papers)).expect("bib");
    let res = doc.compile_pdf(out_dir, "SIGIL_LEGIBILITY_DIVIDEND_v0");
    if res.success {
        println!("OK {}", res.pdf_path.unwrap());
        println!(
            "model: speedup={:.0}x  usd/dev/yr={:.0}  VIY {:.2}->{:.3} bits  L_portfolio={:.0}x  floor_ratio={:.3e}",
            speedup_noop, usd_dev_year, viy_before, viy_after, l_portfolio, ratio_after
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
