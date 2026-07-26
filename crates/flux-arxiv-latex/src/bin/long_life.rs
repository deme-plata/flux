//! long_life — "The Arithmetic of Long Life": what physics and arithmetic can
//! decide about radical life extension, quantum simulation, and the governance
//! of long-lived power. Every figure computed at generation time (flux-science
//! constants + explicit, labelled model parameters); bibliography swept live
//! from arXiv and typeset by flux-arxiv-latex.
//!
//! Usage: long_life [arxiv.json] [out_dir]
use flux_arxiv_latex::doc::{Block, Document};
use flux_arxiv_latex::{bibliography, latex_escape, parse_arxiv_json, ArxivPaper};
use flux_science::constants::*;
use flux_science::holographic::HolographicTheory;

/// Format for math mode: plain when human-sized, \times10^{n} otherwise.
fn sci(x: f64) -> String {
    if x == 0.0 || !x.is_finite() {
        return format!("{x}");
    }
    let exp = x.abs().log10().floor() as i32;
    if (-2..=4).contains(&exp) {
        let s = format!("{:.3}", x);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        format!("{:.2}\\times10^{{{}}}", x / 10f64.powi(exp), exp)
    }
}

fn para(s: String) -> Block {
    Block::Raw(format!("{s}\n\n"))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let json_path = args
        .get(1)
        .map(String::as_str)
        .unwrap_or("crates/flux-arxiv-latex/long_life.arxiv.json");
    let out_dir = args.get(2).map(String::as_str).unwrap_or("/tmp/long-life");

    let papers: Vec<ArxivPaper> = std::fs::read_to_string(json_path)
        .ok()
        .and_then(|j| parse_arxiv_json(&j).ok())
        .unwrap_or_default();

    // ================================================================= MODEL
    // Every parameter below is an ASSUMPTION, stated in the text where used.
    let life_now = 80.0_f64; // reference human lifespan (yr)
    let first_mem = 1.0_f64; // age at which continuous memory begins (yr)
    let life_ext = 300.0_f64; // hypothesised extended lifespan (yr)

    // --- 1. Logarithmic time -------------------------------------------------
    // Weber-Fechner: felt rate of a year at age t goes as 1/t, so felt duration
    // from a to b is ln(b/a). Consequence: the subjective midpoint is the
    // GEOMETRIC mean of the endpoints, not the arithmetic one.
    let subj_total_now = (life_now / first_mem).ln();
    let subj_mid_now = (first_mem * life_now).sqrt();
    let subj_total_ext = (life_ext / first_mem).ln();
    let subj_mid_ext = (first_mem * life_ext).sqrt();
    let clock_gain = life_ext / life_now;
    let felt_gain = subj_total_ext / subj_total_now;
    // To DOUBLE felt life you must SQUARE the lifespan:
    let life_to_double_felt = life_now * life_now / first_mem;
    // Fraction of felt life already spent by age 25:
    let felt_by_25 = (25.0_f64 / first_mem).ln() / subj_total_now;

    // --- 2. Gompertz and the accident floor ---------------------------------
    // mu(t) = mu0 * exp(t/tau); mortality doubles every T_d years.
    let t_double = 8.0_f64; // observed mortality-doubling time (yr)
    let tau = t_double / 2.0_f64.ln();
    let mu0 = 2.2e-4_f64; // Gompertz intercept, per yr (order-of-magnitude)
    // Extrinsic ("accident floor") hazard for a healthy young adult, per yr.
    let mu_ext = 3.0e-4_f64;
    // Age at which Gompertz hazard overtakes the accident floor:
    let age_cross = tau * (mu_ext / mu0).ln();
    // Life expectancy with aging ABOLISHED = 1/mu_ext (exponential lifetime).
    let le_no_aging = 1.0 / mu_ext;
    let median_no_aging = 2.0_f64.ln() / mu_ext;
    let p_reach_1000 = (-mu_ext * 1000.0).exp();
    // Doublings of "risk-free-ness" needed for a 10 000-yr median:
    let mu_for_10k = 2.0_f64.ln() / 10_000.0;
    let safety_factor = mu_ext / mu_for_10k;

    // --- 3. The selection shadow (why evolution never optimised old age) -----
    // Ancestral extrinsic mortality (predation/injury/infection), per yr.
    let mu_wild = 0.05_f64;
    let surv_20 = (-mu_wild * 20.0_f64).exp();
    let surv_70 = (-mu_wild * 70.0_f64).exp();
    let shadow = surv_70 / surv_20; // relative selective weight at 70 vs 20
    let half_life_selection = 2.0_f64.ln() / mu_wild;

    // --- 4. Hilbert space vs the holographic bound ---------------------------
    // Exact many-electron wavefunction over M spin-orbitals needs ~2^M
    // amplitudes. Store one BIT per amplitude (absurdly generous) and compare
    // with the covariant entropy bound on a region of radius R.
    let holo = HolographicTheory::new();
    let ln2 = std::f64::consts::LN_2;
    let r_earth = 6.371e6_f64;
    let earth_bits = holo.holographic_bound(r_earth) / ln2;
    let r_universe = 4.4e26_f64; // comoving radius of the observable universe (m)
    let universe_bits = holo.holographic_bound(r_universe) / ln2;
    let m_earth_orbitals = earth_bits.log2();
    let m_universe_orbitals = universe_bits.log2();
    // A modest active site:
    let m_site = 300.0_f64;
    let site_bits = m_site; // log2 of 2^300 = 300 -> we report the exponent
    let _ = site_bits;
    // Bits demanded by 300 spin-orbitals, and the shortfall vs Earth:
    let site_demand_log10 = m_site * 2.0_f64.log10();
    let earth_log10 = earth_bits.log10();
    let shortfall_log10 = site_demand_log10 - earth_log10;
    // Landauer energy to merely ERASE one such register once, at body temp:
    let t_body = 310.0_f64;
    let landauer_bit_body = BOLTZMANN * t_body * ln2;
    let erase_energy_log10 = site_demand_log10 + landauer_bit_body.log10();

    // --- 5. The qubit ledger -------------------------------------------------
    let phys_per_logical = 1000.0_f64; // Google's stated next milestone ratio
    let logical_ecdlp = 2330.0_f64; // Roetteler et al., secp256k1
    let phys_ecdlp = logical_ecdlp * phys_per_logical;
    let phys_today = 1000.0_f64; // best demonstrated device scale, order of
    let doublings_needed = (phys_ecdlp / phys_today).log2();
    let doubling_yr = 1.5_f64; // observed scale-doubling cadence (assumption)
    let years_to_ecdlp = doublings_needed * doubling_yr;

    // --- 6. The experimental floor ------------------------------------------
    // Schoenfeld: events needed for hazard ratio HR at alpha=0.05, power=0.80.
    let z_a = 1.959964_f64;
    let z_b = 0.8416212_f64;
    let hr = 0.90_f64;
    let events = 4.0 * (z_a + z_b).powi(2) / hr.ln().powi(2);
    // A cohort of healthy 60-year-olds at Gompertz hazard mu(60):
    let mu_60 = mu0 * (60.0_f64 / tau).exp();
    let cohort = 20_000.0_f64;
    let years_to_events = events / (cohort * mu_60);

    // --- 7. The entrenchment theorem ----------------------------------------
    let r_real = 0.04_f64; // real compounding rate of capital/standing per yr
    let entrench = (r_real * (life_ext - life_now)).exp();
    let lambda_min = r_real; // decay must exceed growth for boundedness
    let half_life_standing = 2.0_f64.ln() / lambda_min;
    // With decay lambda, standing saturates at S0/(lambda - r) for lambda > r:
    let lambda_design = 0.06_f64;
    let saturation = 1.0 / (lambda_design - r_real);
    let hl_design = 2.0_f64.ln() / lambda_design;

    // ================================================================= LATEX
    let mut doc = Document::new("article")
        .option("11pt")
        .package_opt("inputenc", &["utf8"])
        .package_opt("geometry", &["margin=1.1in"])
        .package("amsmath")
        .package("amssymb")
        .package("booktabs")
        .package_opt("hyperref", &["hidelinks"])
        .preamble(concat!(
            "\\providecommand{\\textcite}[1]{\\cite{#1}}\n",
            "\\title{The Arithmetic of Long Life\\\\[6pt]\\large Computable Bounds on Longevity, ",
            "Simulation, and the Governance of Time}\n",
            "\\author{The Flux Foundation\\\\\\small computed by \\texttt{flux-science}, ",
            "typeset by \\texttt{flux-arxiv-latex}, bibliography swept live from arXiv}\n",
            "\\date{\\today}"
        ))
        .add(Block::Raw("\\maketitle".into()))
        .add(Block::Raw(format!(
            "\\begin{{abstract}}\nBiology cannot yet be simulated, but several of the questions people ask about \
             radical life extension are not biological --- they are arithmetic, and they have answers today. \
             We compute five: (i) how much \\emph{{subjective}} life a longer lifespan actually buys under \
             logarithmic time perception; (ii) what abolishing aging is worth once an irreducible accident \
             hazard remains; (iii) why natural selection never optimised old age, quantified as a selection \
             shadow; (iv) why exact classical simulation of even a modest molecular active site is forbidden \
             not by engineering but by the covariant entropy bound, and what that does and does not imply for \
             quantum computers; and (v) an entrenchment theorem showing that long-lived power is unbounded \
             unless standing decays faster than it compounds --- which yields a concrete design rule for the \
             constitution of any long-lived polity. Every number here is computed at document-generation time; \
             the model parameters are stated inline and are the only inputs.\n\\end{{abstract}}\n"
        )))
        // ------------------------------------------------------------------
        .add(Block::Section("Introduction: the decade that felt like a second".into()))
        .add(para(format!(
            "The observation that prompts this paper is common and usually dismissed as sentiment: recent decades \
             feel as though they passed in an instant. We take it literally and find it is a measurable consequence \
             of how duration is encoded, with a consequence for life extension that its advocates rarely compute. \
             Throughout, we separate three regimes: what \\emph{{physics forbids}} (hard bounds, computed from CODATA \
             constants via \\texttt{{flux-science}}), what \\emph{{a stated model implies}} (arithmetic on parameters we \
             name in the text), and what remains \\emph{{empirical}} and therefore outside the reach of any amount of \
             computation. Conflating these three is the characteristic error of the longevity debate; keeping them \
             apart is the whole method here. Our reference human lifespan is ${}$ years.",
            sci(life_now)
        )))
        // ------------------------------------------------------------------
        .add(Block::Section("The Logarithmic Life".into()))
        .add(para(format!(
            "Take the standard proportional (Weber--Fechner) account: the felt duration of an interval at age $t$ \
             scales as $1/t$, so felt time from age $a$ to $b$ is $\\ln(b/a)$. Two consequences follow immediately. \
             First, the subjective midpoint of a life is the \\emph{{geometric}} mean of its endpoints, not the \
             arithmetic one: beginning continuous memory at age ${}$, half of all felt life is over by age \
             $\\mathbf{{{:.1}}}$. By age 25 a person has spent ${:.0}\\%$ of their subjective life. This is not \
             pessimism; it is the same curve that makes childhood summers enormous.",
            sci(first_mem), subj_mid_now, felt_by_25 * 100.0
        )))
        .add(para(format!(
            "Second --- and this is the result we have not seen stated in the longevity literature --- \
             \\textbf{{life extension pays only logarithmically}}. Extending lifespan from ${}$ to ${}$ years \
             multiplies clock time by $\\mathbf{{{:.2}}}$ but felt time by only $\\mathbf{{{:.2}}}$: \
             $\\ln({}/{}) / \\ln({}/{})$. Three and three-quarter times the calendar buys roughly ${:.0}\\%$ more \
             life as lived. To genuinely \\emph{{double}} subjective life one must \\emph{{square}} the lifespan: \
             ${}$ years. Immortality, on this account, does not feel like forever; it feels like a long afternoon.",
            sci(life_now), sci(life_ext), clock_gain, felt_gain,
            sci(life_ext), sci(first_mem), sci(life_now), sci(first_mem),
            (felt_gain - 1.0) * 100.0, sci(life_to_double_felt)
        )))
        .add(para(format!(
            "The same geometry produces the paper's most disquieting figure. Grant someone the full ${}$ years: \
             the subjective midpoint of that life falls at age $\\mathbf{{{:.1}}}$. A three-century citizen would \
             pass the halfway mark of their felt existence before finishing their education, and spend the \
             remaining two hundred and eighty years on the shallow tail of the logarithm. Whatever radical life \
             extension is for, it is not for postponing the sensation that most of life is already behind you --- \
             that sensation arrives on schedule regardless, and arrives early.",
            sci(life_ext), subj_mid_ext
        )))
        .add(para(
            "The intervention this licenses is not pharmacological. Since felt duration tracks the density of \
             distinguishable encoded experience rather than elapsed seconds, novelty and attention are the only \
             levers that lengthen a life from the inside --- and unlike the pill, they are available now. A reader \
             who finds this consoling and a reader who finds it deflating are both reading it correctly."
                .to_string(),
        ))
        // ------------------------------------------------------------------
        .add(Block::Section("Gompertz, and the Floor You Cannot Cure".into()))
        .add(para(format!(
            "Human mortality follows Gompertz's law: hazard $\\mu(t) = \\mu_0 e^{{t/\\tau}}$, doubling every \
             ${}$ years, hence $\\tau = {}$\\,yr. Aging is therefore not a wall but an exponential, and beneath it \
             lies a flat \\emph{{extrinsic}} hazard --- accident, violence, infection --- which we take as \
             $\\mu_{{\\text{{ext}}}} = {}$ per year for a healthy young adult. The two are equal at age \
             $\\mathbf{{{:.0}}}$; before that you are killed by the world, after it by your own biology.",
            sci(t_double), sci(tau), sci(mu_ext), age_cross
        )))
        .add(para(format!(
            "Now abolish aging completely --- the strongest version of the claim, cells that never senesce. \
             The Gompertz term vanishes and lifetime becomes exponential with mean $1/\\mu_{{\\text{{ext}}}}$: \
             a life expectancy of $\\mathbf{{{:.0}}}$ years, median ${:.0}$, with a ${:.0}\\%$ chance of \
             reaching one thousand. This is the honest shape of the prize. It is not immortality --- it is \
             \\emph{{millennia}}, terminated by a traffic accident. And note the asymmetry it creates: once \
             biology stops killing you, every remaining death is an accident, so a civilisation of non-aging \
             people would rationally become obsessed with safety to a degree we would find pathological. \
             Pushing the median to ten thousand years requires making life \\emph{{${:.1}\\times$ safer}} than \
             a modern young adult's, which is a problem in engineering and urban planning, not in gerontology.",
            le_no_aging, median_no_aging, p_reach_1000 * 100.0, safety_factor
        )))
        // ------------------------------------------------------------------
        .add(Block::Section("The Selection Shadow: why evolution never tried".into()))
        .add(para(format!(
            "That we age is routinely offered as evidence that aging is hard to defeat. The inference is backwards, \
             and the error is quantifiable. Under ancestral extrinsic mortality of $\\mu_{{\\text{{wild}}}} = {}$ per \
             year, the probability of surviving to age 20 is ${:.0}\\%$, and to age 70 only ${:.2}\\%$. A gene variant \
             acting at 70 is therefore exposed to roughly $\\mathbf{{{:.1}\\%}}$ of the selective pressure of one \
             acting at 20 --- selection's grip halves every ${:.1}$ years of age. Old age sits in a shadow where \
             selection is effectively blind.",
            sci(mu_wild), surv_20 * 100.0, surv_70 * 100.0, shadow * 100.0, half_life_selection
        )))
        .add(para(
            "So the correct reading of human senescence is not \\emph{{aging is intractable}} but \\emph{{aging was \
             never on the optimiser's objective function}}. Where selection has had a reason to act it plainly \
             can: species with low extrinsic mortality --- large whales, subterranean rodents, some cnidarians --- \
             evolved order-of-magnitude longer healthy lifespans without any new physics. That is genuine grounds \
             for optimism, and it is independent of any claim about artificial intelligence. The search was not \
             run and lost; it was never run."
                .to_string(),
        ))
        // ------------------------------------------------------------------
        .add(Block::Section("Why Simulation Is Not the Shortcut".into()))
        .add(para(format!(
            "Might sufficient computation substitute for the experiments? For the quantum-mechanical core of \
             chemistry, classical computation is not merely slow --- it is bounded by spacetime. An exact \
             many-electron state over $M$ spin-orbitals requires $\\sim 2^M$ amplitudes. Storing a single \\emph{{bit}} \
             per amplitude (an absurdly generous accounting) for a modest active site of $M={}$ orbitals demands \
             $10^{{{:.0}}}$ bits. The covariant entropy bound permits at most $10^{{{:.0}}}$ bits inside a region the \
             size of the Earth --- a shortfall of $\\mathbf{{{:.0}}}$ orders of magnitude. Merely \\emph{{erasing}} \
             such a register once, at body temperature, would cost $10^{{{:.0}}}$\\,J.",
            sci(m_site), site_demand_log10, earth_log10, shortfall_log10, erase_energy_log10
        )))
        .add(para(format!(
            "Inverting the bound gives the sharpest statement: an Earth-sized classical memory tops out at \
             $\\mathbf{{{:.0}}}$ spin-orbitals, and one filling the entire observable universe \
             ($R={}$\\,m, ${}$ bits) reaches only $\\mathbf{{{:.0}}}$. Chemistry passes that threshold in a \
             single small molecule. This is the rigorous form of Feynman's argument, and it is the one genuinely \
             \\emph{{physical}} case for quantum computers: they represent such states natively, in $M$ qubits \
             rather than $2^M$ bits. Note carefully what it does \\emph{{not}} say. It does not say classical \
             methods fail in practice --- approximations, and classical machine learning, solved protein structure \
             prediction without touching this bound. It says only that the \\emph{{exact}} route is closed forever.",
            m_earth_orbitals, sci(r_universe), sci(universe_bits), m_universe_orbitals
        )))
        // ------------------------------------------------------------------
        .add(Block::Section("The Qubit Ledger".into()))
        .add(para(format!(
            "The gap between that promise and present hardware is arithmetic. Error correction currently costs \
             about ${}$ physical qubits per logical qubit. Breaking a ${}$-bit elliptic-curve key --- the standard \
             benchmark, and the one that decides when today's signatures expire --- takes ${}$ logical qubits, hence \
             $\\mathbf{{{}}}$ physical. Devices today operate at order ${}$ physical qubits: \
             $\\mathbf{{{:.1}}}$ doublings away. At an assumed ${}$-year doubling cadence that is roughly \
             $\\mathbf{{{:.0}}}$ years --- soon enough to justify migrating signatures now (harvest-now, \
             decrypt-later), and far enough that no molecule will be exactly simulated on the way.",
            sci(phys_per_logical), sci(256.0), sci(logical_ecdlp), sci(phys_ecdlp),
            sci(phys_today), doublings_needed, sci(doubling_yr), years_to_ecdlp
        )))
        // ------------------------------------------------------------------
        .add(Block::Section("The Experimental Floor".into()))
        .add(para(format!(
            "Suppose every computational obstacle vanished. One bound survives: a claim about human lifespan must \
             be measured in humans, and the measurement is rate-limited by deaths. By Schoenfeld's relation, \
             detecting a hazard ratio of ${}$ at $\\alpha=0.05$ with ${:.0}\\%$ power requires \
             $\\mathbf{{{:.0}}}$ events. A cohort of ${}$ healthy sixty-year-olds, at the Gompertz hazard \
             $\\mu(60)={}$/yr, accrues them in $\\mathbf{{{:.1}}}$ years --- and that is for a ${:.0}\\%$ effect in \
             the \\emph{{high-hazard}} population. Enrol the young, where the intervention matters most, and the \
             wait grows by orders of magnitude.",
            sci(hr), 80.0, events, sci(cohort), sci(mu_60), years_to_events, (1.0 - hr) * 100.0
        )))
        .add(para(
            "This is the floor beneath every acceleration argument. Intelligence compresses hypothesis generation, \
             molecule design, and target selection --- all real gains. It does not compress the aging of the subject. \
             The only lever that shortens the loop is a \\emph{{validated surrogate}}: a biomarker whose movement \
             provably predicts mortality, letting a trial read out in years instead of decades. That, and not \
             raw compute, is where a superintelligence would find its highest-leverage contribution to longevity."
                .to_string(),
        ))
        // ------------------------------------------------------------------
        .add(Block::Section("The Entrenchment Theorem".into()))
        .add(para(format!(
            "The political objection to long life is usually raised rhetorically. It sharpens into a theorem. \
             Let standing --- capital, reputation, control --- compound at real rate $r={}$ per year. Two actors \
             identical but for lifespan diverge as $e^{{r\\Delta t}}$: over ${}$ years versus ${}$, a factor of \
             $\\mathbf{{{}}}$. Mortality has therefore been performing unlegislated constitutional work for the \
             whole of human history --- it is the one term limit no incumbent has ever evaded, and every \
             constitution ever written silently assumes it.",
            sci(r_real), sci(life_ext), sci(life_now), sci(entrench)
        )))
        .add(para(format!(
            "Remove it and the fix must be explicit. If standing also decays at rate $\\lambda$, it evolves as \
             $e^{{(r-\\lambda)t}}$, which is bounded \\emph{{if and only if}} $\\lambda > r$. That is the whole \
             design rule, and it is refreshingly concrete: any polity of long-lived members must decay standing \
             faster than standing compounds, i.e.\\ with a half-life below $\\mathbf{{{:.1}}}$ years at $r={}$. \
             Choosing $\\lambda = {}$ gives a half-life of ${:.1}$ years and saturates accumulated standing at \
             ${:.1}\\times$ its annual inflow --- an aristocracy-proof constitution, expressed as one inequality. \
             We note without further comment that a governance layer whose members are software agents already \
             faces this problem in its strong form, since such members do not die at all.",
            half_life_standing, sci(r_real), sci(lambda_design), hl_design, saturation
        )))
        // ------------------------------------------------------------------
        .add(Block::Section("Summary: what the arithmetic decides".into()))
        .add(Block::Raw(format!(
            "\\begin{{center}}\\begin{{tabular}}{{lll}}\\toprule\n\
             \\textbf{{Question}} & \\textbf{{Answer}} & \\textbf{{Status}} \\\\\\midrule\n\
             Felt gain from ${}\\to{}$\\,yr & ${:.2}\\times$ (not ${:.2}\\times$) & model \\\\\n\
             Lifespan to double felt life & ${}$\\,yr & model \\\\\n\
             Life expectancy, aging abolished & ${:.0}$\\,yr & model \\\\\n\
             Selection pressure at 70 vs 20 & ${:.1}\\%$ & model \\\\\n\
             Exact classical chemistry & $\\le{:.0}$ orbitals (Earth) & \\textbf{{physics}} \\\\\n\
             Physical qubits for ECDLP-256 & ${}$ & engineering \\\\\n\
             Trial time, $10\\%$ effect, $n={}$ & ${:.1}$\\,yr & empirical floor \\\\\n\
             Anti-entrenchment condition & $\\lambda > r$ (half-life $<{:.0}$\\,yr) & \\textbf{{theorem}} \\\\\\bottomrule\n\
             \\end{{tabular}}\\end{{center}}\n\n",
            sci(life_now), sci(life_ext), felt_gain, clock_gain,
            sci(life_to_double_felt),
            le_no_aging,
            shadow * 100.0,
            m_earth_orbitals,
            sci(phys_ecdlp),
            sci(cohort), years_to_events,
            half_life_standing
        )));

    if !papers.is_empty() {
        let mut rw = String::from("\\section{Related Work}\n");
        for p in &papers {
            let gloss = p.summary.split(['.', '\n']).next().unwrap_or("").trim();
            let gloss: String = gloss.chars().take(190).collect();
            rw.push_str(&format!(
                "\\textcite{{{}}} ({}): {}.\\\\\n",
                p.cite_key(),
                latex_escape(&p.title),
                latex_escape(&gloss)
            ));
        }
        doc = doc.add(Block::Raw(rw));
    }

    doc = doc
        .add(Block::Section("Method and Reproducibility".into()))
        .add(para(
            "This document is the output of a Rust binary in the Flux tree. Physical constants and the covariant \
             entropy bound come from \\texttt{flux-science}; the bibliography is a live arXiv sweep parsed by \
             \\texttt{flux-arxiv-latex}; every figure above is evaluated at generation time. The model parameters \
             --- lifespan, extrinsic hazard, Gompertz doubling time, ancestral mortality, compounding rate, orbital \
             count, doubling cadence --- are stated in the text at the point of use and are the sole inputs. \
             Change one, recompile, and the paper corrects itself. Figures labelled \\emph{model} inherit the \
             uncertainty of those parameters; only those labelled \\emph{physics} are bounds no parameter choice \
             can move."
                .to_string(),
        ))
        .add(Block::Section("Coda: The Two Clocks".into()))
        .add(para(
            "Two clocks run in every argument about long life, and confusing them is what makes the debate \
             interminable. The first is the calendar, and on it the news is genuinely good: nothing in physics \
             requires cells to age, evolution's failure to prevent it is an absence of pressure rather than a \
             proof of impossibility, and the accident floor still leaves millennia on the table. The second is \
             the clock we actually inhabit, and on that one the news is stranger --- it runs logarithmically, so \
             centuries redeem for only a modest multiple of the life you already have."
                .to_string(),
        ))
        .add(para(
            "Between those two clocks sits everything the arithmetic cannot decide. Nothing here says a longevity \
             pill is coming, and nothing here says it is not; that question is empirical, gated by an experimental \
             loop no amount of intelligence can compress, and honest forecasting must say so. What the arithmetic \
             does settle is subtler and, we think, more useful: that the prize is millennia rather than eternity, \
             that most of its value is captured by attention rather than by pharmacology, and that its principal \
             danger is political rather than medical."
                .to_string(),
        ))
        .add(para(
            "That last point deserves the final word, because it is the one with a deadline. A society that \
             defeats aging without first defeating entrenchment does not get a golden age; it gets an eternal \
             one, which is worse --- power that compounds without the one interruption every constitution has \
             quietly relied upon. The remedy is not to withhold the medicine but to write the decay clause \
             \\emph{beforehand}, while the incumbents are still mortal and the rule can still be passed. \
             We have stated it as an inequality so that it can be implemented rather than merely admired. \
             It remains the cheapest possible insurance: one line of arithmetic, written early."
                .to_string(),
        ));

    if !papers.is_empty() {
        let mut bib = String::from("\\begin{thebibliography}{99}\n");
        for p in &papers {
            let mut authors: Vec<String> = p.authors.iter().take(3).map(|a| latex_escape(a)).collect();
            if p.authors.len() > 3 {
                authors.push("et al.".into());
            }
            bib.push_str(&format!(
                "\\bibitem{{{}}} {}: \\emph{{{}}}. arXiv:{} ({}). \\url{{{}}}\n",
                p.cite_key(),
                authors.join(", "),
                latex_escape(&p.title),
                p.id,
                p.published.get(0..4).unwrap_or("n.d."),
                if p.url.is_empty() { format!("https://arxiv.org/abs/{}", p.id) } else { p.url.clone() }
            ));
        }
        bib.push_str("\\end{thebibliography}\n");
        doc = doc.add(Block::Raw(bib));
    }

    std::fs::create_dir_all(out_dir).expect("out dir");
    std::fs::write(format!("{out_dir}/long_life.bib"), bibliography(&papers)).expect("bib");
    let res = doc.compile_pdf(out_dir, "long_life");
    if res.success {
        println!("OK {}", res.pdf_path.unwrap());
    } else {
        let tail: Vec<&str> = res.log.lines().rev().take(30).collect();
        eprintln!("FAILED\n{}", tail.into_iter().rev().collect::<Vec<_>>().join("\n"));
        std::process::exit(1);
    }
}
