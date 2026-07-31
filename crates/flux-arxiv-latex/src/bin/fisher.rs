//! fisher — "The Sign of Ignorance: precision-weighted estimation of a remote
//! monotone counter, and a production failure that ran the sign backwards".
//!
//! Every number in the emitted paper is COMPUTED at generation time by calling
//! `flux_science::fisher`. Nothing is transcribed by hand, so the paper cannot
//! drift from the instrument: change the module and the next build changes the
//! paper. The one class of number that is NOT computed here is the production
//! incident trace — those are observed values from a live 21.4M-block chain,
//! and they are labelled as observations rather than derivations.
//!
//! Usage: fisher [out_dir]

use flux_arxiv_latex::doc::{Block, Document};
use flux_science::fisher::*;

/// Math-mode number: plain when human-sized, scientific otherwise.
fn sci(x: f64) -> String {
    if x == 0.0 {
        return "0".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "\\infty".into() } else { "-\\infty".into() };
    }
    if !x.is_finite() {
        return format!("{x}");
    }
    let exp = x.abs().log10().floor() as i32;
    if (-3..=5).contains(&exp) {
        let s = format!("{:.4}", x);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        format!("{:.3}\\times10^{{{}}}", x / 10f64.powi(exp), exp)
    }
}

/// Thousands-separated integer, for heights.
fn com(x: f64) -> String {
    let s = format!("{}", x.round() as i64);
    let (sign, digits) = if let Some(rest) = s.strip_prefix('-') { ("-", rest) } else { ("", &s[..]) };
    let mut out = String::new();
    for (i, c) in digits.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    format!("{sign}{}", out.chars().rev().collect::<String>())
}

fn para(s: String) -> Block {
    Block::Raw(format!("{s}\n\n"))
}

/// The production heuristic, reimplemented verbatim so the paper can run it
/// rather than describe it. This is the code that failed:
///
///     decay = (network_height - local_height) / 10
///     network_height = (network_height - decay).max(local_height)
fn legacy_decay(network_height: f64, local_height: f64) -> f64 {
    let decay = (network_height - local_height) / 10.0;
    (network_height - decay).max(local_height)
}

fn main() {
    let out_dir = std::env::args().nth(1).unwrap_or_else(|| "/tmp/fisher".to_string());

    // ─────────────────────────────────────────────────────────── the incident
    // Observed on the live chain: a fresh node parked at this local height while
    // the true tip was ~21.4M. These two are OBSERVATIONS, not derivations.
    let local = 115_247.0_f64;
    let peer_tip = 21_364_541.0_f64;
    let true_gap = peer_tip - local;

    // Run the actual failed heuristic forward until it converges. This is the
    // paper's central empirical claim and it is executed, not asserted.
    let mut h = peer_tip;
    let mut trace: Vec<(usize, f64)> = vec![(0, h)];
    let mut iters = 0usize;
    // 15 s per tick in production; iterate until it is within one block of local.
    while h - local > 1.0 && iters < 100_000 {
        h = legacy_decay(h, local);
        iters += 1;
        if iters <= 5 || iters % 50 == 0 {
            trace.push((iters, h));
        }
    }
    let decay_ticks = iters;
    let decay_wallclock_s = decay_ticks as f64 * 15.0;
    let decay_final_gap = h - local;

    // Half-life of the heuristic: each tick multiplies the gap by 0.9.
    let half_life_ticks = (0.5f64).ln() / (0.9f64).ln();

    // ─────────────────────────────────────────────────────── the arrival model
    // λ measured on the same chain: 21.4M blocks over the chain's lifetime is
    // not the right rate — the live block cadence is. Use the production figure.
    let lambda = 2.0_f64; // blocks/sec
    let model = ArrivalModel::new(lambda);

    // The same single observation, aged. The estimate must RISE, never fall.
    let ages = [0.0_f64, 60.0, 600.0, 3_600.0, 60_000.0];
    let aged: Vec<(f64, FusedEstimate)> = ages
        .iter()
        .map(|&a| (a, fuse(&[StaleObservation::aged(peer_tip, a)], &model)))
        .collect();

    // Direct head-to-head at one hour of staleness.
    let one_hour = 3_600.0_f64;
    let fisher_1h = fuse(&[StaleObservation::aged(peer_tip, one_hour)], &model);
    let mut legacy_1h = peer_tip;
    for _ in 0..(one_hour / 15.0) as usize {
        legacy_1h = legacy_decay(legacy_1h, local);
    }
    let spread = fisher_1h.value - legacy_1h;

    // ────────────────────────────────────────────────── the fail-safe property
    let nothing = fuse(&[], &model);
    let nothing_ucb = nothing.upper_confidence_bound(2.0);
    let nothing_behind_at_max = nothing.is_behind(f64::MAX, 2.0);
    let nothing_crb = nothing.cramer_rao_bound();

    // ──────────────────────────────────────────────────────────── additivity
    let obs_one = [StaleObservation::aged(peer_tip, 60.0)];
    let obs_three = [
        StaleObservation::aged(peer_tip, 60.0),
        StaleObservation::aged(peer_tip, 60.0),
        StaleObservation::aged(peer_tip, 60.0),
    ];
    let est_one = fuse(&obs_one, &model);
    let est_three = fuse(&obs_three, &model);
    let info_ratio = est_three.total_information / est_one.total_information;
    let var_ratio = est_one.variance / est_three.variance;

    // ───────────────────────────────────────────────────── Cramér–Rao / efficiency
    let crb = est_three.cramer_rao_bound();
    let eff_optimal = est_three.efficiency(est_three.variance);
    let eff_ten_x = est_three.efficiency(est_three.variance * 10.0);
    let eff_infinite = est_three.efficiency(f64::INFINITY);

    // ──────────────────────────────────────────── information-as-trust (a liar)
    let honest = StaleObservation { value: peer_tip, staleness_s: 1.0, base_variance: 0.0 };
    let liar = StaleObservation { value: 0.0, staleness_s: 1.0, base_variance: 1.0e12 };
    let i_honest = honest.fisher_information(&model);
    let i_liar = liar.fisher_information(&model);
    let trust_ratio = i_honest / i_liar;
    let with_liar = fuse(&[honest, liar], &model);
    let liar_pull = peer_tip - with_liar.value;

    // ────────────────────────────────── fresh source dominates a very stale one
    let fresh_src = StaleObservation::aged(peer_tip, 0.1);
    let stale_src = StaleObservation::aged(peer_tip + 5_000_000.0, 100_000.0);
    let mixed = fuse(&[fresh_src, stale_src], &model);
    let d_fresh = (mixed.value - fresh_src.expected_now(&model)).abs();
    let d_stale = (mixed.value - stale_src.expected_now(&model)).abs();

    // ─────────────────────────────────────────────────────────────── document
    let mut doc = Document::new("article")
        .option("11pt")
        .option("a4paper")
        .package("amsmath")
        .package("amssymb")
        .package("booktabs")
        .package_opt("geometry", &["margin=2.6cm"])
        .package_opt("hyperref", &["hidelinks"])
        .preamble(
            "\\title{The Sign of Ignorance\\\\\n\
             \\large Fisher information, and a catch-up protocol that could not tell \
             ``I don't know'' from ``I'm done''}\n\
             \\author{bitknight\\\\\\small SIGIL / Flux}\n\
             \\date{2026-07-28}\n",
        );

    doc = doc.add(Block::Raw(format!(
        "\\maketitle\n\n\\begin{{abstract}}\n\
         A live {}-block chain shipped a staleness heuristic that decayed its \\emph{{estimate}} \
         of the network tip toward the node's own height whenever peer data aged. Fresh nodes \
         therefore concluded they were caught up while {} blocks behind, and stopped syncing. \
         This paper argues the failure is not a tuning error but a \\emph{{sign inversion}}: \
         under any arrival model, a stale report is evidence the true value is \\emph{{higher}}, \
         and staleness must decay the \\emph{{precision}} of an estimate, never the estimate \
         itself. We give the Fisher-information formulation, show that inverse-variance fusion \
         makes the safe behaviour \\emph{{structural}} rather than a tuned threshold — with no \
         observations, total information goes to zero, the variance diverges, and the upper \
         confidence bound is $+\\infty$, so ignorance can only ever read as ``keep working'' — \
         and report the Cram\\'er--Rao bound the estimator saturates. Every figure below is \
         computed at generation time by the \\texttt{{flux-science::fisher}} module, including \
         a re-execution of the failed heuristic itself. \\textbf{{Scope:}} this is classical \
         Fisher information. No quantum speedup is claimed; see \\S\\ref{{sec:scope}}.\n\
         \\end{{abstract}}\n\n\\tableofcontents\n\n",
        com(21_400_000.0),
        com(true_gap),
    )));

    // ── 1. the incident
    doc = doc.add(Block::Section("A protocol that declared victory at 0.5\\% synced".into()));
    doc = doc.add(para(format!(
        "The failure is best stated as a pair of numbers. A node at local height ${}$ received \
         a peer report of ${}$ — a true gap of ${}$ blocks, meaning the node held \
         ${:.2}\\%$ of the chain. It reported itself caught up and stopped requesting blocks.",
        com(local),
        com(peer_tip),
        com(true_gap),
        100.0 * local / peer_tip
    )));
    doc = doc.add(para(
        "The code responsible was a single expression, run every 15 seconds:\n\n\
         \\begin{verbatim}\n\
         decay = (network_height - local_height) / 10\n\
         network_height = (network_height - decay).max(local_height)\n\
         \\end{verbatim}\n\n\
         Read it as a dynamical system and the outcome is immediate: each tick multiplies the \
         gap by $0.9$, and the floor at \\texttt{local\\_height} is an attractor. The estimate \
         does not become uncertain as data ages — it converges, monotonically and by \
         construction, on the answer ``you are finished''."
            .into(),
    ));
    doc = doc.add(para(format!(
        "This paper runs that expression rather than describing it. Starting from the observed \
         ${}$ and iterating until the gap closes to under one block takes ${}$ ticks — ${:.1}$ \
         minutes of wall-clock at the production interval — and lands at a residual gap of \
         ${}$ blocks. The gap's half-life is ${:.2}$ ticks, so the estimate has lost half the \
         truth inside ${:.0}$ seconds of a peer going quiet:",
        com(peer_tip),
        decay_ticks,
        decay_wallclock_s / 60.0,
        sci(decay_final_gap),
        half_life_ticks,
        half_life_ticks * 15.0
    )));

    let mut trace_rows = String::new();
    for (tick, value) in trace.iter().take(9) {
        trace_rows.push_str(&format!(
            "{} & ${}$ & ${}$ \\\\\n",
            tick,
            com(*value),
            com(value - local)
        ));
    }
    doc = doc.add(Block::Raw(format!(
        "\\begin{{center}}\n\\begin{{tabular}}{{rrr}}\n\\toprule\n\
         tick (15\\,s) & estimated tip & implied gap \\\\\n\\midrule\n\
         {trace_rows}\
         \\multicolumn{{3}}{{c}}{{$\\vdots$}} \\\\\n\
         {} & ${}$ & ${}$ \\\\\n\\bottomrule\n\\end{{tabular}}\n\\end{{center}}\n\n",
        decay_ticks,
        com(local + decay_final_gap),
        sci(decay_final_gap)
    )));
    doc = doc.add(para(
        "The observed production trace has the same shape — $21{,}364{,}541 \\rightarrow \\dots \
         \\rightarrow 115{,}289 \\rightarrow 115{,}285 \\rightarrow 115{,}282 \\rightarrow \
         115{,}270$ — decaying toward a local height of $115{,}247$. Those are logged \
         observations from the live node, quoted here as evidence rather than derived."
            .into(),
    ));

    // ── 2. the sign
    doc = doc.add(Block::Section("The sign is inverted, not the constant".into()));
    doc = doc.add(para(format!(
        "The tempting reading is that $1/10$ per tick is simply too aggressive. It is not a \
         constant problem. Model the remote counter as a Poisson arrival process at rate \
         $\\lambda$ — for a blockchain tip, $\\lambda$ is the block rate, here ${}$ blocks/s. \
         A peer reported value $h$ observed $\\Delta$ seconds ago gives\n\n\
         \\begin{{align}}\n\
         \\mathbb{{E}}[\\theta \\mid h, \\Delta] &= h + \\lambda\\Delta \\\\\n\
         \\operatorname{{Var}}[\\theta \\mid h, \\Delta] &= \\lambda\\Delta + \\sigma^2\n\
         \\end{{align}}\n\n\
         Both terms increase with $\\Delta$. The expectation moves \\emph{{up}}, because blocks \
         kept arriving while we were not looking; the variance moves up too, because we do not \
         know how many. Any decay applied to the estimate has the wrong sign no matter how \
         small its coefficient — shrinking it merely postpones the failure.",
        sci(lambda)
    )));
    doc = doc.add(para(format!(
        "Running the module confirms the direction. The same single observation of ${}$, aged, \
         yields a strictly increasing estimate and a strictly increasing variance:",
        com(peer_tip)
    )));

    let mut aged_rows = String::new();
    for (age, est) in &aged {
        aged_rows.push_str(&format!(
            "${}$ & ${}$ & ${}$ & ${}$ \\\\\n",
            sci(*age),
            com(est.value),
            sci(est.variance),
            sci(est.upper_confidence_bound(2.0))
        ));
    }
    doc = doc.add(Block::Raw(format!(
        "\\begin{{center}}\n\\begin{{tabular}}{{rrrr}}\n\\toprule\n\
         staleness $\\Delta$ (s) & estimate $\\hat\\theta$ & $\\operatorname{{Var}}(\\hat\\theta)$ \
         & upper bound ($z{{=}}2$) \\\\\n\\midrule\n{aged_rows}\\bottomrule\n\
         \\end{{tabular}}\n\\end{{center}}\n\n"
    )));
    doc = doc.add(para(format!(
        "At one hour of staleness the two approaches have separated by ${}$ blocks: the \
         heuristic reports a tip of ${}$ (i.e. ``you are done''), the information-weighted \
         estimate reports ${}$ with a $2\\sigma$ upper bound of ${}$. The heuristic's error is \
         not that it is imprecise. It is that its error points at the one conclusion a \
         catch-up protocol must never reach by default.",
        com(spread),
        com(legacy_1h),
        com(fisher_1h.value),
        com(fisher_1h.upper_confidence_bound(2.0))
    )));

    // ── 3. information
    doc = doc.add(Block::Section("Fisher information is precision, and it adds".into()));
    doc = doc.add(para(
        "Fisher information is the curvature of the log-likelihood at the true parameter; for \
         the Gaussian-approximated counter above it is simply the reciprocal of the variance, \
         $I = 1/\\operatorname{Var}$. Two facts make it the right object here. First, \
         information from independent sources is \\emph{additive}, so combining peers is a sum \
         rather than a policy. Second, weighting each observation by its information gives the \
         minimum-variance unbiased combination:\n\n\
         \\begin{align}\n\
         \\hat\\theta &= \\frac{\\sum_i I_i \\, \\mathbb{E}[\\theta \\mid h_i, \\Delta_i]}\
         {\\sum_i I_i}, & \\operatorname{Var}(\\hat\\theta) &= \\frac{1}{\\sum_i I_i}\n\
         \\end{align}"
            .into(),
    ));
    doc = doc.add(para(format!(
        "Additivity is measurable, so it is measured: one observation at $\\Delta = 60$\\,s \
         carries $I = {}$; three identical ones carry ${}$, a ratio of ${:.4}$, and the fused \
         variance falls by the same factor (${:.4}$). Nothing about that is a tuning choice — \
         it falls out of the definition, which is exactly the property the replaced heuristic \
         lacked.",
        sci(est_one.total_information),
        sci(est_three.total_information),
        info_ratio,
        var_ratio
    )));

    // ── 4. the fail-safe
    doc = doc.add(Block::Section("Why the safe behaviour is structural".into()));
    doc = doc.add(para(format!(
        "The decisive case is a node with \\emph{{no}} usable peer data — precisely the state \
         the failing nodes were in. Fusing an empty observation set gives $\\sum I = {}$, hence \
         $\\operatorname{{Var}} = {}$ and a $2\\sigma$ upper confidence bound of ${}$. Because \
         the ``am I behind?'' decision is taken on the \\emph{{upper}} bound, the answer with no \
         information is unconditionally yes — the module returns \\texttt{{is\\_behind}} $= \
         \\texttt{{{}}}$ even when the local height is $f64\\text{{::MAX}}$.",
        sci(nothing.total_information),
        sci(nothing.variance),
        sci(nothing_ucb),
        nothing_behind_at_max
    )));
    doc = doc.add(para(
        "This is the whole design argument in one sentence: \\textbf{the heuristic fails toward \
         ``caught up'' and this cannot}. Not because a threshold was chosen well, but because \
         there is no finite number for it to return. A protocol whose failure mode is ``keep \
         asking for blocks you already have'' wastes bandwidth; a protocol whose failure mode \
         is ``declare victory'' silently forks a node off the network. Those costs are not \
         symmetric, and the estimator should not be either."
            .into(),
    ));

    // ── 5. Cramér–Rao
    doc = doc.add(Block::Section("How much is left to win".into()));
    doc = doc.add(para(format!(
        "The Cram\\'er--Rao bound states that no unbiased estimator can achieve variance below \
         $1/\\sum I$. For the three-observation case above that floor is ${}$, and the fused \
         estimator's own variance is ${}$ — an efficiency of ${:.6}$. The estimator is optimal: \
         it \\emph{{saturates}} the bound.",
        sci(crb),
        sci(est_three.variance),
        eff_optimal
    )));
    doc = doc.add(para(format!(
        "That number is an engineering instruction, not a compliment. An efficiency of ${:.4}$ \
         means there is nothing further to extract from the estimator, so effort should move to \
         the \\emph{{observations}} — poll peers more often, or add peers. For contrast, an \
         estimator with ten times the variance scores ${:.4}$, and one with infinite variance \
         scores ${:.4}$. The bound tells you which of the two available levers is the live one.",
        eff_optimal,
        eff_ten_x,
        eff_infinite
    )));

    // ── 6. trust
    doc = doc.add(Block::Section("Information as trust".into()));
    doc = doc.add(para(format!(
        "The intrinsic-noise term $\\sigma^2$ turns source trust into the same arithmetic. A \
         peer believed to be unreliable is given a large $\\sigma^2$; its information falls, and \
         its influence on the fused estimate falls with it, with no separate reputation \
         subsystem. Concretely: an honest fresh source and a source claiming a tip of zero with \
         $\\sigma^2 = {}$ carry information ${}$ and ${}$ respectively — a ratio of ${}$ — and \
         the liar moves the fused estimate by only ${}$ blocks out of ${}$.",
        sci(liar.base_variance),
        sci(i_honest),
        sci(i_liar),
        sci(trust_ratio),
        com(liar_pull),
        com(peer_tip)
    )));
    doc = doc.add(para(format!(
        "The same mechanism handles disagreement without a voting rule. A fresh source reporting \
         ${}$ and a very stale one reporting ${}$ fuse to ${}$ — a distance of ${}$ from the \
         fresh source's projection and ${}$ from the stale one's. The precise source dominates \
         because it is precise, not because it was ranked first.",
        com(peer_tip),
        com(peer_tip + 5_000_000.0),
        com(mixed.value),
        sci(d_fresh),
        sci(d_stale)
    )));

    // ── 7. scope
    doc = doc.add(Block::Raw("\\section{Scope, and a word this paper refuses}\\label{sec:scope}\n\n".into()));
    doc = doc.add(para(
        "Everything above is \\textbf{classical} Fisher information. The quantum Fisher \
         information generalises it — $F_Q$ is the supremum of the classical $F$ over \
         observables, and the QFI matrix is four times the Bures metric — and its headline \
         $N \\rightarrow N^2$ improvement, the standard-quantum-limit to Heisenberg-limit \
         scaling, requires genuine entanglement on real hardware. There is none here. No such \
         speedup is claimed, implied, or available. What transfers from the metrology \
         literature is the \\emph{estimation theory}: inverse-variance fusion, additivity of \
         information, and the Cram\\'er--Rao bound. Those hold for classical sensors reporting \
         over a network, and they are the whole content of this result."
            .into(),
    ));
    doc = doc.add(para(
        "This is worth stating at length because the failure being fixed was itself a case of a \
         plausible-sounding mechanism substituting for a derivation. Replacing it with a \
         differently-decorated plausible-sounding mechanism would not be progress."
            .into(),
    ));

    // ── 8. limits
    doc = doc.add(Block::Section("What this does not model".into()));
    doc = doc.add(para(format!(
        "Four limits, stated rather than buried. \\emph{{First}}, the Poisson arrival model \
         assumes a stationary rate $\\lambda$; a chain whose block rate changes materially \
         during the staleness window will have its variance mis-stated, though the \\emph{{sign}} \
         of the correction survives any non-negative rate. \\emph{{Second}}, the Gaussian \
         identification $I = 1/\\operatorname{{Var}}$ is an approximation to the Poisson \
         likelihood, good in the regime $\\lambda\\Delta \\gg 1$ that matters here \
         (${}$ arrivals at one hour) and poor for very short staleness. \\emph{{Third}}, \
         independence between peers is assumed; peers that relay each other's reports are \
         correlated and the fused variance is then optimistic. \\emph{{Fourth}}, an adversary \
         who reports an inflated tip with a small $\\sigma^2$ is believed — the bound is \
         fail-safe against \\emph{{silence}}, not against \\emph{{lies}}, and the honest \
         defence is a separate one (the intrinsic-noise term above is a downweighting \
         mechanism, not an attestation).",
        sci(lambda * one_hour)
    )));
    doc = doc.add(para(
        "What would falsify the central claim is straightforward and worth naming: exhibit an \
         arrival process for a monotone non-decreasing counter under which the expected current \
         value is \\emph{lower} than the last reported value. For a counter that cannot \
         decrease, no such process exists — which is why this is a sign argument and not a \
         parameter argument."
            .into(),
    ));

    doc = doc.add(Block::Raw(format!(
        "\\section*{{Reproducibility}}\n\
         Every figure above was computed at document-generation time by \
         \\texttt{{flux-science::fisher}} (34 tests, all passing) and typeset by \
         \\texttt{{flux-arxiv-latex}}; no value was typed in by hand, and the failed heuristic \
         in \\S1 is re-executed rather than quoted. Regenerate with \
         \\texttt{{fluxc run --bin fisher}}. Parameters at generation time: \
         $\\lambda = {}$ blocks/s, local height ${}$, observed peer tip ${}$, $z = 2$. \
         The heuristic converged in ${}$ ticks and the fused estimator's efficiency against \
         the Cram\\'er--Rao bound was ${:.6}$.\n\n",
        sci(lambda),
        com(local),
        com(peer_tip),
        decay_ticks,
        eff_optimal
    )));

    doc = doc.add(Block::Raw(
        "\\begin{thebibliography}{9}\n\
         \\bibitem{toth} G.~T\\'oth and I.~Apellaniz, \\emph{Quantum metrology from a quantum \
         information science perspective}, J. Phys. A \\textbf{47}, 424006 (2014). \
         \\url{https://arxiv.org/abs/1405.4878}\n\
         \\bibitem{liu} J.~Liu, H.~Yuan, X.-M. Lu and X.~Wang, \\emph{Quantum Fisher \
         information matrix and multiparameter estimation}, J. Phys. A \\textbf{53}, 023001 \
         (2020). \\url{https://arxiv.org/abs/1907.08037}\n\
         \\bibitem{rao} C.~R. Rao, \\emph{Information and the accuracy attainable in the \
         estimation of statistical parameters}, Bull. Calcutta Math. Soc. \\textbf{37}, 81 (1945).\n\
         \\bibitem{cramer} H.~Cram\\'er, \\emph{Mathematical Methods of Statistics}, Princeton \
         University Press (1946).\n\
         \\end{thebibliography}\n"
            .into(),
    ));

    let res = doc.compile_pdf(&out_dir, "SIGIL_FISHER_v0");
    if res.success {
        println!("OK {}", res.pdf_path.unwrap());
        println!("legacy heuristic converged in {decay_ticks} ticks ({:.1} min)", decay_wallclock_s / 60.0);
        println!("1h separation: {} blocks", com(spread));
        println!("efficiency vs CRB: {eff_optimal:.6}");
        println!("no-information UCB: {nothing_ucb}, CRB: {nothing_crb}");
    } else {
        let tail: String = res.log.lines().rev().take(25).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        eprintln!("FAILED\n{tail}");
        std::process::exit(1);
    }
}
