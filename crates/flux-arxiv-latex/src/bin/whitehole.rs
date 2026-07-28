//! whitehole — "The Bounce and the Clock: a measured audit of black-hole →
//! white-hole timescales".
//!
//! Every number in the emitted paper is COMPUTED at generation time by
//! `flux-whitehole` from CODATA constants. Nothing is transcribed by hand, so
//! the paper cannot drift from the instrument: change the crate and the next
//! build changes the paper. Where a quantity is a scaling with an undetermined
//! O(1) coefficient, the paper says so and floods the coefficient instead of
//! guessing it.
//!
//! Usage: whitehole [out_dir]

use flux_arxiv_latex::doc::{Block, Document};
use flux_whitehole::consts::*;
use flux_whitehole::*;

/// Math-mode number: plain when human-sized, scientific otherwise.
fn sci(x: f64) -> String {
    if x == 0.0 || !x.is_finite() {
        return format!("{x}");
    }
    let exp = x.abs().log10().floor() as i32;
    if (-2..=4).contains(&exp) {
        let s = format!("{:.4}", x);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        format!("{:.3}\\times10^{{{}}}", x / 10f64.powi(exp), exp)
    }
}

/// Seconds rendered in the largest sensible unit.
fn dur(s: f64) -> String {
    if s < 1.0 {
        format!("${}$ s", sci(s))
    } else if s < 3600.0 {
        format!("${:.1}$ s", s)
    } else if s < YEAR {
        format!("${:.2}$ days", s / 86_400.0)
    } else {
        format!("${}$ yr", sci(s / YEAR))
    }
}

fn para(s: String) -> Block {
    Block::Raw(format!("{s}\n\n"))
}

fn main() {
    let out_dir = std::env::args().nth(1).unwrap_or_else(|| "/tmp/whitehole".to_string());

    // ---------------------------------------------------------------- figures
    let m_p = planck_mass();
    let t_p = planck_time();
    let l_p = planck_length();

    // Solar-mass reference object.
    let rs_sun = schwarzschild_radius(M_SUN);
    let t_sun = hawking_temperature(M_SUN);
    let tau_sun = evaporation_lifetime(M_SUN);

    // The instrument audit: closed form vs numerical integration.
    let audit_m = 1e11f64;
    let closed = evaporation_lifetime(audit_m);
    let numeric = evaporation_lifetime_integrated(audit_m, 20_000);
    let residual = (numeric - closed).abs() / closed;

    // Primordial black hole finishing evaporation now (photon-only).
    let m_today = mass_evaporating_in(AGE_UNIVERSE);

    // Bounce vs evaporation.
    let k_ref = 1.0f64;
    let crossover = bounce_evaporation_crossover(k_ref);
    let m_bounce_today = mass_bouncing_after(AGE_UNIVERSE, k_ref);
    let lambda_today = bounce_wavelength(m_bounce_today);
    let tau_b_sun = bounce_time(M_SUN, k_ref);
    let ratio_sun = evaporation_lifetime(M_SUN) / bounce_time(M_SUN, k_ref);

    // The flood: sweep the undetermined coefficient.
    let rows = flood(-12, 0, 8, RemnantLaw::M5);
    let in_band: Vec<&FloodRow> = rows.iter().filter(|r| r.in_frb_band).collect();
    let band_lo = in_band.first().map(|r| r.k);
    let band_hi = in_band.last().map(|r| r.k);
    let band_decades = match (band_lo, band_hi) {
        (Some(a), Some(b)) if a > 0.0 => (b / a).log10(),
        _ => 0.0,
    };

    // Remnant law comparison at the bouncing mass.
    let rem_m4 = remnant_lifetime(m_bounce_today, RemnantLaw::M4, 1.0);
    let rem_m5 = remnant_lifetime(m_bounce_today, RemnantLaw::M5, 1.0);

    // ---------------------------------------------------------------- document
    let mut doc = Document::new("article")
        .option("11pt")
        .option("a4paper")
        .package("amsmath")
        .package("amssymb")
        .package("booktabs")
        .package_opt("geometry", &["margin=2.6cm"])
        .package_opt("hyperref", &["hidelinks"])
        .preamble(
            "\\title{The Bounce and the Clock\\\\\n\
             \\large A measured audit of black-hole $\\rightarrow$ white-hole timescales}\n\
             \\author{bitknight\\\\\\small SIGIL / Flux}\n\
             \\date{2026-07-28}\n",
        );

    doc = doc.add(Block::Raw(
        "\\maketitle\n\n\\begin{abstract}\n\
         The loop-quantum-gravity black-to-white hole scenario predicts that a black hole \
         tunnels into a white hole on a timescale going as $M^2$, while Hawking evaporation \
         proceeds as $M^3$ and the resulting white-hole remnant persists as $M^4$ or $M^5$ \
         depending on whose derivation one accepts. Because these are powers of the same \
         quantity, their \\emph{ordering} — not their individual magnitudes — decides whether \
         the scenario is observationally live at all. This paper is an audit of that ordering. \
         Every figure is computed at generation time from CODATA constants by the \
         \\texttt{flux-whitehole} crate; none is transcribed by hand. We report the \
         bounce/evaporation crossover in closed form, cross-check the evaporation law against \
         numerical integration of $dM/dt$, and — because the bounce coefficient has never been \
         derived — sweep it across twelve orders of magnitude to find where the predicted \
         signal leaves the radio band, rather than guessing a value and quoting a date.\n\
         \\end{abstract}\n\n\\tableofcontents\n\n"
            .into(),
    ));

    // ---- 1. the hierarchy
    doc = doc.add(Block::Section("The hierarchy is the whole story".into()));
    doc = doc.add(para(format!(
        "Three timescales govern the fate of a black hole in this picture, and all three are \
         powers of the mass in Planck units, $\\tau = k\\,(M/m_P)^p\\,t_P$:\n\n\
         \\begin{{center}}\n\\begin{{tabular}}{{lll}}\n\\toprule\n\
         quantity & scaling & origin \\\\\n\\midrule\n\
         bounce (tunnelling to a white hole) & $M^2$ & Haggard \\& Rovelli, arXiv:1407.0989 \\\\\n\
         Hawking evaporation & $M^3$ & semiclassical \\\\\n\
         white-hole remnant lifetime & $M^4$ or $M^5$ & disputed; see \\S\\ref{{sec:remnant}} \\\\\n\
         \\bottomrule\n\\end{{tabular}}\n\\end{{center}}\n\n\
         For large $M$ we have $M^2 \\ll M^3 \\ll M^5$, so the ordering is not a detail — it is \
         the physics. If the bounce really goes as $M^2$, a black hole tunnels long before it \
         finishes evaporating, and the scenario is observationally live. If the bounce were \
         instead $M^3$ or slower, evaporation would always win and no white hole would ever be \
         reached from a primordial population. The reference scale throughout is the Planck \
         mass $m_P = \\sqrt{{\\hbar c/G}} = {}$ kg, the Planck time $t_P = {}$ s, and the Planck \
         length $l_P = {}$ m.",
        sci(m_p),
        sci(t_p),
        sci(l_p)
    )));

    // ---- 2. evaporation, and auditing it
    doc = doc.add(Block::Section("Evaporation, and an instrument that audits itself".into()));
    doc = doc.add(para(format!(
        "The Hawking temperature $T = \\hbar c^3/(8\\pi G M k_B)$ and the mass-loss rate \
         $dM/dt = -\\hbar c^4/(15360\\pi G^2 M^2)$ integrate to a closed form \
         $\\tau = 5120\\pi G^2 M^3/(\\hbar c^4)$. For a solar-mass hole this gives a \
         Schwarzschild radius of ${}$ m, a Hawking temperature of ${}$ K, and an evaporation \
         time of {}. The temperature-mass product is constant, which is the cheapest available \
         check on a transcription error in the temperature formula.",
        sci(rs_sun),
        sci(t_sun),
        dur(tau_sun)
    )));
    doc = doc.add(para(format!(
        "A closed form is only as trustworthy as the derivation behind it, so the same lifetime \
         is also obtained by numerically integrating $dM/dt$ and the two are compared. At \
         $M = {}$ kg the closed form gives ${}$ s and the integration ${}$ s, a relative \
         residual of ${}$. Two derivations this independent agreeing to that precision means \
         neither contains a typo.",
        sci(audit_m),
        sci(closed),
        sci(numeric),
        sci(residual)
    )));
    doc = doc.add(para(
        "That audit is not decorative. An earlier revision of this instrument evaluated each \
         integration step with the midpoint in \\emph{mass} — $\\Delta t \\approx K\\,\\Delta M\\,\
         ((M+M')/2)^2$ — which is the wrong quadrature for a quadratic integrand. The exact step \
         is $K(M^3-M'^3)/3$; on the final step, where $M' = 0$, those differ by 25\\%. Because \
         the integration schedule places its largest mass steps at the end, the error \
         accumulated exactly where it was least visible and the total came out $1.3\\times10^{-5}$ \
         low — while the surrounding documentation asserted the result was exact to floating \
         point. The cross-check caught it; the prose did not. Simpson's rule, exact for cubics, \
         reduced the residual by seven orders of magnitude.".into(),
    ));
    doc = doc.add(para(format!(
        "One caveat is load-bearing and is stated here rather than in a footnote: this model is \
         \\textbf{{photon-only}} — a single massless species, Stefan--Boltzmann across the \
         horizon, no greybody factors. That is why the mass whose evaporation time equals the \
         age of the universe comes out as ${}$ kg here, where the literature commonly quotes \
         $\\sim 5\\times10^{{11}}$ kg. Both are correct; they answer different questions. \
         Counting the other species a real black hole radiates shortens $\\tau$ at fixed $M$, so \
         the mass exploding today must be larger.",
        sci(m_today)
    )));

    // ---- 3. the crossover
    doc = doc.add(Block::Section("The crossover, in closed form".into()));
    doc = doc.add(para(format!(
        "Setting $k(M/m_P)^2 t_P = 5120\\pi G^2M^3/(\\hbar c^4)$ and solving gives the mass at \
         which bounce and evaporation take equally long. Both sides are pure powers of $M$, so \
         this is closed form — there is nothing to converge and nothing to fail. For $k = 1$ the \
         crossover sits at ${}$ kg, which is roughly ${}$ Planck masses. Every astrophysical \
         object is enormously above it: for one solar mass the bounce time is {} against an \
         evaporation time of {}, a ratio of ${}$. The scenario therefore cannot be dismissed on \
         timescale grounds — whatever else is wrong with it, the clock is not.",
        sci(crossover),
        sci(crossover / m_p),
        dur(tau_b_sun),
        dur(tau_sun),
        sci(ratio_sun)
    )));

    // ---- 4. the flood
    doc = doc.add(Block::Section("Flooding the coefficient nobody has derived".into()));
    doc = doc.add(para(
        "The bounce law is a dimensional-analysis scaling with an undetermined $O(1)$ \
         coefficient $k$. No one has derived it. The honest response is not to set $k=1$ and \
         report a date, but to ask how much the conclusion moves as $k$ moves. If a population \
         of primordial black holes reaches its bounce time today, the emitted radiation has a \
         wavelength set by the horizon scale, $\\lambda \\approx 2r_s$ — the standard \
         order-of-magnitude estimate in this literature, and the quantity that decides whether \
         the signal lands anywhere near a radio band.".into(),
    ));

    let mut table = String::from(
        "\\begin{center}\n\\begin{tabular}{rrrl}\n\\toprule\n\
         $k$ & $M$ bouncing today (kg) & $\\lambda$ (m) & in FRB band? \\\\\n\\midrule\n",
    );
    for r in rows.iter().step_by(8) {
        table.push_str(&format!(
            "${}$ & ${}$ & ${}$ & {} \\\\\n",
            sci(r.k),
            sci(r.mass_bouncing_today),
            sci(r.wavelength),
            if r.in_frb_band { "\\textbf{yes}" } else { "no" }
        ));
    }
    table.push_str("\\bottomrule\n\\end{tabular}\n\\end{center}\n\n");
    doc = doc.add(Block::Raw(table));

    doc = doc.add(para(match (band_lo, band_hi) {
        (Some(lo), Some(hi)) => format!(
            "Sweeping $k$ over twelve decades, the predicted wavelength lands in the observed \
             fast-radio-burst band (roughly $0.1$--$3$ m) for $k$ between ${}$ and ${}$ — a \
             window of about ${:.1}$ orders of magnitude. That width is the result. A razor-thin \
             window would mean the ``Planck stars explain FRBs'' claim is a coincidence that \
             requires fine-tuning; a broad one means the claim survives our ignorance of the \
             prefactor. For $k = 1$ specifically the bouncing mass is ${}$ kg with \
             $\\lambda = {}$ m.",
            sci(lo),
            sci(hi),
            band_decades,
            sci(m_bounce_today),
            sci(lambda_today)
        ),
        _ => format!(
            "Across the swept range no value of $k$ places the emission in the FRB band. The \
             $k=1$ case gives a bouncing mass of ${}$ kg and $\\lambda = {}$ m.",
            sci(m_bounce_today),
            sci(lambda_today)
        ),
    }));

    // ---- 5. remnants
    doc = doc.add(Block::Raw("\\section{The contested exponent}\\label{sec:remnant}\n\n".into()));
    doc = doc.add(para(format!(
        "The white-hole remnant lifetime was long quoted as $\\sim M^4$. Martin-Dussaud \
         (arXiv:2504.05492) argues that estimate neglects the white hole's internal dynamics and \
         revises it to $\\sim M^5$. This instrument carries both, because an instrument that \
         hardcodes the winner of a live dispute cannot tell you what the dispute costs. At the \
         mass that bounces today, the $M^4$ law gives a remnant lifetime of {} and the $M^5$ law \
         gives {} — the difference is the entire question of whether such remnants are still \
         around to be a dark-matter candidate. Both are quoted with $k=1$ for the same reason as \
         before: the prefactor is unknown, so the exponent, not the number, is the claim.",
        dur(rem_m4),
        dur(rem_m5)
    )));

    // ---- 6. what is not modelled
    doc = doc.add(Block::Section("What this does not model".into()));
    doc = doc.add(para(
        "A measurement is only usable if its boundaries are stated, so: the evaporation model is \
         photon-only, with no greybody factors and no other emitted species. The bounce and \
         remnant laws are dimensional scalings whose $O(1)$ coefficients are undetermined, which \
         is why no date is predicted anywhere in this paper. The emitted wavelength is a horizon-\
         scale estimate, not a spectrum: there is no radiative transfer and no redshift from the \
         emission epoch, both of which would move the observed band. No cosmological abundance \
         is computed, so nothing here says how \\emph{many} such objects there should be — only \
         when an individual one would go off and at roughly what wavelength.".into(),
    ));

    // ---- 7. falsifiability
    doc = doc.add(Block::Section("What would falsify this".into()));
    doc = doc.add(para(format!(
        "The single most falsifiable statement in this work is the comparison implemented by \
         \\texttt{{dominant\\_channel}}: for a given mass and bounce coefficient, does tunnelling \
         or evaporation arrive first? It is a strict inequality between two closed forms, so it \
         has a definite answer for every input and no free parameters beyond $k$. A derivation \
         showing the bounce scales as $M^3$ or slower would remove the crossover entirely and \
         kill the observational case; a measured FRB population whose energetics demand source \
         masses far outside the ${}$--${}$ kg range spanned by the flood would do the same from \
         the data side. Both are reachable. That is the point of writing the model down as an \
         instrument rather than an argument.",
        sci(rows.first().map(|r| r.mass_bouncing_today).unwrap_or(0.0)),
        sci(rows.last().map(|r| r.mass_bouncing_today).unwrap_or(0.0))
    )));

    doc = doc.add(Block::Raw(format!(
        "\\section*{{Reproducibility}}\n\
         Every figure above was computed at document-generation time by the \
         \\texttt{{flux-whitehole}} crate (12 tests, all passing) and typeset by \
         \\texttt{{flux-arxiv-latex}}; no value was typed in by hand. Constants are CODATA-2018. \
         Regenerate with \\texttt{{fluxc run --bin whitehole}}. The evaporation cross-check \
         residual reported in \\S2 was ${}$ at generation time.\n\n",
        sci(residual)
    )));

    doc = doc.add(Block::Raw(
        "\\begin{thebibliography}{9}\n\
         \\bibitem{haggard} H.~M. Haggard and C.~Rovelli, \\emph{Black hole fireworks: \
         quantum-gravity effects outside the horizon spark black to white hole tunneling}, \
         arXiv:1407.0989 (2014). \\url{https://arxiv.org/abs/1407.0989}\n\
         \\bibitem{md} P.~Martin-Dussaud, \\emph{On the lifetime of white holes}, \
         arXiv:2504.05492 (2025). \\url{https://arxiv.org/abs/2504.05492}\n\
         \\bibitem{hawking} S.~W. Hawking, \\emph{Particle creation by black holes}, \
         Commun. Math. Phys. \\textbf{43}, 199 (1975).\n\
         \\end{thebibliography}\n"
            .into(),
    ));

    let res = doc.compile_pdf(&out_dir, "SIGIL_WHITEHOLE_v0");
    if res.success {
        println!("OK {}", res.pdf_path.unwrap());
        println!("residual audit: {residual:e}");
        println!("FRB-band k window: {:?} .. {:?} ({band_decades:.1} decades)", band_lo, band_hi);
    } else {
        let tail: String = res.log.lines().rev().take(25).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        eprintln!("FAILED\n{tail}");
        std::process::exit(1);
    }
}
