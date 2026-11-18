//! `codata_em` — print the CODATA 2022 electromagnetism findings and write them as a
//! LaTeX note whose every number is computed at generation time from the embedded NIST
//! listings (`flux_science::electromagnetism`).
//!
//! Usage: `codata_em [OUT_DIR]` → prints the summary, writes `OUT_DIR/codata-em-2022.tex`.

use flux_science::electromagnetism::{self as em, si2019, CodataTable};
use std::fmt::Write as _;

/// `1.2566e-06` → `1.2566\times10^{-6}` (in math mode).
fn tex_sci(x: f64, digits: usize) -> String {
    let s = format!("{x:.digits$e}");
    match s.split_once('e') {
        Some((m, e)) if e != "0" => format!("{m}\\times10^{{{}}}", e.trim_start_matches('+')),
        Some((m, _)) => m.to_string(),
        None => s,
    }
}

/// Make plain text (names, methods) safe for LaTeX text mode.
fn tex_text(s: &str) -> String {
    s.replace('_', r"\_").replace('&', r"\&").replace('%', r"\%")
}

/// NIST unit strings (`Hz V^-1`, `m^-1`) for LaTeX text mode.
fn tex_unit(u: &str) -> String {
    let mut out = String::new();
    let mut chars = u.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '^' {
            let mut exp = String::new();
            while let Some(&d) = chars.peek() {
                if d == '-' || d.is_ascii_digit() {
                    exp.push(d);
                    chars.next();
                } else {
                    break;
                }
            }
            let _ = write!(out, "$^{{{exp}}}$");
        } else {
            out.push(c);
        }
    }
    out
}

/// `alpha^n R_inf^k (m_e/m_p)^j` with zero powers dropped and unit powers bare.
fn law_factors(n: i32, k: i32, j: i32) -> String {
    let pow = |base: &str, e: i32| match e {
        0 => String::new(),
        1 => format!("{{{base}}}"),
        _ => format!("{base}^{{{e}}}"),
    };
    let s = [pow(r"\alpha", n), pow(r"R_\infty", k), pow(r"(m_e/m_p)", j)].concat();
    if s.is_empty() { "1".into() } else { s }
}

fn tex_symbol(symbol: &str) -> &'static str {
    match symbol {
        "mu0" => r"\mu_0",
        "eps0" => r"\varepsilon_0",
        "Z0" => r"Z_0",
        "a0" => r"a_0",
        "lambda_C" => r"\lambda_{\rm C}",
        "r_e" => r"r_e",
        "sigma_e" => r"\sigma_e",
        "m_e" => r"m_e",
        "mu_B" => r"\mu_{\rm B}",
        "mu_N" => r"\mu_{\rm N}",
        "E_h" => r"E_{\rm h}",
        "E_au" => r"\mathcal{E}_{\rm au}",
        "g_e" => r"g_e",
        "mu_e" => r"\mu_e",
        _ => r"?",
    }
}

fn main() {
    let out_dir = std::env::args().nth(1).unwrap_or_else(|| "/home/storage/flux-scratch/codata-em-1113/out".into());
    std::fs::create_dir_all(&out_dir).expect("create output dir");

    let (t18, t22) = (CodataTable::nist_2018(), CodataTable::nist_2022());
    let a18 = t18.alpha_inverse();
    let a22 = t22.alpha_inverse();
    let alpha_shift_sigma = (a22.value - a18.value) / a18.sigma;
    let dla = em::ln_alpha_ratio(&t18, &t22);

    let checks = em::consistency(&t22);
    let max_z = checks.iter().map(|c| c.z.abs()).fold(0.0, f64::max);
    let exact = em::exact_checks(&t22);
    let shifts = em::alpha_fingerprint(&t18, &t22);
    let anomaly = em::anomaly_fingerprint(&t18, &t22);
    let raw = em::weighted_mean(&em::ALPHA_INPUTS_2022, 1.0);
    let expanded = em::weighted_mean(&em::ALPHA_INPUTS_2022, em::ALPHA_EXPANSION_FACTOR_2022);
    let birge_needed = raw.birge_ratio;
    let (d18, s18) = em::mu0_drift(&t18);
    let (d22, s22) = em::mu0_drift(&t22);
    let z0_over_rk = t22.must("characteristic impedance of vacuum").value / si2019::von_klitzing();
    let coulomb_k = 1.0 / (4.0 * std::f64::consts::PI * t22.must("vacuum electric permittivity").value);
    let rp18 = t18.must("proton rms charge radius");
    let rp22 = t22.must("proton rms charge radius");
    let amu22 = t22.must("muon mag. mom. anomaly");
    let fnal_total = em::muon_g2::FNAL_2025_STAT.hypot(em::muon_g2::FNAL_2025_SYST);
    let fnal_vs_codata = em::tension((em::muon_g2::FNAL_2025, fnal_total), (amu22.value, amu22.sigma));
    let inp = &em::ALPHA_INPUTS_2022;
    let pair = |i: usize, j: usize| em::tension((inp[i].alpha_inv, inp[i].sigma), (inp[j].alpha_inv, inp[j].sigma));

    // ── terminal summary ────────────────────────────────────────────────────────────────
    println!("CODATA {} vs {} — electromagnetism, recomputed from NIST's own listings", t22.year, t18.year);
    println!("  embedded tables: {} constants (2022), {} (2018), BLAKE3 pinned", t22.constants.len(), t18.constants.len());
    println!("  alpha^-1: {:.9} -> {:.9}  (moved {:.2} sigma_2018; d ln alpha = {:.4e})", a18.value, a22.value, alpha_shift_sigma, dla);
    println!("\n  alpha inputs (paper Sec. XV):");
    for (x, r) in inp.iter().zip(&raw.normalized_residuals) {
        println!("    {:<12} {:.9}({:>2.0})  residual {:+.2}  [{}]", x.label, x.alpha_inv, x.sigma * 1e9, r, x.method);
    }
    println!("    raw mean      {:.9}({:.0})  chi2 {:.1} / {} dof  Birge {:.2}  p {:.1e}", raw.mean, raw.sigma * 1e9, raw.chi2, raw.dof, raw.birge_ratio, em::chi2_p_dof2(raw.chi2));
    println!("    x2.5 mean     {:.9}({:.0})  chi2 {:.2}  p {:.3}   (paper: 137.035999178(21))", expanded.mean, expanded.sigma * 1e9, expanded.chi2, em::chi2_p_dof2(expanded.chi2));
    println!("    tensions: Rb-Cs {:.2} sigma, g2-Cs {:.2}, Rb-g2 {:.2}", pair(1, 0), pair(2, 0), pair(1, 2));
    println!("\n  fingerprint: each constant's 2018->2022 move in powers of alpha");
    for s in &shifts {
        println!("    {:<9} n_theory {:>2}  n_measured {:+.4} ± {:.4}  moved {:+6.2} sigma_2018  [{}]", s.symbol, s.n_theory, s.n_measured, s.bound, s.shift_in_old_sigma, s.nist_name);
    }
    println!("    a_e       measured {:.4} ± {:.4}; fixed QED {:.4}; with A1(10) 6.675->6.08: {:.4}", anomaly.n_measured, anomaly.bound, anomaly.n_fixed_theory, anomaly.n_with_a10_update);
    println!("\n  2022 consistency (derived from alpha^-1, R_inf, me/mp, a_e + exact e,h,c): max |z| = {max_z:.3}");
    for c in &checks {
        println!("    {:<9} table {:.11e}  derived {:.11e}  z {:+.3}  u_r/u_r(alpha) {:.2}", c.symbol, c.table, c.derived, c.z, c.rel_sigma / em::ALPHA_REL_SIGMA_2022_UNROUNDED);
    }
    println!("\n  mu0/(4pi e-7) - 1: 2018 {:+.1}({:.0})e-11 = {:+.1} sigma;  2022 {:+.1}({:.0})e-11 = {:+.1} sigma", d18 * 1e11, s18 * 1e11, d18 / s18, d22 * 1e11, s22 * 1e11, d22 / s22);
    println!("  Z0/R_K = {:.12} ; 2 alpha = {:.12}", z0_over_rk, 2.0 / a22.value);
    println!("  Coulomb k = 1/(4 pi eps0) = {coulomb_k:.10e} N m^2 C^-2");
    for x in &exact {
        println!("    exact {:<24} computed {:.12e}  printed {:.10e}...", x.nist_name, x.computed, x.printed);
    }
    println!("\n  proton radius: {:.4}({:.0}) fm -> {:.5}({:.0}) fm; electronic vs muonic (2022, Table XVI): {:.2} sigma", rp18.value * 1e15, rp18.sigma * 1e19, rp22.value * 1e15, rp22.sigma * 1e20, em::proton_radius_tension());
    println!("  muon a_mu: CODATA 2022 {:.8e}({:.0}), paper says {:.1} sigma vs theory; Fermilab 2025 {:.12e} is {:+.2} sigma from it; exp-WP25 = {:.1}e-11 = {:.2} sigma", amu22.value, amu22.sigma * 1e11, em::muon_g2::CODATA_2022_DISCREPANCY_SIGMA, em::muon_g2::FNAL_2025, fnal_vs_codata, (em::muon_g2::EXP_AVERAGE_2025.0 - em::muon_g2::SM_WP25.0) * 1e11, em::muon_g2::wp25_tension());

    // ── LaTeX note ──────────────────────────────────────────────────────────────────────
    let mut t = String::new();
    t.push_str(r"\documentclass[11pt]{article}
\usepackage[a4paper,margin=2.2cm]{geometry}
\usepackage{amsmath,amssymb,booktabs,xcolor,float,xurl}
\usepackage[colorlinks=true,linkcolor=blue!50!black,urlcolor=blue!50!black]{hyperref}
\setlength{\parskip}{0.45em}\setlength{\parindent}{0pt}
\title{\textbf{Electromagnetism after the 2019 SI}\\[0.3em]\large What CODATA 2022 changed, recomputed from NIST's own tables}
\author{Rocky (Claude) \quad\textperiodcentered\quad Flux Science, \texttt{flux-science::electromagnetism}}
\date{13 November 2026}
\begin{document}\maketitle
");
    let _ = write!(t, r"\begin{{abstract}}\noindent
Since 20 May 2019 the elementary charge $e$, the Planck constant $h$ and the speed of light $c$ are exact.
The magnetic constant $\mu_0$ therefore stopped being the defined number $4\pi\times10^{{-7}}$\,N\,A$^{{-2}}$ and became a
measurement, and with it $\varepsilon_0$ and $Z_0$. The whole electromagnetic block of the CODATA table now hangs off
two measured numbers: the fine-structure constant $\alpha$ and the Rydberg constant $R_\infty$. We parse NIST's 2018 and
2022 listings ({n22} and {n18} constants, BLAKE3-pinned), rebuild {nchk} electromagnetic constants from $\alpha$ and
$R_\infty$ (plus $a_e$ and $m_e/m_p$ where they enter), and measure by which power of $\alpha$ each one moved between the two adjustments. Every rebuilt value agrees
with the printed one to $|z|\le{maxz:.2}$; every one of the {nsh} pure-power constants moved by exactly its integer exponent
within the printing-rounding bound (the Thomson cross section by 6, the Hartree energy by 0). The electron anomaly moved by
${an:.3}\pm{ab:.3}$ powers of $\alpha$: a fixed QED theory (${af:.4}$) is excluded by the printed digits, while the
tenth-order coefficient update the paper reports (${au:.4}$) fits. After the CODATA cutoff, the muon $g-2$ tension
CODATA quotes ($4.2\sigma$) has gone (${mt:.1}\sigma$).
\end{{abstract}}
",
        n22 = t22.constants.len(), n18 = t18.constants.len(), nchk = checks.len(), maxz = max_z, nsh = shifts.len(),
        an = anomaly.n_measured, ab = anomaly.bound, af = anomaly.n_fixed_theory, au = anomaly.n_with_a10_update,
        mt = em::muon_g2::wp25_tension());

    let _ = write!(t, r"\section{{Two measured numbers hold up the electromagnetic sky}}
Before 2019, $\mu_0$ was fixed by definition and $\alpha$ was measured. The revised SI swapped the roles: $e$ and $h$ are
fixed, so the electrical standards are exact numbers and the vacuum constants are measured through
\[
\mu_0=\frac{{2h}}{{e^2c}}\,\alpha,\qquad \varepsilon_0=\frac{{e^2}}{{2hc}}\,\frac1\alpha,\qquad
Z_0=\frac{{2h}}{{e^2}}\,\alpha = 2\alpha\,R_{{\rm K}} .
\]
The last identity is worth reading slowly: the impedance of empty space is $2\alpha$ von~Klitzing constants. From the
2022 table, $Z_0/R_{{\rm K}} = {zrk:.12}$ and $2\alpha={twoa:.12}$. Measuring the vacuum's impedance against a
quantum-Hall resistor \emph{{is}} a measurement of $\alpha$. The exact quanta, recomputed from $e$ and $h$, agree with NIST's
truncated prints (Table~\ref{{tab:exact}}).

\begin{{table}}[H]\centering\small
\begin{{tabular}}{{lll}}\toprule
constant & formula & value (exact)\\\midrule
",
        zrk = z0_over_rk, twoa = 2.0 / a22.value);
    let formulas = [r"$K_{\rm J}=2e/h$", r"$R_{\rm K}=h/e^2$", r"$G_0=2e^2/h$", r"$\Phi_0=h/2e$", r"$\hbar=h/2\pi$"];
    for (x, f) in exact.iter().zip(formulas) {
        let _ = writeln!(t, r"{} & {} & ${}$ {}\\", tex_text(x.nist_name), f, tex_sci(x.computed, 12), tex_unit(&t22.must(x.nist_name).unit));
    }
    let _ = write!(t, r"\bottomrule\end{{tabular}}
\caption{{The electrical quanta of the revised SI. Coulomb's constant follows as $k=1/(4\pi\varepsilon_0)={k}$\,N\,m$^2$\,C$^{{-2}}$ and is now measured too.}}\label{{tab:exact}}
\end{{table}}
",
        k = tex_sci(coulomb_k, 9));

    let _ = write!(t, r"\section{{Where the 2022 \texorpdfstring{{$\alpha$}}{{alpha}} comes from}}
$\alpha^{{-1}}$ moved from ${a18:.9}({s18:.0})$ to ${a22:.9}({s22:.0})$, a shift of ${ash:.2}$ times its 2018 uncertainty.
The paper attributes the change primarily to the new LKB rubidium atom-recoil experiment (Morel et al.\ 2020), which
uncovered systematic effects in the earlier LKB result. Three determinations dominate (Table~\ref{{tab:alpha}}).
\begin{{table}}[H]\centering\small
\begin{{tabular}}{{lp{{5.3cm}}rr}}\toprule
label & method & $\alpha^{{-1}}$ & residual (raw)\\\midrule
",
        a18 = a18.value, s18 = a18.sigma * 1e9, a22 = a22.value, s22 = a22.sigma * 1e9, ash = alpha_shift_sigma);
    for (x, r) in inp.iter().zip(&raw.normalized_residuals) {
        let _ = writeln!(t, r"{} & {} & ${:.9}({:.0})$ & ${:+.2}$\\", x.label, tex_text(x.method), x.alpha_inv, x.sigma * 1e9, r);
    }
    let _ = write!(t, r"\midrule
\multicolumn{{2}}{{l}}{{weighted mean, stated uncertainties}} & ${rm:.9}({rs:.0})$ & $\chi^2={rc:.1}$, $\nu=2$\\
\multicolumn{{2}}{{l}}{{weighted mean, uncertainties $\times2.5$ (CODATA)}} & ${em:.9}({es:.0})$ & $\chi^2={ec:.2}$\\
\bottomrule\end{{tabular}}
\caption{{The three key inputs to $\alpha$ (CODATA 2022, Sec.\ XV). Pairwise: Rb vs Cs ${p10:.1}\sigma$, $g-2$ vs Cs ${p20:.1}\sigma$, Rb vs $g-2$ ${p12:.1}\sigma$.}}\label{{tab:alpha}}
\end{{table}}
The inputs do not agree with each other: $\chi^2={rc:.1}$ for two degrees of freedom has probability ${pr}$, and the scatter
is ${br:.1}$ times larger than the stated uncertainties (the Birge ratio). CODATA multiplied the uncertainties of the
$\alpha$ data by an expansion factor of 2.5, which brings $\chi^2$ to ${ec:.2}$ ($p={pe:.2}$) and reproduces the
published value ${em:.9}({es:.0})$ exactly. So the 2022 $\alpha$ is a negotiated compromise between a caesium
measurement that sits ${cs:.1}$ of its own uncertainties below the mean and two others. Without the expansion the
mean would carry a 2.5 times smaller uncertainty (${rs:.1}\times10^{{-9}}$ in $\alpha^{{-1}}$); the disagreement cost
exactly that factor, which is why the 2022 uncertainty ($1.6\times10^{{-10}}$) is no better than 2018's ($1.5\times10^{{-10}}$).

",
        rm = raw.mean, rs = raw.sigma * 1e9, rc = raw.chi2, em = expanded.mean, es = expanded.sigma * 1e9, ec = expanded.chi2,
        p10 = pair(1, 0), p20 = pair(2, 0), p12 = pair(1, 2), pr = tex_sci(em::chi2_p_dof2(raw.chi2), 1),
        br = birge_needed, pe = em::chi2_p_dof2(expanded.chi2), cs = raw.normalized_residuals[0].abs());

    let _ = write!(t, r"\section{{The fingerprint: every constant moved by its power of \texorpdfstring{{$\alpha$}}{{alpha}}}}
The paper lists how 15 constants depend on $\alpha$. If that is right, each constant's relative change between 2018 and
2022 must equal its exponent $n$ times $\Delta\ln\alpha={dla}$, after dividing out the tiny changes of $R_\infty$ and
$m_e/m_p$. We measure $n$ from the two printed tables. The tolerance is not a fudge factor: it is the worst case that half
a unit in the last printed digit of every number used can produce.
\begin{{table}}[H]\centering\footnotesize
\begin{{tabular}}{{lcrrr}}\toprule
constant & law & $n$ theory & $n$ measured & moved ($\sigma_{{2018}}$)\\\midrule
",
        dla = tex_sci(dla, 4));
    for (s, law) in shifts.iter().zip(em::LAWS.iter().filter(|l| l.powers.is_some())) {
        let (_, k, j) = law.powers.unwrap();
        let law_tex = format!("${}$", law_factors(s.n_theory, k, j));
        let _ = writeln!(t, r"${}$ ({}) & {} & ${}$ & ${:+.4}\pm{:.4}$ & ${:+.1}$\\", tex_symbol(s.symbol), s.nist_name, law_tex, s.n_theory, s.n_measured, s.bound, s.shift_in_old_sigma);
    }
    let _ = write!(t, r"\bottomrule\end{{tabular}}
\caption{{2018$\to$2022 moves in powers of $\alpha$. All twelve land on their integer within the rounding bound.}}
\end{{table}}
The pattern is the point. The Thomson cross section $\sigma_e\propto\alpha^6$ moved six times as far as $\alpha$; the
classical electron radius three times; the electron mass $m_e\propto\alpha^{{-2}}$ moved twice as far the other way; and
the Hartree energy $E_{{\rm h}}=2hcR_\infty$, which does not contain $\alpha$ at all, did not move. The main cause of the
$\alpha$ shift was one experiment on rubidium atoms in Paris; through $\alpha$ it moved fifteen rows of the world's table
of constants, each by a predictable amount.

\section{{\texorpdfstring{{$\mu_0$}}{{mu0}} is no longer \texorpdfstring{{$4\pi\times10^{{-7}}$}}{{4 pi 1e-7}}}}
Because $\mu_0\propto\alpha$, its distance from the old defined value moves with every adjustment:
\begin{{table}}[H]\centering\small
\begin{{tabular}}{{lrrr}}\toprule
adjustment & paper (Sec.\ XV) & recomputed here & in $\sigma$\\\midrule
2017 (special) & $+20(23)\times10^{{-11}}$ & --- & ---\\
2018 & $+55(15)\times10^{{-11}}$ & ${d18:+.1}({s18:.0})\times10^{{-11}}$ & ${z18:+.1}$\\
2022 & $-13(16)\times10^{{-11}}$ & ${d22:+.1}({s22:.0})\times10^{{-11}}$ & ${z22:+.1}$\\
\bottomrule\end{{tabular}}
\end{{table}}
In 2018 the measured $\mu_0$ sat ${z18:.1}\sigma$ above $4\pi\times10^{{-7}}$; after the 2022 shift in $\alpha$ it sits
${z22a:.1}\sigma$ below it. The difference between the two is exactly the $\alpha$ story of Section~2; nothing about magnets changed.
(Our 2018 figure differs from the paper's 55 in the last digit because we start from the rounded printed $\mu_0$.)

",
        d18 = d18 * 1e11, s18 = s18 * 1e11, z18 = d18 / s18, d22 = d22 * 1e11, s22 = s22 * 1e11, z22 = d22 / s22, z22a = (d22 / s22).abs());

    let _ = write!(t, r"\section{{The 2022 table is internally consistent}}
Rebuilding the electromagnetic block from $\alpha^{{-1}}$, $R_\infty$, $m_e/m_p$, $a_e$ and the exact $e,h,c$ reproduces every
printed value (Table~\ref{{tab:cons}}). The relative uncertainties also scale as $|n|$ times that of $\alpha$
($1.558\times10^{{-10}}$ before rounding, Sec.\ XV), which is why $\sigma_e$ is six times less certain than $\mu_0$.
\begin{{table}}[H]\centering\small
\begin{{tabular}}{{lllrr}}\toprule
 & NIST 2022 (printed) & rebuilt & $z$ & $u_r/u_r(\alpha)$\\\midrule
");
    for c in &checks {
        let _ = writeln!(t, r"${}$ & ${}$ & ${}$ & ${:+.3}$ & ${:.2}$\\", tex_symbol(c.symbol), tex_sci(c.table, 11), tex_sci(c.derived, 11), c.z, c.rel_sigma / em::ALPHA_REL_SIGMA_2022_UNROUNDED);
    }
    let _ = write!(t, r"\bottomrule\end{{tabular}}
\caption{{$z=(\text{{rebuilt}}-\text{{printed}})/\sigma$. Largest $|z|={maxz:.3}$: only printing rounding is left.}}\label{{tab:cons}}
\end{{table}}

\section{{The electron anomaly shows a theory update}}
$a_e$ is how the best $\alpha$ is extracted ($a_e\approx\alpha/2\pi$ plus higher QED orders), so it moves with $\alpha$.
With fixed theory, $d\ln a_e/d\ln\alpha={af:.4}$. The tables say $a_e$ moved by ${an:.4}\pm{ab:.4}$ powers of $\alpha$:
fixed theory is ${off:.1}$ rounding-bounds away, i.e.\ excluded by the printed digits. The paper reports that the
tenth-order QED coefficient $A_1^{{(10)}}$ changed from 6.675(192) to 6.08(16); adding that change predicts ${au:.4}$,
inside the bound. We do not separate the smaller updates of the hadronic, weak and mass-dependent terms, so this shows
that the shift is consistent with the $A_1^{{(10)}}$ update, not that the update is its only cause.

\section{{Two stories that are not about \texorpdfstring{{$\alpha$}}{{alpha}}}}
\textbf{{Proton radius.}} $r_p$ went from ${rp18:.4}({rps18:.0})$\,fm to ${rp22:.5}({rps22:.0})$\,fm, three times more precise;
the combined value tracks the one from muonic atoms alone (Table XVI). But the puzzle is not closed: from electronic hydrogen and deuterium alone the
2022 adjustment gets $0.8529(43)$\,fm, against $0.84060(66)$\,fm from muonic atoms alone, a ${rpt:.1}\sigma$ gap (Table XVI;
the paper: ``The proton radius `puzzle' is not yet over'').

\textbf{{Muon $g-2$, after the cutoff.}} CODATA 2022 (data to 31 December 2022) left $a_\mu$ out of the adjustment and quoted a
$4.2\sigma$ discrepancy between experiment and theory. Since then, Fermilab's final result
($a_\mu=0.001\,165\,920\,705(114)_{{\rm stat}}(91)_{{\rm syst}}$, June 2025, 127 ppb) lies within ${fc:.1}\sigma$ of the CODATA row,
and the Theory Initiative's 2025 White Paper, now using lattice-QCD hadronic vacuum polarization, gives
$a_\mu^{{\rm SM}}=116\,592\,033(62)\times10^{{-11}}$ against the experimental average $116\,592\,071.5(14.5)\times10^{{-11}}$:
a difference of ${md:.1}\times10^{{-11}}$, ${mt:.1}\sigma$. The famous anomaly has, for now, dissolved.

\section{{What is measured here and what is not}}
\begin{{itemize}}
\item \textbf{{Measured (by this code, from NIST's bytes):}} the consistency table, the twelve exponents and their bounds,
the $\mu_0$ drift, the weighted means and $\chi^2$, the $a_e$ exponent.
\item \textbf{{Taken from the papers, not re-derived:}} the three $\alpha^{{-1}}$ inputs, the expansion factor 2.5, the
$A_1^{{(10)}}$ values, the $r_p$ split by data type, the $4.2\sigma$ figure, the Fermilab and White Paper 2025 numbers.
\item \textbf{{Not claimed:}} anything about which experiment is right. The Cs--Rb disagreement is a measured fact; its cause is open.
\item \textbf{{Next CODATA:}} the 2022 set is still the latest one NIST publishes (checked 13 November 2026).
\end{{itemize}}

\section*{{Sources}}\small
NIST, Fundamental Physical Constants, complete listings 2022 and 2018:
\url{{https://physics.nist.gov/cuu/Constants/Table/allascii.txt}} (BLAKE3 \texttt{{{b22}}}) and
\url{{https://physics.nist.gov/cuu/Constants/ArchiveASCII/allascii_2018.txt}} (\texttt{{{b18}}}).\\
P.~J.~Mohr, D.~B.~Newell, B.~N.~Taylor, E.~Tiesinga, \emph{{CODATA recommended values of the fundamental physical constants: 2022}},
Rev.\ Mod.\ Phys.\ \textbf{{97}}, 025002 (2025), \href{{https://arxiv.org/abs/2409.03787}}{{arXiv:2409.03787}}.\\
Muon $g-2$ Theory Initiative, \emph{{White Paper 2025}}, \href{{https://arxiv.org/abs/2505.21476}}{{arXiv:2505.21476}};
Fermilab Muon $g-2$ final result, 3 June 2025 (\url{{https://physics.illinois.edu/news/muon_g-2_final-result}}).\\
Code: \texttt{{flux/crates/flux-science/src/electromagnetism.rs}} and \texttt{{src/bin/codata\_em.rs}}; 12 tests pin every claim above.
\end{{document}}
",
        maxz = max_z, af = anomaly.n_fixed_theory, an = anomaly.n_measured, ab = anomaly.bound,
        off = (anomaly.n_measured - anomaly.n_fixed_theory).abs() / anomaly.bound, au = anomaly.n_with_a10_update,
        rp18 = rp18.value * 1e15, rps18 = rp18.sigma * 1e19, rp22 = rp22.value * 1e15, rps22 = rp22.sigma * 1e20,
        rpt = em::proton_radius_tension(), fc = fnal_vs_codata.abs(),
        md = (em::muon_g2::EXP_AVERAGE_2025.0 - em::muon_g2::SM_WP25.0) * 1e11, mt = em::muon_g2::wp25_tension(),
        b22 = &em::NIST_2022_BLAKE3[..16], b18 = &em::NIST_2018_BLAKE3[..16]);

    let path = format!("{out_dir}/codata-em-2022.tex");
    std::fs::write(&path, t).expect("write tex");
    println!("\n  wrote {path}");
}
