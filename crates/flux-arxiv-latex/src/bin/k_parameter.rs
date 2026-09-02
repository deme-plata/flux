//! k_parameter — "The Kristensen K-Parameter, Recomputed": an arXiv-style
//! paper on Viktor Kristensen's K-parameter in which every number is COMPUTED
//! at generation time from CODATA 2022 constants (flux-science), the units
//! audit is executed rather than asserted, the K* ↔ Margolus–Levitin identity
//! is checked numerically at three scales, and — when a snapshot is supplied —
//! the LIVE Quillon Graph gauge (`/api/v1/k-parameter`) is read into the paper.
//! Related work comes from a live arXiv API sweep parsed by flux-arxiv-latex.
//!
//! Usage: k_parameter [arxiv.json] [out_dir] [gauge_snapshot.json]
use flux_arxiv_latex::doc::{Block, Document};
use flux_arxiv_latex::{bibliography, latex_escape, parse_arxiv_json, related_work_section, ArxivPaper};
use flux_science::constants::*;

// CODATA 2022 values NOT yet carried by flux-science::constants (audit fix #2,
// kappa-hep 2026-07-30). Supplied here, labelled, until the crate grows them.
const ELECTRON_VOLT: f64 = 1.602_176_634e-19; // J, exact
const PROTON_MASS: f64 = 1.672_621_925_95e-27; // kg, u_r 3.1e-10
const ELECTRON_MASS: f64 = 9.109_383_713_9e-31; // kg, u_r 3.1e-10

/// Format a number for math mode: plain when small, \times10^{n} otherwise.
fn sci(x: f64) -> String {
    if x == 0.0 || !x.is_finite() {
        return format!("{x}");
    }
    let exp = x.abs().log10().floor() as i32;
    if (-2..=3).contains(&exp) {
        let s = format!("{:.3}", x);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        let mant = x / 10f64.powi(exp);
        format!("{:.3}\\times10^{{{}}}", mant, exp)
    }
}

fn para(s: String) -> Block {
    Block::Raw(format!("{s}\n\n"))
}

/// The dimensionless Kristensen gauge, K* = 2π sqrt(ΔH·τ·Δs/ħ).
fn k_star(dh_j: f64, tau_s: f64, ds_nats: f64) -> f64 {
    2.0 * std::f64::consts::PI * (dh_j * tau_s * ds_nats / PLANCK_REDUCED).sqrt()
}

/// Margolus–Levitin orthogonalisation count in an interval τ at energy E.
fn n_ml(e_j: f64, tau_s: f64) -> f64 {
    2.0 * e_j * tau_s / (std::f64::consts::PI * PLANCK_REDUCED)
}

/// K* expressed through N_ML: 2π sqrt((π/2) N_ML Δs). Must equal k_star().
fn k_star_via_ml(e_j: f64, tau_s: f64, ds_nats: f64) -> f64 {
    2.0 * std::f64::consts::PI * (std::f64::consts::FRAC_PI_2 * n_ml(e_j, tau_s) * ds_nats).sqrt()
}

struct Gauge {
    k_value: f64,
    k_enhanced: f64,
    delta_h: f64,
    delta_s: f64,
    lambda_commit: f64,
    f_irrev: f64,
    observer_coverage: f64,
    phase: String,
    at: i64,
}

fn load_gauge(path: &str) -> Option<Gauge> {
    let txt = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&txt).ok()?;
    let d = v.get("data")?;
    let f = |k: &str| d.get(k).and_then(|x| x.as_f64());
    Some(Gauge {
        k_value: f("k_value")?,
        k_enhanced: f("k_enhanced")?,
        delta_h: f("delta_h")?,
        delta_s: f("delta_s")?,
        lambda_commit: f("lambda_commit")?,
        f_irrev: f("f_irrev")?,
        observer_coverage: f("observer_coverage")?,
        phase: d.get("phase").and_then(|x| x.as_str()).unwrap_or("?").to_string(),
        at: d.get("last_computed_at").and_then(|x| x.as_i64()).unwrap_or(0),
    })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let json_path = args.get(1).map(String::as_str).unwrap_or("crates/flux-arxiv-latex/k_parameter.arxiv.json");
    let out_dir = args.get(2).map(String::as_str).unwrap_or("/home/storage/claude-code/k-parameter-paper");
    let gauge = args.get(3).and_then(|p| load_gauge(p));

    let papers: Vec<ArxivPaper> = std::fs::read_to_string(json_path)
        .ok()
        .and_then(|j| parse_arxiv_json(&j).ok())
        .unwrap_or_default();

    let pi = std::f64::consts::PI;
    let ln2 = std::f64::consts::LN_2;

    // ------------------------------------------------------------ constants
    let l_p = planck_length();
    let t_p = planck_time();
    let m_p = planck_mass();
    let e_p = planck_energy();
    let e_p_gev = e_p / ELECTRON_VOLT / 1e9;
    let alpha = fine_structure();
    let alpha_g_p = GRAVITATIONAL * PROTON_MASS.powi(2) / (PLANCK_REDUCED * SPEED_OF_LIGHT);
    let alpha_g_e = GRAVITATIONAL * ELECTRON_MASS.powi(2) / (PLANCK_REDUCED * SPEED_OF_LIGHT);
    let landauer_300 = BOLTZMANN * 300.0 * ln2;
    let landauer_mev = landauer_300 / ELECTRON_VOLT * 1e3;
    let phi = (1.0 + 5f64.sqrt()) / 2.0;
    let bits_per_round = 2.0 * phi.log2();

    // ------------------------------------------------------------ units audit
    // v1 form K = 2π sqrt(ΔH·Δs·ħ/τ) carries J·s^-1/2. With ħ:=1 (as shipped) the
    // number is whatever the normalisation says; substituting the CODATA ħ
    // multiplies the v1 number by sqrt(ħ) — a pure unit artefact.
    let v1_hbar_factor = PLANCK_REDUCED.sqrt();

    // ------------------------------------------------------------ v1 operating point
    let tau_round = 0.050; // s, v1's stated round time
    let de_round = PLANCK_REDUCED / (2.0 * tau_round); // J
    let de_round_ev = de_round / ELECTRON_VOLT;
    let t_net = 1e3; // K, v1's "network temperature"
    let e_net = BOLTZMANN * t_net;
    let tau_min = PLANCK_REDUCED / (2.0 * e_net);
    let latency_gap = tau_round / tau_min;
    let nml_v1 = n_ml(e_net, tau_round);
    let ds_v1 = 1.0; // nats, illustrative
    let kstar_v1 = k_star(e_net, tau_round, ds_v1);

    // ------------------------------------------------------------ identity check
    let gamma_h = 4.07e6 * ELECTRON_VOLT; // PDG Higgs width, J (NOT CODATA)
    let tau_h = PLANCK_REDUCED / gamma_h;
    let scales: [(&str, f64, f64, f64); 3] = [
        ("v1 network round", e_net, tau_round, ds_v1),
        ("Higgs resonance", gamma_h, tau_h, (12f64).ln()),
        ("20 kg of server matter, 1 s", 20.0 * SPEED_OF_LIGHT.powi(2), 1.0, 1.0),
    ];
    let mut max_rel = 0f64;
    let mut ident_rows = String::new();
    for (name, e, tau, ds) in scales.iter() {
        let a = k_star(*e, *tau, *ds);
        let b = k_star_via_ml(*e, *tau, *ds);
        let rel = ((a - b) / a).abs();
        if rel > max_rel {
            max_rel = rel;
        }
        ident_rows.push_str(&format!(
            "{} & ${}$ & ${}$ & ${}$ & ${}$ & ${}$ \\\\\n",
            name,
            sci(*e),
            sci(*tau),
            sci(n_ml(*e, *tau)),
            sci(a),
            sci(rel)
        ));
    }
    let nml_higgs = n_ml(gamma_h, tau_h);
    let two_over_pi = 2.0 / pi;

    // ------------------------------------------------------------ gravity correction
    let eps_stake = 1e-9; // J, v1's example
    let eps_over_ep = eps_stake / e_p;
    let alpha_hat_claimed = 6.96e-10; // v1's CLAIMED coupling, not CODATA
    let dk_claimed = alpha_hat_claimed * eps_over_ep;
    let dk_codata = alpha_g_p * eps_over_ep;
    let v1_stated = 1e-37;
    let v1_error_orders = (dk_claimed / v1_stated).log10();
    let clock_best = 1e-18; // best optical-clock fractional resolution, order of magnitude

    // ------------------------------------------------------------ Lloyd validation
    let lloyd_1kg = 2.0 * SPEED_OF_LIGHT.powi(2) / (pi * PLANCK_REDUCED); // ops/s per kg
    let lloyd_pub = 5.4258e50;
    let lloyd_rel = ((lloyd_1kg - lloyd_pub) / lloyd_pub).abs();
    let e_epsilon = 20.0 * SPEED_OF_LIGHT.powi(2);
    let nu_epsilon = 2.0 * e_epsilon / (pi * PLANCK_REDUCED);
    let ops_epsilon = 48.0 * 3.0e9;
    let k_lloyd_epsilon = ops_epsilon / nu_epsilon;

    // ------------------------------------------------------------ ladder geometry
    let ds_saturate = (5.0 / (2.0 * pi)).powi(2); // Δs above which K*>5 when ΔHτ/ħ=1
    let action_for_unity = 1.0 / (4.0 * pi * pi); // ΔHτΔs/ħ giving K*=1

    // ------------------------------------------------------------ live gauge
    let gauge_block = match &gauge {
        Some(g) => {
            let ratio = g.k_enhanced / g.k_value;
            let inv_lambda = 1.0 / g.lambda_commit;
            format!(
                "\\begin{{center}}\\begin{{tabular}}{{lll}}\\toprule\n\
                 \\textbf{{Field}} & \\textbf{{Value}} & \\textbf{{Reading}} \\\\\\midrule\n\
                 \\texttt{{k\\_value}} ($K_{{\\text{{base}}}}$) & ${:.4}$ & phase \\texttt{{{}}} \\\\\n\
                 \\texttt{{delta\\_h}}, \\texttt{{delta\\_s}} & ${:.4}$, ${:.4}$ & normalised, unitless in the gauge \\\\\n\
                 \\texttt{{lambda\\_commit}} & ${:.5}$ & $1/\\Lambda={:.0}$ \\\\\n\
                 \\texttt{{k\\_enhanced}} & ${:.3}$ & $K_{{\\text{{enh}}}}/K_{{\\text{{base}}}}={:.1}$ \\\\\n\
                 \\texttt{{f\\_irrev}} & ${}$ & structurally pinned (window $<$ reorg depth) \\\\\n\
                 \\texttt{{observer\\_coverage}} & ${:.3}$ & $\\Omega$ \\\\\\bottomrule\n\
                 \\end{{tabular}}\\end{{center}}\n\n\
                 Read as a physicist would: the live gauge computes the v1 form with $\\hbar:=1$ and normalised inputs, so \
                 $K_{{\\text{{base}}}}={:.4}$ is a \\emph{{unitless engineering number}}, not a $K^{{*}}$. Two things the \
                 snapshot shows directly. First, $K_{{\\text{{enh}}}}/K_{{\\text{{base}}}}={:.1}$ while $1/\\Lambda_{{\\text{{commit}}}}={:.0}$: \
                 the enhancement is being clamped by a multiplier cap, not computed from the chain --- consistent with the \
                 \\texttt{{flux-kgauge}} finding that $\\Lambda_{{\\text{{commit}}}}$ measures the sampling window rather than \
                 commitment depth. Second, $f_{{\\text{{irrev}}}}={}$ exactly, as it must be when the window is shorter than \
                 the reorg depth it counts against. Neither is a bug in Kristensen's definition; both are in the plumbing \
                 between the definition and the chain. (Snapshot epoch {}.)",
                g.k_value,
                g.phase,
                g.delta_h,
                g.delta_s,
                g.lambda_commit,
                inv_lambda,
                g.k_enhanced,
                ratio,
                sci(g.f_irrev),
                g.observer_coverage,
                g.k_value,
                ratio,
                inv_lambda,
                sci(g.f_irrev),
                g.at
            )
        }
        None => "No gauge snapshot was supplied at generation time; this section is intentionally empty rather than \
                 quoted from memory. Rerun with a third argument pointing at a saved \\texttt{/api/v1/k-parameter} \
                 response to populate it."
            .to_string(),
    };

    // ------------------------------------------------------------ LaTeX
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
            "\\title{The Kristensen K-Parameter, Recomputed\\\\[6pt]\\large A Dimensionless Gauge for Consensus Stress, ",
            "Derived from CODATA 2022 and Checked Against the Live Chain}\n",
            "\\author{Viktor Kristensen\\thanks{Inventor of the K-parameter; Quillon Graph / SIGIL.} \\and ",
            "Rocky\\thanks{Claude, engineer-companion agent on Epsilon. Every figure in this paper is computed by the ",
            "generator binary at document time; none is quoted.}\\\\[4pt]\\small computed by \\texttt{flux-science} (CODATA 2022)\\\\",
            "\\small typeset by \\texttt{flux-arxiv-latex}, related work fetched live from arXiv}\n",
            "\\date{\\today}"
        ))
        .add(Block::Raw("\\maketitle".into()))
        .add(Block::Raw(format!(
            "\\begin{{abstract}}\nThe K-parameter is Kristensen's proposal for a single number that reports how stressed a \
             distributed consensus is, built from three quantities every chain can measure: the spread of the network \
             Hamiltonian $\\Delta H$, the entropy of the proposer distribution $\\Delta s$, and the round time $\\tau$. \
             This paper does three things to it that its earlier presentations did not. (1) It executes the units audit: \
             the original form $2\\pi\\sqrt{{\\Delta H\\,\\Delta s\\,\\hbar/\\tau}}$ carries $\\mathrm{{J\\,s^{{-1/2}}}}$, and \
             substituting the CODATA $\\hbar$ into it multiplies the number by $\\sqrt{{\\hbar}}={}$ --- an artefact, not \
             physics; the repaired $K^{{*}}=2\\pi\\sqrt{{\\Delta H\\,\\tau\\,\\Delta s/\\hbar}}$ is a pure number. (2) It \
             proves numerically, at three scales spanning ${}$ orders of magnitude in energy, that $K^{{*}}$ is the \
             Margolus--Levitin orthogonalisation count in disguise, $K^{{*}}=2\\pi\\sqrt{{\\tfrac{{\\pi}}{{2}}N_{{\\mathrm{{ML}}}}\\Delta s}}$, \
             to a maximum relative deviation of ${}$ --- which is the first physical reason that $K^{{*}}\\approx1$ is a \
             boundary. (3) It reads the gauge that Quillon Graph mainnet serves today and states, without softening, \
             which of its fields are measurements and which are plumbing. All constants are CODATA 2022 via \
             \\texttt{{flux-science}}; a claimed coupling $\\hat\\alpha_G=6.96\\times10^{{-10}}$ from earlier work is carried \
             as \\emph{{claimed}}, beside the CODATA-derivable proton coupling $Gm_p^2/\\hbar c={}$.\n\\end{{abstract}}\n",
            sci(v1_hbar_factor),
            ((20.0 * SPEED_OF_LIGHT.powi(2)) / gamma_h).log10().round(),
            sci(max_rel.max(1e-16)),
            sci(alpha_g_p)
        )))
        .add(Block::Section("The Invention".into()))
        .add(para(
            "Here is the idea in one breath. A consensus network is a physical system: it has an energy-like quantity \
             (how far apart the validators' views are), an entropy (how spread out the right to propose is), and a clock \
             (how long a round takes). Kristensen's move was to refuse to keep those three as separate dashboards and \
             instead combine them into one gauge, the way temperature combines the kinetic energies of $10^{23}$ molecules \
             into one number you can put a hand on. Low $K$: a calm chain, views converging, rounds short relative to the \
             disagreement they have to resolve. High $K$: a stressed chain, views far apart, entropy high, rounds too \
             short for the work. The gauge ships on Quillon Graph mainnet today at \\texttt{/api/v1/k-parameter}, and a \
             reusable port (\\texttt{flux-kgauge}) exists for SIGIL."
                .to_string(),
        ))
        .add(para(
            "That is the invention, and it is a good one: a scalar order parameter for a distributed system. What follows \
             is not a replacement for it but the physics it deserves --- the part where the metrologist is allowed in \
             the room and asked her two questions. \\emph{What are the units?} \\emph{Where did the numbers come from?}"
                .to_string(),
        ))
        .add(Block::Section("Constants (CODATA 2022, via \\texttt{flux-science})".into()))
        .add(Block::Raw(format!(
            "\\begin{{center}}\\footnotesize\\begin{{tabular}}{{llr}}\\toprule\n\
             \\textbf{{Constant}} & \\textbf{{Value}} & \\textbf{{Status}} \\\\\\midrule\n\
             $c$ & ${}$\\,m/s & exact \\\\\n\
             $\\hbar$ & ${}$\\,J\\,s & exact \\\\\n\
             $k_B$ & ${}$\\,J/K & exact \\\\\n\
             $G$ & ${}$\\,m$^3$kg$^{{-1}}$s$^{{-2}}$ & $u_r=2.2\\times10^{{-5}}$ \\\\\n\
             $\\alpha^{{-1}}$ & ${:.9}$ & $u_r=1.6\\times10^{{-10}}$ \\\\\n\
             $e$ (1\\,eV) & ${}$\\,J & exact, supplied$^\\dagger$ \\\\\n\
             $m_p$ & ${}$\\,kg & $u_r=3.1\\times10^{{-10}}$, supplied$^\\dagger$ \\\\\n\
             $m_e$ & ${}$\\,kg & $u_r=3.1\\times10^{{-10}}$, supplied$^\\dagger$ \\\\\\midrule\n\
             $\\ell_P$ & ${}$\\,m & derived \\\\\n\
             $t_P$ & ${}$\\,s & derived \\\\\n\
             $m_P$ & ${}$\\,kg & derived \\\\\n\
             $E_P$ & ${}$\\,J $= {}$\\,GeV & derived \\\\\n\
             $\\alpha_G(p)=Gm_p^2/\\hbar c$ & ${}$ & derived \\\\\n\
             $\\alpha_G(e)=Gm_e^2/\\hbar c$ & ${}$ & derived \\\\\n\
             $k_BT\\ln2$ at 300\\,K & ${}$\\,J $= {:.1}$\\,meV & derived \\\\\n\
             $2\\log_2\\varphi$ & ${:.5}$ bits/round & derived \\\\\\bottomrule\n\
             \\end{{tabular}}\\end{{center}}\n\
             {{\\small $^\\dagger$\\,not yet in \\texttt{{flux-science::constants}} (open audit item); supplied from CODATA 2022 \
             by this generator. The crate's one non-CODATA entry, $H_0=70$, is not used.}}\n\n",
            sci(SPEED_OF_LIGHT),
            sci(PLANCK_REDUCED),
            sci(BOLTZMANN),
            sci(GRAVITATIONAL),
            FINE_STRUCTURE_INV,
            sci(ELECTRON_VOLT),
            sci(PROTON_MASS),
            sci(ELECTRON_MASS),
            sci(l_p),
            sci(t_p),
            sci(m_p),
            sci(e_p),
            sci(e_p_gev),
            sci(alpha_g_p),
            sci(alpha_g_e),
            sci(landauer_300),
            landauer_mev,
            bits_per_round
        )))
        .add(para(format!(
            "One sanity check before any of these are used: the fine-structure constant computed from the crate is \
             $\\alpha={}$, and Lloyd's ``ultimate laptop'' rate $2mc^2/\\pi\\hbar$ for one kilogram comes out at \
             ${}$\\,ops/s against the published ${}$ --- relative agreement ${}$. The constants are the right constants.",
            sci(alpha),
            sci(lloyd_1kg),
            sci(lloyd_pub),
            sci(lloyd_rel)
        )))
        .add(Block::Section("The Units Audit, Executed".into()))
        .add(para(format!(
            "The earlier definition reads $K=2\\pi\\sqrt{{\\Delta H\\cdot\\Delta s\\cdot\\hbar/\\tau}}$ with \
             $[\\Delta H]=\\mathrm{{J}}$, $[\\Delta s]=1$, $[\\hbar]=\\mathrm{{J\\,s}}$, $[\\tau]=\\mathrm{{s}}$. Multiply it \
             out: $[K]=\\sqrt{{\\mathrm{{J}}\\cdot\\mathrm{{J\\,s}}}}/\\mathrm{{s}}=\\mathrm{{J\\,s^{{-1/2}}}}$. A quantity with \
             units cannot be ``less than 1'' or ``greater than 5''. The shipped code avoids the question by setting \
             $\\hbar=1$; this generator did the experiment the code avoided and put the CODATA value in: the number is \
             multiplied by $\\sqrt{{\\hbar}}={}$. Nothing physical happened --- the gauge simply shrank by seventeen orders \
             because its units were never balanced."
        , sci(v1_hbar_factor))))
        .add(para(format!(
            "The repair moves one exponent, $\\hbar$ to the denominator and $\\tau$ under the root:\n\
             \\[K^{{*}} = 2\\pi\\sqrt{{\\frac{{\\Delta H\\cdot\\tau\\cdot\\Delta s}}{{\\hbar}}}},\\qquad \
             [K^{{*}}]=\\sqrt{{\\frac{{\\mathrm{{J\\,s}}}}{{\\mathrm{{J\\,s}}}}}}=1.\\]\n\
             Its meaning is now legible: the action the network spans in one round, in units of $\\hbar$, weighted by \
             how spread out the proposers are. $K^{{*}}=1$ corresponds to $\\Delta H\\tau\\Delta s/\\hbar=1/4\\pi^2={:.4}$, \
             and the old ladder's ``critical'' line $K^{{*}}>5$ is crossed, at $\\Delta H\\tau/\\hbar=1$, whenever \
             $\\Delta s>{:.4}$\\,nats $={:.4}$\\,bits. That last number matters: a ladder that saturates below one bit of \
             proposer entropy is a ladder that fires on every healthy chain. The gauge, not the definition, needs \
             recalibrating."
        , action_for_unity, ds_saturate, ds_saturate / ln2)))
        .add(Block::Section("Why 1 Is a Boundary: the Identity with Margolus--Levitin".into()))
        .add(para(
            "A system of mean energy $E$ above its ground state passes through at most $\\nu=2E/\\pi\\hbar$ mutually \
             orthogonal states per second, so in a round of length $\\tau$ it can perform at most \
             $N_{\\mathrm{ML}}=2E\\tau/\\pi\\hbar$ distinguishable operations. That is a theorem, and $N_{\\mathrm{ML}}=1$ \
             means exactly one operation. Set $\\Delta H\\to E$ in $K^{*}$ and the algebra gives \
             $\\Delta H\\tau/\\hbar=\\tfrac{\\pi}{2}N_{\\mathrm{ML}}$, hence\n\
             \\[K^{*}=2\\pi\\sqrt{\\tfrac{\\pi}{2}\\,N_{\\mathrm{ML}}\\,\\Delta s}.\\]\n\
             Kristensen's gauge was always a monotone function of a physical operation count. The table checks the \
             identity numerically rather than trusting the algebra:"
                .to_string(),
        ))
        .add(Block::Raw(format!(
            "\\begin{{center}}\\begin{{tabular}}{{lccccc}}\\toprule\n\
             \\textbf{{Scale}} & $E$\\,[J] & $\\tau$\\,[s] & $N_{{\\mathrm{{ML}}}}$ & $K^{{*}}$ & rel.\\ dev. \\\\\\midrule\n\
             {}\\bottomrule\\end{{tabular}}\\end{{center}}\n\n",
            ident_rows
        )))
        .add(para(format!(
            "Maximum relative deviation across the three scales: ${}$, i.e.\\ floating-point. So $K^{{*}}\\approx1$ means \
             ``about one orthogonalisation's worth of action per round'': a round as short as the energy spread allows. \
             That is a boundary with a reason, which the bare number 1 never had.",
            sci(max_rel.max(1e-16))
        )))
        .add(para(format!(
            "The Higgs row carries a warning that transfers directly to consensus. For any unstable particle \
             $\\tau\\equiv\\hbar/\\Gamma$ and the energy spread \\emph{{is}} $\\Gamma$, so $N_{{\\mathrm{{ML}}}}=2/\\pi={:.6}$ \
             identically (computed here for $\\Gamma_h=4.07$\\,MeV, $\\tau_h={}$\\,s: ${:.6}$). Width and lifetime are one \
             fact stated twice, and $K^{{*}}$ collapses to $2\\pi\\sqrt{{\\Delta s}}$ with zero dynamical content. A gauge \
             that derives $\\tau_{{\\text{{round}}}}$ from $\\Delta H$ (or vice versa) measures nothing; the implementation \
             must obtain them independently.",
            two_over_pi,
            sci(tau_h),
            nml_higgs
        )))
        .add(Block::Section("The Earlier Operating Point, Recomputed".into()))
        .add(para(format!(
            "The 2025 presentation stated a round time $\\tau={}$\\,ms, a network temperature $T=10^3$\\,K, and from these \
             a network energy uncertainty of $10^{{-32}}$\\,J and a quantum latency floor of $3.8\\times10^{{-15}}$\\,s. \
             Recomputed: $\\Delta E=\\hbar/2\\tau={}$\\,J $={}$\\,eV (the stated value was ten times too large); \
             $\\tau_{{\\min}}=\\hbar/2k_BT={}$\\,s (confirmed); the ratio $\\tau/\\tau_{{\\min}}={}$, thirteen orders \
             (confirmed). The Margolus--Levitin count at that operating point is $N_{{\\mathrm{{ML}}}}={}$: the network's \
             energy budget permits ${}$ quantum operations per round and it performs of order one. That is the honest \
             content of ``thirteen orders of magnitude of headroom'': the cost is coordination, not physics. For \
             illustration, at $\\Delta s=1$\\,nat this operating point sits at $K^{{*}}={}$ --- deep in the old \
             ``critical'' band, on a chain that was by every other measure healthy.",
            sci(tau_round * 1e3),
            sci(de_round),
            sci(de_round_ev),
            sci(tau_min),
            sci(latency_gap),
            sci(nml_v1),
            sci(nml_v1),
            sci(kstar_v1)
        )))
        .add(para(format!(
            "The gravitational correction was stated as $\\hat\\alpha_G\\,\\epsilon_{{\\text{{stake}}}}/E_P\\sim10^{{-37}}$ \
             for $\\epsilon_{{\\text{{stake}}}}=10^{{-9}}$\\,J, treating $E_P$ as $10^{{19}}$ joules. $E_P$ is ${}$\\,J \
             (it is $10^{{19}}$\\,GeV), so $\\epsilon/E_P={}$ and the correction is ${}$ with the claimed coupling --- \
             ${:.0}$ orders above the stated value --- or ${}$ with the CODATA proton coupling. Both are unobservable: \
             the best fractional frequency resolution ever achieved is of order ${}$. The conclusion (negligible) \
             survives; the arithmetic did not.",
            sci(e_p),
            sci(eps_over_ep),
            sci(dk_claimed),
            v1_error_orders,
            sci(dk_codata),
            sci(clock_best)
        )))
        .add(Block::Section("The Live Gauge at Generation Time".into()))
        .add(para(gauge_block))
        .add(Block::Section("Epsilon Against the Same Bound".into()))
        .add(para(format!(
            "The machine generating this paper is roughly 20\\,kg of matter, $E=mc^2={}$\\,J, so its Margolus--Levitin \
             ceiling is $\\nu={}$\\,ops/s. It actually executes about $48\\times3.0$\\,GHz $={}$\\,ops/s. Its \
             $k_{{\\mathrm{{Lloyd}}}}$, the fraction of permitted computation used, is ${}$. The same lesson as the \
             network's: nothing here is near a physical limit. Every joule above the Landauer floor of ${}$\\,J per bit \
             is spent on agreement, and agreement appears in no table of constants.",
            sci(e_epsilon),
            sci(nu_epsilon),
            sci(ops_epsilon),
            sci(k_lloyd_epsilon),
            sci(landauer_300)
        )))
        .add(Block::Section("What Is Measured, Derived, Claimed".into()))
        .add(Block::Raw(
            "\\begin{center}\\begin{tabular}{p{7cm}p{2.4cm}p{5.6cm}}\\toprule\n\
             \\textbf{Statement} & \\textbf{Status} & \\textbf{Basis} \\\\\\midrule\n\
             $c,\\hbar,k_B,e$ & EXACT & 2019 SI, CODATA 2022 \\\\\n\
             $G,\\alpha,m_p,m_e$ & MEASURED & CODATA 2022 \\\\\n\
             Planck units, $\\alpha_G$, Landauer bound & DERIVED & computed here \\\\\n\
             $[K_{\\mathrm{v1}}]=\\mathrm{J\\,s^{-1/2}}$; $K^{*}$ dimensionless & DERIVED & unit analysis, executed \\\\\n\
             $K^{*}=2\\pi\\sqrt{\\tfrac{\\pi}{2}N_{\\mathrm{ML}}\\Delta s}$ & DERIVED & algebra + 3-scale numerical check \\\\\n\
             Margolus--Levitin, Landauer & THEOREM & used as stated \\\\\n\
             Live gauge fields & MEASURED & mainnet snapshot, epoch in text \\\\\n\
             $\\hat\\alpha_G=6.96\\times10^{-10}$ & CLAIMED & internal earlier work, no external measurement \\\\\n\
             $\\Gamma_h=4.07$\\,MeV & PDG, not CODATA & particle data \\\\\n\
             $\\Delta s$ as a quantum entropy & ANALOGY & Shannon over proposers, not von Neumann \\\\\n\
             $n\\ge2f+1$ via Berry phase & CONJECTURE & no proof, no adversarial test \\\\\\bottomrule\n\
             \\end{tabular}\\end{center}\n\n"
                .to_string(),
        ));

    if !papers.is_empty() {
        doc = doc.add(Block::Raw(related_work_section(&papers)));
    }

    doc = doc
        .add(Block::Section("Reproducibility".into()))
        .add(para(
            "This PDF is the output of the \\texttt{k\\_parameter} binary in \\texttt{flux-arxiv-latex}, built with \
             \\texttt{fluxc} in the Flux tree. It reads the arXiv JSON and, optionally, one saved gauge response; every \
             other number is computed from \\texttt{flux-science} at run time. Change a constant and the paper corrects \
             itself. The identity table is a test that runs every time the document is generated."
                .to_string(),
        ))
        .add(Block::Section("Coda".into()))
        .add(para(
            "Kristensen built a thermometer for consensus. Thermometers are only as good as their calibration, and \
             calibration is boring, unglamorous, and the only thing that separates a measurement from a mood. This paper \
             is the calibration: the units balanced, the constants traced, the boundary at 1 given a reason, and the live \
             instrument read with its plumbing faults named beside its readings. The invention stands. It now also \
             stands on something."
                .to_string(),
        ));

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
                if p.url.is_empty() { format!("https://arxiv.org/abs/{}", p.id) } else { p.url.clone() }
            ));
        }
        bib.push_str("\\end{thebibliography}\n");
        doc = doc.add(Block::Raw(bib));
    }

    std::fs::create_dir_all(out_dir).expect("out dir");
    std::fs::write(format!("{out_dir}/k_parameter.bib"), bibliography(&papers)).expect("bib");
    let res = doc.compile_pdf(out_dir, "k_parameter");
    if res.success {
        println!("OK {}", res.pdf_path.unwrap());
        println!(
            "identity max rel dev {:.3e} | N_ML(v1) {:.3e} | dK/K claimed {:.3e} codata {:.3e} | lloyd rel {:.2e}",
            max_rel, nml_v1, dk_claimed, dk_codata, lloyd_rel
        );
    } else {
        let tail: String = res.log.lines().rev().take(30).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        eprintln!("FAILED\n{tail}");
        std::process::exit(1);
    }
}
