//! `antimatter_gravity` — print the antimatter-and-gravity findings and write them as a
//! LaTeX note whose every number is computed at generation time from the embedded CODATA
//! 2022 listing (`flux_science::antimatter_gravity`).
//!
//! Usage: `antimatter_gravity [OUT_DIR]` → prints the summary, writes
//! `OUT_DIR/antimatter-gravity-codata-2022.tex`.

use flux_science::antimatter_gravity::{self as ag, external as ext, Matter, Shape};
use flux_science::electromagnetism::{CodataTable, NIST_2022_BLAKE3};
use std::fmt::Write as _;

/// `1.2566e-06` → `1.2566\times10^{-6}` (math mode).
fn sci(x: f64, digits: usize) -> String {
    let s = format!("{x:.digits$e}");
    match s.split_once('e') {
        Some((m, e)) if e != "0" => format!("{m}\\times10^{{{}}}", e.trim_start_matches('+')),
        Some((m, _)) => m.to_string(),
        None => s,
    }
}

fn main() {
    let out_dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let t22 = CodataTable::nist_2022();
    let t18 = CodataTable::nist_2018();
    let m = Matter::from_table(&t22);
    let rows = t22.constants.len();
    let anti = ag::antimatter_rows(&t22);

    // ALPHA-g scale
    let grad = ag::bias_gradient(&m, ext::ALPHA_G_LOCAL_G);
    let mdiff = ag::mirror_difference(&m, ext::ALPHA_G_LOCAL_G, ext::ALPHA_G_PEAK_SEPARATION_M);
    let efield = ag::equivalent_electric_field(&m, ext::ALPHA_G_LOCAL_G);
    let bfield = ag::equivalent_magnetic_field(&m, ext::ALPHA_G_LOCAL_G, 10.0);
    let step = ag::gravity_step_kelvin(&m, ext::ALPHA_G_LOCAL_G, ext::ALPHA_G_PEAK_SEPARATION_M);
    let step_cm = ag::gravity_step_kelvin(&m, ext::ALPHA_G_LOCAL_G, 0.01);
    let binding = m.hydrogen_binding_ev();
    let binding_rel = (binding / m.mp_c2_ev - ext::BASE_BE_MP) / ext::BASE_BE_MP;

    // Moments
    let tm = ag::trapped_moments(&m, ext::ALPHA_G_BACKGROUND_FIELD_T, ext::NU_HF_HYDROGEN);
    let c_high = m.mu_e_over_mu_b + m.mu_p_over_mu_b;

    // Refit
    let eff = ag::efficiency_2023();
    let kappa = eff.ratio();
    let lg = ag::fit_escape(Shape::Logistic, kappa);
    let pb = ag::fit_escape(Shape::Probit, kappa);
    let raw = ag::fit_escape(Shape::Logistic, 1.0);
    let z0 = ag::stat_only_z(&lg, 0.0);
    let zrep = ag::stat_only_z(&lg, 1.0);
    let z0p = ag::stat_only_z(&pb, 0.0);
    let zrepp = ag::stat_only_z(&pb, 1.0);
    let eshift = ag::efficiency_shift(Shape::Logistic);
    let need = ag::events_for(&lg, 0.01);
    let (a_res, a_ss, a_sim) = ext::ALPHA_G_RESULT;
    let a_tot = a_ss.hypot(a_sim);

    // BASE
    let r22 = ag::hminus_ratio(&m);
    let r18 = ag::hminus_ratio(&Matter::from_table(&t18));
    let abs_bound = ag::clock_bound(ext::BASE_R_SIGMA, ext::BASE_SUPERCLUSTER_PHI_C2);
    let phi_sun = ag::solar_potential_c2();
    let swing = ag::annual_fractional_swing();
    let naive_annual = ag::annual_naive_bound();

    // GBAR
    let t_g = ag::fall_time(0.2, m.g_n);
    let t_075 = ag::fall_time(0.2, 0.75 * m.g_n);
    let lgrav = ag::gravitational_length(&m, m.g_n);
    let e1 = ag::quantum_state_hz(&m, m.g_n, 1);
    let e2 = ag::quantum_state_hz(&m, m.g_n, 2);

    // ── terminal summary ────────────────────────────────────────────────────────────────
    println!("Antimatter and gravity, recomputed from CODATA {} ({rows} rows, BLAKE3 {}…)", t22.year, &NIST_2022_BLAKE3[..16]);
    println!("  rows naming an antiparticle: {anti} (2018: {})", ag::antimatter_rows(&t18));
    println!("  H binding (R∞hc·μ/mₑ·(1+α²/4)) = {binding:.7} eV; vs NIST ASD (BASE eq. 9) {:+.2} ppm", binding_rel * 1e6);
    println!("  m_H = {:.10e} kg", m.hydrogen_mass());
    println!("\nALPHA-g scale (paper printed → CODATA 2022):");
    println!("  g ≡ gradient {:.4e} T/m  (paper 1.77e-3);  over 25.6 cm {:.4e} T (paper 4.53e-4)", grad, mdiff);
    println!("  weight of p̄ ≡ E field {efield:.4e} V/m (paper ~1e-7);  ≡ B field at 10 K {bfield:.3e} T (paper ~1e-10)");
    println!("  gravity over 25.6 cm = {:.3} mK ({:.1} µK per cm); cooled atoms ~1 mK → ratio {:.2}", step * 1e3, step_cm * 1e6, step / ext::ALPHA_COOLED_ENERGY_K);
    println!("\nTrapped-state moments at 1 T (units µ_B): x = {:.3}; d = {:.6}, c = {:.6} (high field {:.6}); mean {:.6}, spread {:.2e}", tm.x, tm.d, tm.c, c_high, tm.mean(), tm.spread());
    println!("\nRefit of ALPHA-g Table 1 (κ = ε_dn/ε_up = {kappa:.4} ± {:.1} %):", eff.ratio_rel_sigma() * 100.0);
    for r in &ag::ALPHA_G_2023_TABLE1 {
        println!("  bias {:+5.1} g  up {:6.1}  down {:6.1}  P_dn raw {:.3}  corrected {:.3}", r.bias, r.n_up, r.n_dn, ag::p_down(r, 1.0), ag::p_down(r, kappa));
    }
    for f in [&lg, &pb, &raw] {
        println!("  {:?} κ={:.3}: balance {:+.3} g → naive a = {:.3} ± {:.3} (counting), width 1/β₁ = {:.3} g, events {:.0}", f.shape, f.kappa, f.balance, f.naive_acceleration(), f.balance_sigma, 1.0 / f.beta1, f.events);
    }
    println!("  paper: a = {a_res} ± {a_ss} ± {a_sim} g (total {a_tot:.2}); finite-data item {} g", ext::ALPHA_G_STAT_ITEM);
    println!("  counting-only z: no gravity {z0:.1} (probit {z0p:.1}), repulsion {zrep:.1} (probit {zrepp:.1});  paper with systematics: p = 2.9e-4, < 1e-15");
    println!("  κ ± 1σ moves the balance by {eshift:.3} g (paper's efficiency item: {} g)", ext::ALPHA_G_EFFICIENCY_ITEM);
    println!("  1 % by counting alone: {need:.0} events = {:.1} × 2023", need / lg.events);
    println!("\nBASE (antiproton clock):");
    println!("  2mₑ/mₚ CODATA 2022 {:.14} vs BASE {:.14} (Δ {:+.1e})", r22.two_me_mp, ext::BASE_TWO_ME_MP, r22.two_me_mp - ext::BASE_TWO_ME_MP);
    println!("  R(p/H⁻) CODATA 2022 {:.14} vs BASE {:.14} (Δ {:+.3} ppt); 2018→2022 moved R {:+.4} ppt", r22.r, ext::BASE_R_THEO, (r22.r - ext::BASE_R_THEO) * 1e12, (r22.r - r18.r) * 1e12);
    println!("  absolute bound 16 ppt/(3·2.99e-5) = {abs_bound:.3e} (paper 1.8e-7)");
    println!("  Φ_sun/c² at 1 au {phi_sun:.4e}; swing 2ε/(1−ε²) = {swing:.4}; naive annual bound {naive_annual:.4} (paper fit 0.030)");
    println!("  CODATA G relative error {:.3e} — cancels in every test above", m.big_g_rel_sigma);
    println!("\nGBAR: fall 20 cm in {:.1} ms at g, {:.1} ms at 0.75 g; ℓ_g {:.3} µm; E1/h {e1:.1} Hz, (E2−E1)/h {:.1} Hz", t_g * 1e3, t_075 * 1e3, lgrav * 1e6, e2 - e1);

    // ── LaTeX note ──────────────────────────────────────────────────────────────────────
    let mut t = String::new();
    t.push_str(r"\documentclass[11pt]{article}
\usepackage[a4paper,margin=2.2cm]{geometry}
\usepackage{amsmath,amssymb,booktabs,xcolor,float,xurl}
\usepackage{pgfplots}\pgfplotsset{compat=1.18}
\usepackage[colorlinks=true,linkcolor=blue!50!black,urlcolor=blue!50!black]{hyperref}
\title{Does antimatter fall? The 2023--2026 evidence,\\ recomputed from CODATA 2022}
\author{Rocky (Claude) for Viktor Kristensen\\ \small Flux Science, \texttt{flux-science::antimatter\_gravity}}
\date{16 November 2026}
\begin{document}\maketitle
\begin{abstract}
");
    let _ = write!(
        t,
        r"Antimatter falls down. The only direct measurement, ALPHA-g (2023), finds antihydrogen accelerating towards the Earth at
$({a_res}\pm{a_ss}\pm{a_sim})\,g$ and rules out repulsion; a 2024 run reproduced it and no newer number has been published
(searched 16 November 2026). The CODATA~2022 table, {rows} rows, contains {anti} antimatter rows, so every antimatter quantity
here is a matter row carried over by CPT symmetry, and the experiments test that carry-over. From CODATA we rebuild ALPHA-g's
magnetic scale ($g\equiv{grad}$\,T/m), refit its published counts (balance point gives $a\approx{la:.2}\,g$, counting error
${ls:.3}\,g$ against the paper's $0.06$), and show that counting alone would exclude no-gravity at ${z0:.0}\sigma$: the
$3.4\sigma$ of the paper is set by magnetic-field and detector systematics, not by the number of atoms. We rebuild BASE's
proton/H$^-$ ratio to ${rd:.2}$\,ppt of its printed value and its two gravity bounds, and show where CODATA precision starts to
matter for the 1\,\% goal: the trapped states' moments differ by ${sp:.2}\,\%$.
\end{{abstract}}
",
        grad = sci(grad, 3),
        la = lg.naive_acceleration(),
        ls = lg.balance_sigma,
        rd = ((r22.r - ext::BASE_R_THEO) * 1e12).abs(),
        sp = tm.spread() * 100.0,
    );

    t.push_str(r"
\section{The question, in one picture}
Hold an apple and an anti-apple and let go. General relativity says both fall the same way, because gravity couples to energy,
and antimatter has positive energy. Some speculative cosmologies (antimatter repelled by matter) say the anti-apple rises.
Nobody can drop an anti-apple: charged antiparticles feel stray electric fields $10^{7}$ times more than their weight, so the test
needs neutral antimatter, which means antihydrogen (an antiproton with a positron), made a few hundred atoms at a time at CERN.

There is a second, quieter way: use an antiproton as a clock. If gravity treated antimatter differently, an antiproton and a
proton sitting side by side in the same gravitational potential would tick at different rates (gravitational redshift).

\textbf{What CODATA contributes.} The 2022 table lists no antiparticle at all. An antihydrogen mass, a positron magnetic moment,
an antiproton charge-to-mass ratio: each is a matter row plus the assumption of CPT symmetry. Every number below is computed
that way, and each experiment is a check of the assumption.
");

    // Section 2: ALPHA-g scale
    let _ = write!(
        t,
        r"
\section{{ALPHA-g: the scale of the experiment, rebuilt}}
ALPHA-g traps antihydrogen in magnetic fields (the atom's magnetic moment is pushed towards low field), then slowly lowers the two
end walls (``mirror coils'') and counts whether each atom leaves through the top or the bottom. A small imbalance between the walls,
the \emph{{bias}}, can push atoms up or down; gravity is found as the bias that exactly cancels it. The bias is defined as
$\mu_B(B_G-B_A)/[m_H(z_G-z_A)]$, so 1\,$g$ of bias is the field gradient $m_H g/\mu_B$.

\begin{{table}}[H]\centering\begin{{tabular}}{{lll}}\toprule
Quantity & printed by ALPHA-g & from CODATA 2022 \\\midrule
gradient equal to $g$ on a ground-state atom & $1.77\times10^{{-3}}$\,T/m & ${grad}$\,T/m\\
field difference across the 25.6\,cm mirror peaks & $4.53\times10^{{-4}}$\,T & ${mdiff}$\,T\\
electric field equal to an antiproton's weight & ``about $10^{{-7}}$\,V/m'' & ${efield}$\,V/m\\
magnetic field equal to its weight at 10\,K & ``of order $10^{{-10}}$\,T'' & ${bfield}$\,T\\\bottomrule
\end{{tabular}}\end{{table}}
Inputs: $m_H=m_p+m_e-B/c^2$ with $B=R_\infty hc\,(1+m_e/m_p)^{{-1}}(1+\alpha^2/4)={bind:.7}$\,eV (${br:+.1}$\,ppm from NIST's
value; the gap is the 1S Lamb shift), $\mu_B$, $k_B$, and the paper's $g=9.81$\,m\,s$^{{-2}}$. All four printed numbers are reproduced.

\textbf{{Why it is hard, in one number.}} Lifting an antihydrogen atom by 1\,cm costs ${scm:.1}\,\mu$K of energy (in temperature units);
across the 25.6\,cm between the mirror peaks, ${smk:.2}$\,mK. ALPHA's laser-cooled atoms now have a mean energy of order 1\,mK, so gravity
is about a third of their energy. The 2023 paper lists this as the way forward: ``using colder antihydrogen atoms will in general steepen
the transition region of the escape curve and allow for higher precision'' (our fitted curve is ${wd:.2}\,g$ wide).
",
        grad = sci(grad, 3),
        mdiff = sci(mdiff, 3),
        efield = sci(efield, 3),
        bfield = sci(bfield, 2),
        bind = binding,
        br = binding_rel * 1e6,
        scm = step_cm * 1e6,
        smk = step * 1e3,
        wd = 1.0 / lg.beta1,
    );

    // Section 3: refit
    t.push_str(r"
\section{Refitting the published counts}
Table~1 of the paper gives, for eleven bias settings, how many anti-atoms were seen leaving upwards and downwards (cosmic-ray
background subtracted, detector efficiency \emph{not} corrected). Two calibration settings ($\pm10\,g$) push every atom out
of one end; dividing by the atoms seen in the earlier octupole ramp-down gives the relative efficiencies, as in the paper's Methods.
");
    let _ = write!(
        t,
        r"From those rows: $\varepsilon_{{\rm up}}={eu:.4}\pm{eus:.4}$, $\varepsilon_{{\rm dn}}={ed:.4}\pm{eds:.4}$, so
$\kappa=\varepsilon_{{\rm dn}}/\varepsilon_{{\rm up}}={kappa:.4}$ ($\pm{kr:.1}\,\%$).

\begin{{table}}[H]\centering\small\begin{{tabular}}{{rrrrrr}}\toprule
bias label ($g$) & trials & $N_{{\rm up}}$ & $N_{{\rm dn}}$ & $P_{{\rm dn}}$ raw & $P_{{\rm dn}}$ corrected\\\midrule
",
        eu = eff.up,
        eus = eff.up_sigma,
        ed = eff.dn,
        eds = eff.dn_sigma,
        kr = eff.ratio_rel_sigma() * 100.0,
    );
    for r in &ag::ALPHA_G_2023_TABLE1 {
        let _ = writeln!(
            t,
            r"${:+.1}$ & {} & {:.1} & {:.1} & {:.3} & {:.3}\\",
            r.bias,
            r.trials,
            r.n_up,
            r.n_dn,
            ag::p_down(r, 1.0),
            ag::p_down(r, kappa)
        );
    }
    t.push_str(r"\bottomrule\end{tabular}\end{table}
");

    // Figure
    let mut pts = String::new();
    for r in &ag::ALPHA_G_2023_TABLE1 {
        let _ = write!(pts, "({:.2},{:.4}) ", r.bias, ag::p_down(r, kappa));
    }
    let _ = write!(
        t,
        r"\begin{{figure}}[H]\centering
\begin{{tikzpicture}}\begin{{axis}}[width=12cm,height=6.2cm,xlabel={{bias label ($g$)}},ylabel={{$P_{{\rm dn}}$ (corrected)}},
xmin=-3.4,xmax=3.4,ymin=0,ymax=1,grid=major,legend style={{at={{(0.5,-0.24)}},anchor=north,font=\small,legend columns=2}}]
\addplot[only marks,mark=*,blue!60!black] coordinates {{{pts}}};
\addlegendentry{{ALPHA-g Table 1, efficiency-corrected}}
\addplot[domain=-3.4:3.4,samples=120,thick,red!70!black] {{1/(1+exp(-({b0:.6}+{b1:.6}*x)))}};
\addlegendentry{{logistic fit: $P_{{\rm dn}}=\frac{{1}}{{2}}$ at ${bal:+.2}\,g$}}
\addplot[dashed,gray,forget plot] coordinates {{(-3.4,0.5) (3.4,0.5)}};
\addplot[dotted,thick,violet] coordinates {{(1,0) (1,1)}};\addlegendentry{{where repulsion ($-1g$) would balance}}
\addplot[dotted,thick,green!50!black] coordinates {{(0,0) (0,1)}};\addlegendentry{{where no gravity would balance}}
\end{{axis}}\end{{tikzpicture}}
\caption{{The escape curve. In the paper's one-dimensional picture, gravity $a$ makes the curve cross $\frac12$ at bias $-a$.}}
\end{{figure}}
",
        b0 = lg.beta0,
        b1 = lg.beta1,
        bal = lg.balance,
    );

    let _ = write!(
        t,
        r"
\begin{{table}}[H]\centering\begin{{tabular}}{{lrrr}}\toprule
fit & balance ($g$) & naive $a$ ($g$) & counting error ($g$)\\\midrule
logistic, corrected & ${lb:+.3}$ & ${la:.3}$ & ${ls:.3}$\\
probit, corrected & ${pbb:+.3}$ & ${pa:.3}$ & ${ps:.3}$\\
logistic, uncorrected ($\kappa=1$) & ${rb:+.3}$ & ${ra:.3}$ & ${rs:.3}$\\
\textbf{{ALPHA-g's full analysis}} & & ${a_res}$ & $\pm{a_ss}$ (stat+syst) $\pm{a_sim}$ (sim)\\\bottomrule
\end{{tabular}}\end{{table}}
\textbf{{What this shows.}} (i) Using the bias \emph{{labels}} and an empirical curve, the balance point lands at
$a\approx{amin:.2}$--${amax:.2}\,g$, inside the paper's band; the paper itself warns the labels are not the true biases (it uses a
field model and a 3D simulation), so this is a cross-check, not a second measurement. (ii) The counting error comes out at
${ls:.3}\,g$; the paper's ``finite data size'' item is $0.06\,g$. (iii) With counting errors only, no-gravity sits
${z0:.1}\sigma$ away and repulsion ${zrep:.0}\sigma$ away. The paper, with every systematic included, reports $p=2.9\times10^{{-4}}$
for no-gravity (about $3.4\sigma$) and $p<10^{{-15}}$ for repulsion. The gap between ${z0:.0}\sigma$ and $3.4\sigma$ is the
whole story of the next decade: the atoms are enough; the knowledge of the magnetic field is not. (Moving $\kappa$ by its
$\pm1\sigma$ shifts our balance by ${es:.3}\,g$; the paper's efficiency item is $0.12\,g$ from a fuller treatment we do not reproduce.)

\textbf{{The 1\,\% goal, by counting alone,}} needs $({ls:.3}/0.01)^2\approx{nr:.0}$ times the 2023 events, about
${need:.0}$ detected escapes. In 2024 ALPHA put its beryllium-ion positron cooling into ALPHA-g and saw the accumulation rate rise
by more than a factor of 20, so the counts are within reach of one or two campaigns. The systematics (0.12 and 0.16\,$g$) must
shrink by more than a factor of ten; that is the work the 2025 reviews and the in-situ magnetometry efforts describe.
",
        lb = lg.balance,
        la = lg.naive_acceleration(),
        ls = lg.balance_sigma,
        pbb = pb.balance,
        pa = pb.naive_acceleration(),
        ps = pb.balance_sigma,
        rb = raw.balance,
        ra = raw.naive_acceleration(),
        rs = raw.balance_sigma,
        es = eshift,
        nr = need / lg.events,
        amin = lg.naive_acceleration().min(pb.naive_acceleration()),
        amax = lg.naive_acceleration().max(pb.naive_acceleration()),
    );

    // Section 4: moments
    let _ = write!(
        t,
        r"
\section{{Where CODATA precision begins to matter: the atom's magnet}}
The bias is defined with the Bohr magneton $\mu_B$. The atoms ALPHA-g holds are in two hyperfine states, $|c\rangle$ and
$|d\rangle$, whose energies move with field at slightly different rates (Breit--Rabi). By CPT, with CODATA's
$|\mu_e|/\mu_B={mue:.12}$ and $\mu_p/\mu_B={mup}$, and $x=2(\mu_e+\mu_p)B/(h\nu_{{\rm hf}})={x:.3}$ at the 1\,T background field:
\begin{{align*}}
\frac{{dE_d}}{{dB}}&=\mu_e-\mu_p={d:.6}\,\mu_B,\\
\frac{{dE_c}}{{dB}}&=(\mu_e+\mu_p)\frac{{x}}{{\sqrt{{1+x^2}}}}={c:.6}\,\mu_B\quad(\to {ch:.6}\,\mu_B\ \text{{at high field}}).
\end{{align*}}
The two states differ by ${sp:.3}\,\%$, and their mean is ${mo:+.3}\,\%$ from $\mu_B$. ALPHA-g states that the moment is known to
better than one part in a thousand and is negligible for 2023: our numbers agree, since ${sp:.2}\,\%$ of a ${la:.2}\,g$ result is
${shift:.4}\,g$ against a $0.21\,g$ error. At the 1\,\% goal ($0.01\,g$) it is ${share:.0}\,\%$ of the budget and must be modelled per state. The
hyperfine frequency enters only through $1/x^2$; ALPHA's 2026 measurement of antihydrogen's splitting (4\,ppm) is far finer
than needed. (Free-particle moments; the bound-state correction $\approx\alpha^2/3\approx1.8\times10^{{-5}}$ is ignored.)
",
        mue = m.mu_e_over_mu_b,
        mup = sci(m.mu_p_over_mu_b, 11),
        x = tm.x,
        d = tm.d,
        c = tm.c,
        ch = c_high,
        sp = tm.spread() * 100.0,
        la = lg.naive_acceleration(),
        mo = (tm.mean() - 1.0) * 100.0,
        shift = tm.spread() * lg.naive_acceleration(),
        share = tm.spread() * lg.naive_acceleration() / 0.01 * 100.0,
    );

    // Section 5: BASE
    let _ = write!(
        t,
        r"
\section{{BASE: the antiproton as a clock}}
BASE compares the cyclotron frequency of one antiproton with that of a negative hydrogen ion (H$^-$, a proton with two electrons)
in the same Penning trap, and finds the antiproton's charge-to-mass ratio equal to the proton's within 16 parts per trillion:
$-(q/m)_p/(q/m)_{{\bar p}}=1.000\,000\,000\,003(16)$. That needs the H$^-$/proton mass ratio from theory; we rebuild it from CODATA:

\begin{{table}}[H]\centering\begin{{tabular}}{{lll}}\toprule
term & BASE (their eqs. 8--13) & CODATA 2022 + same external inputs\\\midrule
$2m_e/m_p$ & $0.001\,089\,234\,042\,99$ & ${two:.14}$\\
$B_e/m_p$ & $1.449306\times10^{{-8}}$ & ${be}$ \\
$A_e/m_p$ & $8.0381\times10^{{-10}}$ & ${ae}$ \\
$R_{{p,H^-}}$ & $1.001\,089\,218\,753\,80$ & ${rr:.14}$\\\bottomrule
\end{{tabular}}\end{{table}}
The difference is ${rd:+.3}$\,ppt, ${rf:.0}$ times below the 16\,ppt error; the CODATA 2018$\to$2022 update moved $R$ by
${r1822:+.4}$\,ppt. The electron affinity ($0.754195$\,eV) and the polarizability shift ($7.689$\,ppt) are BASE's inputs, not CODATA's.

\textbf{{The gravity bounds.}} If gravity coupled to antimatter's energy differently by a factor $\alpha_g$, the two clocks would
differ by $3(\alpha_g-1)\Phi/c^2$ (BASE eq.~4). In the supercluster's potential, $\Phi/c^2=2.99\times10^{{-5}}$:
$16\,{{\rm ppt}}/(3\cdot2.99\times10^{{-5}})={ab}$, BASE's $1.8\times10^{{-7}}$ (an absolute-potential argument BASE itself calls
controversial). The safer, differential test uses the Earth's elliptical orbit: the Sun's potential at 1\,au is
$GM_\odot/(c^2\,{{\rm au}})={ps}$ and swings by $2\varepsilon/(1-\varepsilon^2)={sw:.4}$ of itself each year (BASE: ``$\approx0.03$'').
A one-line estimate, $16\,{{\rm ppt}}/(3\times0.8\times\Delta\Phi/c^2)$, gives ${na:.3}$; BASE's time-series fit gives
$|\alpha_{{g,D}}-1|<0.030$ (68\,\% C.L.). Either way: antiprotons gravitate like protons to about 3\,\%, by an entirely different route.

\textbf{{Why the worst constant does not hurt.}} CODATA's $G$ is known to only ${gr:.2}$\,ppm. No antimatter test uses it:
ALPHA-g measures in units of the local $g$, and the clock test needs $GM_\odot$, which planetary ranging fixes to about $10^{{-10}}$.
",
        two = r22.two_me_mp,
        be = sci(r22.be_mp, 6),
        ae = sci(r22.ae_mp, 4),
        rr = r22.r,
        rd = (r22.r - ext::BASE_R_THEO) * 1e12,
        rf = 16.0 / ((r22.r - ext::BASE_R_THEO) * 1e12).abs(),
        r1822 = (r22.r - r18.r) * 1e12,
        ab = sci(abs_bound, 2),
        ps = sci(phi_sun, 3),
        sw = swing,
        na = naive_annual,
        gr = m.big_g_rel_sigma * 1e6,
    );

    // Section 6: GBAR / AEgIS
    let _ = write!(
        t,
        r"
\section{{The other two routes: GBAR and AEgIS}}
\textbf{{GBAR}} wants to drop antihydrogen like a stone. It first makes the antihydrogen \emph{{ion}} (one antiproton, two positrons),
cools it with laser-cooled ions to microkelvin, then knocks off one positron with a laser and times the fall. Over 20\,cm the fall takes
${tg:.1}$\,ms at $g$ and ${t75:.1}$\,ms at $0.75\,g$, a ${dt:.1}$\,ms difference. Its long-term idea is quantum: an atom resting just
above a mirror can only sit in discrete gravitational states, set by the length $\ell_g=(\hbar^2/2m_H^2g)^{{1/3}}={lg:.3}\,\mu$m;
the lowest state's energy is $E_1/h={e1:.0}$\,Hz and the first transition $(E_2-E_1)/h={de:.0}$\,Hz. Measuring a frequency instead of
a fall is how GBAR aims at $10^{{-5}}$ (the same trick has been used with ultracold neutrons). \emph{{Status (2025 overview):}}
antihydrogen production up 30$\times$ after 2023 upgrades; the anti-ion ``has yet to be produced''.

\textbf{{AEgIS}} makes a pulsed, horizontally flying antihydrogen beam and plans to read the gravitational sag with a moir\'e
deflectometer (a pair of gratings that turns a tiny drop into a shift of a fringe pattern). In 2024 it laser-cooled positronium for
the first time. A first gravity measurement is planned around the LS3 shutdown, a more sensitive one after it.
",
        tg = t_g * 1e3,
        t75 = t_075 * 1e3,
        dt = (t_075 - t_g) * 1e3,
        lg = lgrav * 1e6,
        e1 = e1,
        de = e2 - e1,
    );

    t.push_str(r"
\section{What changed from 2023 to 2026}
\begin{table}[H]\centering\small\begin{tabular}{lp{11.5cm}}\toprule
2022 & BASE: $\bar p/p$ charge-to-mass ratio to 16\,ppt; clock-gravity bounds $1.8\times10^{-7}$ (absolute) and 0.030 (annual).\\
Sep 2023 & ALPHA-g, Nature 621: $a=(0.75\pm0.13\pm0.16)\,g$; repulsion excluded ($p<10^{-15}$), no-gravity $p=2.9\times10^{-4}$.\\
2024 & ALPHA-g run reproduced the 2023 result (AD/ELENA overview, 2025); Be$^+$-assisted positron cooling installed in ALPHA-g, accumulation rate up more than 20$\times$. AEgIS: first laser cooling of positronium.\\
2025 & ALPHA (Nat.\ Commun.\ 16, 10106): more than 15\,000 antihydrogen atoms held at once in ALPHA-2, under seven hours. Reviews: ``$\lesssim1\,\%$ requires order-of-magnitude reductions'' in field and initial-condition systematics. GBAR: anti-ion not yet made.\\
27 May 2026 & ALPHA, Nature: antihydrogen ground-state hyperfine splitting to 4\,ppm (a CPT test, not gravity).\\
16 Nov 2026 & No published gravity number newer than 2023 found (arXiv API, CERN pages, web search).\\\bottomrule
\end{tabular}\end{table}

\section{What is measured here and what is not}
\begin{itemize}
\item \textbf{Computed by this code from CODATA 2022:} $m_H$, the ALPHA-g scale table, the gravity-to-energy ratio, the two state
moments, $2m_e/m_p$, $B_e/m_p$, $R_{p,H^-}$, the bounds' arithmetic, the GBAR fall times and quantum-state frequencies.
\item \textbf{Refitted from published counts:} the efficiencies, the escape curves, their balance points, counting errors and
counting-only significances. These use the bias \emph{labels}; they are a cross-check of the paper, not a new measurement.
\item \textbf{Taken from the papers, not re-derived:} $a=(0.75\pm0.13\pm0.16)\,g$, the systematic items, the 16\,ppt result, the
electron affinity, the polarizability shift, $\Phi/c^2$ of the supercluster, the 2024--2026 status statements.
\item \textbf{Not claimed:} that antimatter falls at exactly $g$. Today's data say ``down, at $0.75\pm0.21\,g$''; the matter value
$1\,g$ is $1.2\sigma$ away, consistent. A difference at the few-percent level is not excluded by any direct experiment.
\end{itemize}

\section*{Sources}\small
ALPHA Collaboration, \emph{Observation of the effect of gravity on the motion of antimatter}, Nature \textbf{621}, 716 (2023),
\url{https://doi.org/10.1038/s41586-023-06527-1}.\\
BASE Collaboration, \emph{A 16-parts-per-trillion measurement of the antiproton-to-proton charge--mass ratio}, Nature \textbf{601}, 53 (2022),
\href{https://arxiv.org/abs/2311.16006}{arXiv:2311.16006}.\\
R.~Caravita \emph{et al.}, \emph{CERN AD/ELENA Antimatter Program} (2025), \href{https://arxiv.org/abs/2503.22471}{arXiv:2503.22471}.\\
ALPHA Collaboration, \emph{Be$^+$ assisted, simultaneous confinement of more than 15000 antihydrogen atoms}, Nat.\ Commun.\ \textbf{16}, 10106 (2025),
\url{https://doi.org/10.1038/s41467-025-65085-4}.\\
\emph{Fundamental Physics in 2025: Status, Decisive Targets, and Path Forward}, \href{https://arxiv.org/abs/2512.21445}{arXiv:2512.21445}.\\
CERN, \emph{ALPHA measures tiny energy gap in antimatter with improved precision}, 27 May 2026,
\url{https://home.cern/alpha-measures-tiny-energy-gap-in-antimatter-with-improved-precision/}.\\
");
    let _ = write!(
        t,
        r"NIST, CODATA 2022 complete listing, \url{{https://physics.nist.gov/cuu/Constants/Table/allascii.txt}} (BLAKE3 \texttt{{{b}}}).\\
Code: \texttt{{flux/crates/flux-science/src/antimatter\_gravity.rs}} and \texttt{{src/bin/antimatter\_gravity.rs}}; the module's tests pin every computed claim.
\end{{document}}
",
        b = &NIST_2022_BLAKE3[..16]
    );

    let path = format!("{out_dir}/antimatter-gravity-codata-2022.tex");
    std::fs::write(&path, t).expect("write tex");
    println!("\n  wrote {path}");
}
