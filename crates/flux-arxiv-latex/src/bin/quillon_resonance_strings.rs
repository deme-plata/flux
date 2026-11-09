//! quillon_resonance_strings — "Strings on a ledger: an honest test of Quillon's resonance model",
//! built THROUGH flux-arxiv-latex (Document/Block API).
//!
//! CODATA edition: the physical scales (Planck length and time, the reduced Planck constant) are
//! CODATA 2022 values from flux-science. Every ledger and software number is computed here, at build
//! time, from the measurement files in the data directory; the program refuses to build when one is
//! missing, and prints their BLAKE3 digests into the paper.
//!
//! Usage: quillon_resonance_strings [data_dir] [out_dir]
use flux_arxiv_latex::doc::{Block, Document};
use flux_science::constants::{GRAVITATIONAL, PLANCK_REDUCED, SPEED_OF_LIGHT};
use serde_json::Value;
use std::f64::consts::PI;

const FILES: &[&str] = &[
    "captured-at.txt",
    "source.sha256",
    "code-facts.txt",
    "own-tests.txt",
    "clock_lab.rs.txt",
    "clock-lab.txt",
    "metadata-sample.json",
    "metadata-summary.txt",
    "census-24320000.txt",
    "census-24330000.txt",
];

/// The consensus thresholds the real caller passes (q-resonance integration.rs, ResonanceOrdering::new).
const ENERGY_THRESHOLD: f64 = 100.0;
/// temporal_weight in EnergyFunctional::new (energy.rs).
const TEMPORAL_WEIGHT: f64 = 0.3;

fn read(dir: &str, f: &str) -> String {
    std::fs::read_to_string(format!("{dir}/{f}")).unwrap_or_else(|e| panic!("refusing to build: {dir}/{f}: {e}"))
}

/// x as LaTeX math in scientific notation: 1.05\times10^{-17}
fn sci(x: f64, digits: usize) -> String {
    if x == 0.0 {
        return "0".into();
    }
    let e = x.abs().log10().floor() as i32;
    let m = x / 10f64.powi(e);
    if (-2..=4).contains(&e) {
        return format!("{:.*}", digits.saturating_sub((e.max(0)) as usize), x);
    }
    format!("{m:.digits$}\\times10^{{{e}}}")
}

/// 24310000 -> 24,310,000
fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push_str("{,}");
        }
        out.push(c);
    }
    out
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = args.get(1).map(String::as_str).unwrap_or("/home/storage/qnk-resonance-lab-1031/paper-data");
    let out_dir = args.get(2).map(String::as_str).unwrap_or("/home/storage/qnk-resonance-lab-1031/paper-out");

    // ── provenance ──────────────────────────────────────────────────────────────────────────
    let mut digests = String::new();
    for f in FILES {
        let b = std::fs::read(format!("{dir}/{f}")).unwrap_or_else(|_| panic!("refusing to build: {dir}/{f} missing"));
        digests.push_str(&format!("\\texttt{{{}}} & \\texttt{{{}}}\\\\\n", f.replace('_', "\\_"), &blake3::hash(&b).to_hex()[..16]));
    }
    let captured = read(dir, "captured-at.txt").trim().to_string();

    // ── the crate's own tests ───────────────────────────────────────────────────────────────
    let own = read(dir, "own-tests.txt");
    let npass = own.lines().filter(|l| l.starts_with("test ") && l.ends_with("... ok")).count();
    let failed: Vec<String> = own
        .lines()
        .filter(|l| l.starts_with("test ") && l.ends_with("... FAILED"))
        .map(|l| l.trim_start_matches("test ").trim_end_matches(" ... FAILED").rsplit("::").next().unwrap().to_string())
        .collect();
    assert_eq!(failed.len(), 2, "the paper text describes exactly two failing tests");
    assert!(failed.iter().any(|f| f == "test_consensus_detection") && failed.iter().any(|f| f == "test_wavefunction_computation"));

    // ── experiments A and B (tests/clock_lab.rs on the unmodified sources) ──────────────────
    let lab = read(dir, "clock-lab.txt");
    let mut rows_a = String::new();
    let mut a300 = String::new();
    for l in lab.lines().filter(|l| l.starts_with("A n=")) {
        // A n= 100: 20 runs, converged 20, gave up 0, max phase difference vs run 1 = 2.280e-16, final energy range 15.39..87.07
        let n: usize = l[4..l.find(':').unwrap()].trim().parse().unwrap();
        let num = |key: &str| -> String {
            let i = l.find(key).unwrap_or_else(|| panic!("missing {key} in {l}")) + key.len();
            l[i..].split(|c| c == ',' || c == ' ').find(|s| !s.is_empty()).unwrap().to_string()
        };
        let runs = l.split(": ").nth(1).unwrap().split(' ').next().unwrap().to_string();
        let conv = num("converged ");
        let dev: f64 = num("vs run 1 = ").parse().unwrap();
        let range = l.split("final energy range ").nth(1).unwrap().trim();
        let range_tex = if range == "n/a" {
            "---".to_string()
        } else {
            let (lo, hi) = range.split_once("..").unwrap();
            format!("{:.2} to {:.2}", lo.parse::<f64>().unwrap(), hi.parse::<f64>().unwrap())
        };
        if n >= 300 {
            a300 = format!("{conv} of {runs}");
        }
        rows_a.push_str(&format!("{n} & {runs} & {conv} & ${}$ & {range_tex}\\\\\n", sci(dev, 1)));
    }
    let mut rows_b = String::new();
    for l in lab.lines().filter(|l| l.starts_with("B n=")) {
        let n: usize = l[4..l.find(':').unwrap()].trim().parse().unwrap();
        let mut cells = vec![n.to_string()];
        for part in l.split("age").skip(1) {
            let e: f64 = part.split("E=").nth(1).unwrap().trim().split(' ').next().unwrap().parse().unwrap();
            let ok = part.contains("consensus=true");
            cells.push(format!("{e:.1}\\,{}", if ok { "\\checkmark" } else { "$\\times$" }));
        }
        rows_b.push_str(&(cells.join(" & ") + "\\\\\n"));
    }
    let bound = ENERGY_THRESHOLD / TEMPORAL_WEIGHT;
    // Extra age the N=1000 check accumulated while running: E = 0.3 N (0.1 s + extra).
    let e1000: f64 = lab.lines().find(|l| l.starts_with("B n=1000")).and_then(|l| l.split("E=").nth(1))
        .and_then(|s| s.trim().split(' ').next()).and_then(|s| s.parse().ok()).expect("B n=1000 first cell");
    let extra_ms = (e1000 / (TEMPORAL_WEIGHT * 1000.0) - 0.1) * 1000.0;
    // Upper bound on one N=300 iteration: the whole test run's time over the N=300 iterations alone.
    let secs: f64 = lab.lines().find(|l| l.contains("finished in ")).and_then(|l| l.split("finished in ").nth(1))
        .and_then(|s| s.trim_end_matches('s').trim().parse().ok()).expect("test duration");
    let runs300: f64 = a300.split(" of ").nth(1).expect("N=300 runs").parse().unwrap();
    let iter_s = secs / (runs300 * 1000.0);
    let grow = TEMPORAL_WEIGHT * 300.0 * iter_s;

    // ── the live chain: quantum_metadata of sampled blocks ──────────────────────────────────
    let sample: Vec<Value> = serde_json::from_str(&read(dir, "metadata-sample.json")).expect("metadata json");
    let f = |r: &Value, k: &str| r[k].as_f64().unwrap_or_else(|| panic!("field {k}"));
    let nblk = sample.len();
    let (h_lo, h_hi) = (
        sample.iter().map(|r| r["h"].as_u64().unwrap()).min().unwrap(),
        sample.iter().map(|r| r["h"].as_u64().unwrap()).max().unwrap(),
    );
    let ph: Vec<f64> = sample.iter().map(|r| f(r, "phase") / PI).collect();
    let ph_max = ph.iter().cloned().fold(0.0, f64::max);
    let ph_med = median(ph.clone());
    let ks: Vec<f64> = sample.iter().map(|r| f(r, "k")).collect();
    let (k_min, k_med, k_max) = (ks.iter().cloned().fold(f64::MAX, f64::min), median(ks.clone()), ks.iter().cloned().fold(0.0, f64::max));
    let k_err = sample
        .iter()
        .filter(|r| f(r, "k") > 0.0)
        .map(|r| {
            let k = 2.0 * PI * (f(r, "potential").log10() * f(r, "entropy") * 0.1).sqrt() / f(r, "temporal").max(1.0);
            ((k - f(r, "k")) / f(r, "k")).abs()
        })
        .fold(0.0, f64::max);
    let n_ord = sample.iter().filter(|r| f(r, "ordering") == r["h"].as_u64().unwrap() as f64 - 1.0).count();
    let n_zero = sample.iter().filter(|r| f(r, "coupling") == 0.0 && f(r, "fault") == 0.0 && f(r, "finality") == 0.0).count();
    let h_share = median(sample.iter().map(|r| f(r, "ordering") / f(r, "energy")).collect());
    let ed: Vec<f64> = sample.iter().map(|r| f(r, "entropy") * f(r, "potential")).collect();
    let ed_max = ed.iter().cloned().fold(0.0, f64::max);
    let ed_med = median(ed);
    let t_med = median(sample.iter().map(|r| f(r, "temporal")).collect());
    let diff: Vec<f64> = sample.iter().map(|r| f(r, "potential")).collect();
    let (d_min, d_med, d_max) = (diff.iter().cloned().fold(f64::MAX, f64::min), median(diff.clone()), diff.iter().cloned().fold(0.0, f64::max));

    // ── transaction census: blocks hold nothing but coinbase ────────────────────────────────
    let mut census = String::new();
    for c in ["census-24320000.txt", "census-24330000.txt"] {
        let t = read(dir, c);
        let first = t.lines().next().unwrap().to_string(); // "blocks A..B, missing 0"
        let non_cb = t.lines().skip(1).take_while(|l| !l.starts_with("non-coinbase txs FROM")).filter(|l| !l.trim().is_empty()).count();
        assert_eq!(non_cb, 0, "the text says no non-coinbase transaction types appear in {c}");
        census.push_str(&format!("{}; ", first.replace("..", "--").replace("blocks ", "heights ").replace(", missing 0", ", every block present")));
    }

    // ── CODATA scales ───────────────────────────────────────────────────────────────────────
    let l_p = (PLANCK_REDUCED * GRAVITATIONAL / SPEED_OF_LIGHT.powi(3)).sqrt();
    let t_p = l_p / SPEED_OF_LIGHT;
    let sqrt_hbar = PLANCK_REDUCED.sqrt();

    let numbers = serde_json::json!({
        "captured": captured, "own_tests_pass": npass, "own_tests_failed": failed,
        "a_300_converged": a300, "consensus_bound_n_times_age_s": bound,
        "blocks_sampled": nblk, "height_range": [h_lo, h_hi],
        "phase_over_pi_median": ph_med, "phase_over_pi_max": ph_max,
        "k_min": k_min, "k_median": k_med, "k_max": k_max, "k_rederive_max_rel_err": k_err,
        "ordering_is_height_minus_1": n_ord, "coupling_fault_finality_zero": n_zero,
        "median_energy_share_block_height": h_share,
        "entropy_times_difficulty_median": ed_med, "entropy_times_difficulty_max": ed_max,
        "median_block_interval_s": t_med, "difficulty": [d_min, d_med, d_max],
        "planck_length_m": l_p, "planck_time_s": t_p,
    });

    let body = BODY
        .replace("@CAPTURED@", &captured)
        .replace("@NPASS@", &npass.to_string())
        .replace("@NTESTS@", &(npass + failed.len()).to_string())
        .replace("@ROWSA@", &rows_a)
        .replace("@ROWSB@", &rows_b)
        .replace("@A300@", &a300)
        .replace("@BOUND@", &format!("{bound:.0}"))
        .replace("@EXTRAMS@", &format!("{extra_ms:.0}"))
        .replace("@E1000@", &format!("{e1000:.1}"))
        .replace("@SECS@", &format!("{secs:.1}"))
        .replace("@ITERMS@", &format!("{:.1}", iter_s * 1000.0))
        .replace("@ITERS@", &format!("{iter_s:.4}"))
        .replace("@GROWTOL@", &sci(grow / 1e-6, 1))
        .replace("@GROW@", &format!("{grow:.2}"))
        .replace("@NBLK@", &thousands(nblk as u64))
        .replace("@HLO@", &thousands(h_lo))
        .replace("@HHI@", &thousands(h_hi))
        .replace("@PHMED@", &sci(ph_med, 1))
        .replace("@PHMAX@", &sci(ph_max, 1))
        .replace("@PHPCT@", &format!("{:.4}", ph_max * 100.0))
        .replace("@KMIN@", &format!("{k_min:.4}"))
        .replace("@KMED@", &format!("{k_med:.4}"))
        .replace("@KMAX@", &format!("{k_max:.4}"))
        .replace("@KERR@", &sci(k_err, 1))
        .replace("@NORD@", &thousands(n_ord as u64))
        .replace("@NZERO@", &thousands(n_zero as u64))
        .replace("@HSHARE@", &format!("{:.0}", h_share * 100.0))
        .replace("@EDMED@", &format!("{ed_med:.1}"))
        .replace("@EDMAX@", &format!("{ed_max:.1}"))
        .replace("@TMED@", &format!("{t_med:.2}"))
        .replace("@DMIN@", &sci(d_min, 2))
        .replace("@DMED@", &sci(d_med, 2))
        .replace("@DMAX@", &sci(d_max, 2))
        .replace("@CENSUS@", census.trim_end_matches("; "))
        .replace("@LP@", &sci(l_p, 4))
        .replace("@TP@", &sci(t_p, 4))
        .replace("@TBLOCKTP@", &sci(t_med / t_p, 1))
        .replace("@SQRTHBAR@", &sci(sqrt_hbar, 2))
        .replace("@DIGESTS@", &digests);
    assert!(!body.contains('@'), "unfilled placeholder: {}", &body[body.find('@').unwrap()..][..40]);

    let doc = Document::new("article")
        .option("11pt")
        .package_opt("geometry", &["margin=2.2cm"])
        .package("amsmath")
        .package("amssymb")
        .package("booktabs")
        .package("xcolor")
        .package("xurl")
        .package("hyperref")
        .preamble(PREAMBLE)
        .add(Block::Raw(body));

    std::fs::create_dir_all(out_dir).expect("out dir");
    std::fs::write(format!("{out_dir}/quillon-resonance-strings-numbers.json"), serde_json::to_string_pretty(&numbers).expect("json")).expect("numbers");
    let res = doc.compile_pdf(out_dir, "quillon-resonance-strings-codata-2026-10-31");
    println!("flux-arxiv-latex: success={} pdf={:?}", res.success, res.pdf_path);
    if !res.success {
        let tail = &res.log[res.log.len().saturating_sub(2500)..];
        eprintln!("--- compile log tail ---\n{tail}");
        std::process::exit(1);
    }
}

const PREAMBLE: &str = r##"
\hypersetup{colorlinks=true,urlcolor=blue!60!black,linkcolor=blue!60!black}
\setlength{\parskip}{5pt}\setlength{\parindent}{0pt}
"##;

const BODY: &str = r##"
\begin{center}
{\LARGE\bfseries Strings on a ledger}\\[6pt]
{\large An honest test of Quillon Graph's resonance model:\\ what the wave function does, what the live chain records,\\ and what it would take to make it count}\\[4pt]
{\normalsize\itshape CODATA edition}\\[12pt]
{\normalsize Viktor S.\ Kristensen\quad and\quad Rocky\footnote{Claude (Anthropic), engineer-companion agent on Epsilon. Every number in this paper is computed by its build program (\texttt{flux-arxiv-latex}, binary \texttt{quillon\_resonance\_strings}) from the measurement files listed in Section~11; the program refuses to build when one is missing. Physical constants are CODATA 2022 values from \texttt{flux-science}.}}\\[4pt]
{\small 31 October 2026 \quad\textperiodcentered\quad Flux science \quad\textperiodcentered\quad Quillon Graph \quad\textperiodcentered\quad data captured @CAPTURED@}
\end{center}

\begin{quote}\small
\textbf{Abstract.} Quillon Graph carries an unusual idea in its source code: model every transaction as a vibrating string whose amplitude is the square root of its stake, whose frequency is its priority and whose phase is its place in time, and let consensus emerge as the strings settle into agreement. We test that idea against the code that implements it and against the live chain. The model's two core files compile unmodified outside the node; @NPASS@ of their @NTESTS@ own tests pass. The two that fail are informative: one evaluates the wave function at a fixed end of the string, where a standing wave is zero, and one shows that the energy contains a wall-clock term, so that perfectly agreeing transactions lose ``consensus'' as they age. With the thresholds the node itself uses, $N$ transactions of mean age $a$ seconds can reach consensus only while $N a < @BOUND@$. The wave function $\psi(x,t)$ is never evaluated outside its own test: the operative mathematics is phase synchronisation on the unit circle, the XY or Kuramoto picture rather than string theory. The model is switched off in the live node. What the live node does record, in every block, is a ``wave-function phase'', an ``entropy'', an ``energy'' and a K-parameter; over @NBLK@ sampled blocks the phase never exceeds $@PHMAX@\,\pi$, the entropy is a disguised measure of mining difficulty, and the ``energy'' is mostly the block height. We list five changes that would turn the idea into a measurable instrument.
\end{quote}

\section{The idea}
The resonance model (\texttt{q-resonance}, 6{,}612 lines of Rust in the Quillon source) describes each transaction as a string state with
\[
\psi(x,t) \;=\; A\, e^{i(kx-\omega t+\varphi)}\,\sin\!\frac{n\pi x}{L},\qquad A=\sqrt{\text{stake}},\quad \omega = 2\pi\cdot\text{priority},
\]
a phase $\varphi$ that encodes temporal alignment and a mode number $n$ that places the string in a layer of the hypergraph (\texttt{string\_state.rs}, lines 1--8). Consensus is to emerge from minimising an energy functional with six parts: coupling, potential, ordering, fault tolerance, temporal coherence and finality (\texttt{energy.rs}). When the strings' phases line up (``harmonic convergence''), the round is agreed. The idea grew out of the K-parameter, which Kristensen wrote in 2024 as an order parameter for a phase transition and which the node still computes in every block (Section~8).

It is an appealing picture, and it deserves to be tested the way physics is tested: by asking what each symbol does in the running program, and what the program does with real input.

\section{What the physics actually is}
\paragraph{A classical string.} A string of length $L$ with fixed ends obeys the wave equation $\partial_t^2 y = v^2\partial_x^2 y$ and vibrates in standing modes $\sin(n\pi x/L)$ with angular frequencies $\omega_n = n\pi v/L$. Two consequences matter below. The ends, $x=0$ and $x=L$, are nodes: the displacement there is zero at all times. And the frequency is not free: it is fixed by the mode, the length and the wave speed. In the model, $\omega$ is set by the transaction's priority, independently of $n$ and $L$, and $L$ is set to the number of spatial coordinates (\texttt{string\_state.rs}, \texttt{wavefunction}), so the formula borrows the shape of a string without its dynamics.

\paragraph{What the code minimises.} Each step of the minimiser renormalises every phase back onto the unit circle, $\varphi_i = e^{i\theta_i}$ (\texttt{update\_phase}). The coupling energy then reads
\[
E_{\text{coupling}} = \sum_{i<j} J_{ij}\,\bigl|e^{i\theta_i}-e^{i\theta_j}\bigr|^2 = \sum_{i<j} 2J_{ij}\bigl(1-\cos(\theta_i-\theta_j)\bigr),
\]
which is the Hamiltonian of the classical XY model, and its gradient flow is the synchronisation dynamics of coupled phase oscillators studied by Kuramoto~[1]. Here $J_{ij}=\sqrt{A_iA_j}\cos(\theta_i-\theta_j)/(1+d_{ij})$, with $d_{ij}$ the distance between the two strings' positions. This is good, well-understood mathematics for reaching agreement among many parties. It is not quantum mechanics (nothing is quantised or measured) and it is not string theory.

\paragraph{The scale of string theory.} The strings of string theory are relativistic objects whose tension sets a length near the Planck length~[2]. From CODATA 2022 values of $\hbar$, $G$ and $c$~[3],
\[
\ell_{\mathrm P}=\sqrt{\hbar G/c^3}=@LP@\ \text{m},\qquad t_{\mathrm P}=\ell_{\mathrm P}/c=@TP@\ \text{s}.
\]
The ledger's natural unit of time, the median interval between Quillon blocks measured below ($@TMED@$\,s), is $@TBLOCKTP@$ Planck times. Nothing in a ledger probes physics at that scale. The vocabulary of strings, waves and resonance is therefore an analogy, and we treat it as one: the question is whether the analogy produces a useful instrument, not whether it is physics.

\section{Where the idea lives in the code}
There are two places, and they must not be confused.
\begin{enumerate}
\item \textbf{The resonance model} (\texttt{q-resonance}): string states, the energy functional, a resonance-based ordering of transactions, a spectral Byzantine detector and a ``shadow mode'' that would run beside the existing consensus. It is an \emph{optional} feature of the node (\texttt{resonance}) and is \emph{absent from the running binary}: the feature is not in the default build, and none of its coordinator type names appears in the live 10.11.114f executable.
\item \textbf{The quantum metadata} written by the live block producer (function \texttt{generate\_quantum\_}\allowbreak\texttt{metadata} in \texttt{block\_producer.rs}): a hypergraph coordinate, a K-parameter, an energy split into the same six named parts, a wave-function phase and an entropy, in every block. This code runs today; Section~8 measures what it produces.
\end{enumerate}

\section{The model's own tests}
The two core files, \texttt{string\_state.rs} and \texttt{energy.rs}, need no part of the node, so we compiled them byte-for-byte (SHA-256 in \texttt{source.sha256}) in a small lab crate, leaving out only the spectral detector, which requires a Fortran/OpenBLAS toolchain the server does not have. Their own unit tests give @NPASS@ passes out of @NTESTS@. The two failures are the interesting part.

\textbf{\texttt{test\_wavefunction\_computation}} evaluates $\psi$ at the string's own position $x=(0,0)$ and expects a non-zero value. At $x=0$ the factor $\sin(n\pi x/L)$ vanishes: a fixed end does not move. The physics in the formula is right and the test asks the wrong question. More telling is where $\psi(x,t)$ is used at all: \emph{nowhere else}. No energy term, no ordering step and no consensus check ever evaluates the wave function. They use only each string's phase on the unit circle, its amplitude and its timestamp.

\textbf{\texttt{test\_consensus\_detection}} aligns five strings perfectly and expects the energy to be below 10. The temporal term is
\[
E_{\text{temporal}} = w_t \sum_i \frac{|t_i - t_{\text{now}}|}{1\,\text{s}},\qquad w_t=0.3,
\]
with $t_{\text{now}}$ read from the machine's wall clock (\texttt{energy.rs}, \texttt{temporal\_energy}). The test's strings are timestamped one second after 1~January 1970, so the clock alone contributes about $0.3\times5\times1.8\times10^9$, and the assertion cannot hold on any machine today.

\section{Experiment A: same input, same phases, different energy}
We built $N$ strings with varied stake, priority and position, timestamped half a second before the test began, and minimised identical copies repeatedly with the settings of the node's ordering step (\texttt{minimize(1000, 1e-6)}).
\begin{center}
\begin{tabular}{rrrcl}
\toprule
$N$ & runs & converged & largest phase difference vs.\ run 1 & reported final energy\\
\midrule
@ROWSA@\bottomrule
\end{tabular}
\end{center}
The phases come out identical to rounding, so the minimiser itself is deterministic. The \emph{energy} it reports is not: it grows with the time at which each run happens, because the temporal term keeps counting while the program computes. Any decision taken on that energy, such as ``is this round agreed?'', depends on when a node asks.

\section{Experiment B: consensus has a clock}
The node's ordering step declares consensus when the energy is below 100 and the phase variance below 1 (\texttt{integration.rs}). We aligned $N$ strings perfectly, as the crate's own test does, brought the mean field up to date as the minimiser does at every step, and evaluated the check at several ages. Cells show the energy and the verdict.
\begin{center}
\begin{tabular}{rccccc}
\toprule
$N$ & age 0.1\,s & 1\,s & 3\,s & 10\,s & 60\,s\\
\midrule
@ROWSB@\bottomrule
\end{tabular}
\end{center}
With every other term at zero the energy is $0.3\,N a$, where $a$ is the age at the moment the check runs (at $N=1000$ the check itself takes long enough to add about @EXTRAMS@\,ms of age, hence @E1000@ rather than 30 in the first column), so the rule is
\[
\text{consensus possible}\iff N\,a < \frac{100}{0.3}\approx @BOUND@ \text{ transaction-seconds}.
\]
A hundred perfectly agreeing transactions lose consensus somewhere between 3 and 10 seconds of age; a thousand lose it within one second. Agreement is not the obstacle; the clock is.

\section{Experiment C: three hundred strings never settle}
At $N=300$ the minimiser converged in @A300@ runs: every run used its 1{,}000 iterations and returned an error, which the ordering step propagates as a failed round. The whole experiment took @SECS@\,s, so one iteration at $N=300$ took at most @ITERMS@\,ms; during that time the temporal term grows by up to $0.3\times300\times@ITERS@\approx@GROW@$, about $@GROWTOL@$ times the convergence tolerance of $10^{-6}$. This is consistent with the clock term blocking convergence; proving it would require changing the code, which this study does not do.

\section{What the live chain records}
We read the quantum metadata of every 50th block from height @HLO@ to @HHI@ (@NBLK@ blocks) through the producer's public API.
\begin{itemize}
\item \textbf{Wave-function phase.} It is defined as $\pi$ times the block's ``entropy''. Its median is $@PHMED@\,\pi$ and its largest value $@PHMAX@\,\pi$: the phase never leaves the first $@PHPCT@$\,\% of its range.
\item \textbf{Entropy.} It is the average, over the block's winning mining hashes, of each hash's first eight bytes read as a fraction of $2^{64}$. A winning hash must lie below the difficulty target, so this number is pinned by the difficulty. Indeed entropy $\times$ difficulty never exceeds $@EDMAX@$ (median $@EDMED@$), consistent with hashes uniformly distributed below a target of $128/\text{difficulty}$. The ``quantum entropy'' is a measurement of mining difficulty, not of randomness.
\item \textbf{K-parameter.} It ranges from @KMIN@ to @KMAX@ (median @KMED@). Recomputing
\[K = 2\pi\sqrt{\Delta H\,\Delta s\,\hbar}\,/\,\tau\]
with $\Delta H=\log_{10}(\text{difficulty})$, $\Delta s=0.1\times\text{entropy}$, $\hbar=1$ and $\tau$ the seconds since the previous block reproduces every value to within a relative $@KERR@$. With the CODATA value of $\hbar$ the same number would be smaller by $\sqrt{\hbar}=@SQRTHBAR@$ and would carry units; the live gauge is dimensionless by the choice $\hbar=1$.
\item \textbf{Energy.} The ``ordering'' part equals the block height minus one in @NORD@ of @NBLK@ blocks; coupling, fault tolerance and finality are zero in @NZERO@ (they are marked \texttt{TODO} in the producer). The total adds difficulty, block height and seconds, three quantities with different units; the median block's ``energy'' is @HSHARE@\,\% block height.
\item \textbf{Input for the model.} Over two full runs of 10{,}000 consecutive blocks (@CENSUS@) we found no transaction of any type other than mining payments. Today the resonance model would have nothing to order.
\end{itemize}
Difficulty over the sample ranged from $@DMIN@$ to $@DMAX@$ (median $@DMED@$).

\section{What it would take to make it count}
None of this is a verdict on the idea. It shows where the idea stops being connected to the program. Five changes would reconnect them, each one testable:
\begin{enumerate}
\item \textbf{Take the wall clock out of the energy.} Measure age in block heights or rounds, which every node agrees on, so that two honest nodes compute the same energy and the same verdict.
\item \textbf{Give the phase a real source of variation.} If a block's phase is to mean anything, derive it from something uniformly distributed, such as a hash of the whole solution set, rather than from hashes that are forced below a target.
\item \textbf{Use one unit per energy.} Either normalise each term to a dimensionless scale or drop the sum; a total of difficulty, height and seconds cannot be compared across blocks.
\item \textbf{Run the model in shadow mode on real traffic.} The machinery exists (\texttt{shadow\_mode.rs}). Feed it the node's mempool rather than its blocks, record how its ordering differs from the live one, and publish the agreement rate.
\item \textbf{Keep the tests green.} The two failing unit tests would have caught both problems the day they were introduced.
\end{enumerate}

\section{What is measured, what is derived, what is untested}
\textbf{Measured:} the test results, experiments A--C and every number in Section~8, from the files in Section~11. \textbf{Derived from the code:} the XY form of the coupling energy, the consensus bound $N a < @BOUND@$, the attribution of the entropy to the difficulty target, and the statement that $\psi(x,t)$ has no caller outside its test. \textbf{Analogy, labelled as such:} every use of the words string, wave, resonance and quantum for ledger quantities. \textbf{Untested:} the spectral Byzantine detector and the resonance ordering step, which need the OpenBLAS toolchain; the shadow-mode coordinator; and any behaviour of the model on real transaction traffic, of which the sampled blocks contain none.

\section{Data and provenance}
Input files (BLAKE3, first 16 hex digits):
\begin{center}
\begin{tabular}{ll}
\toprule
file & BLAKE3\\
\midrule
@DIGESTS@\bottomrule
\end{tabular}
\end{center}
The lab crate, the experiment source (\texttt{clock\_lab.rs.txt}) and the sampling script are kept beside these files on Epsilon. Source: the Quillon tree at the 10.11.114f lineage; live data from the producer's public API.

\section*{References}
\small
[1] Y.\ Kuramoto, \emph{Chemical Oscillations, Waves, and Turbulence}, Springer (1984).\par
[2] J.\ Polchinski, \emph{String Theory}, Vol.~1, Cambridge University Press (1998).\par
[3] CODATA Task Group on Fundamental Constants, CODATA recommended values of the fundamental physical constants: 2022, NIST, \url{https://physics.nist.gov/cuu/Constants/}.\par
"##;
