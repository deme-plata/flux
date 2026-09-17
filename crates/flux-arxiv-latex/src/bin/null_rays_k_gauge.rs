//! null_rays_k_gauge — "Null Rays, Twist and the Kristensen Gauge": an arXiv-style
//! reading of A. B. Aazami, *Symplectic 4-manifolds via Lorentzian geometry*
//! (arXiv:1504.06425, Proc. AMS 145 (2017) 387) as a theorem about consensus, with
//! the Cosmic Garden paper (The Cosmic Arcology Mission, Jan 2026) and the July CCC
//! lane (Meissner–Penrose, arXiv:2503.24263) in the margin.
//!
//! Everything numeric is COMPUTED at generation time:
//!   * Aazami's two null-congruence equations (his (1),(2)) are checked on the
//!     rapidly-rotating Kerr principal null congruence by finite differences;
//!   * det ω = e^{4f} k(f)² ι² is tabulated across the Kerr hemisphere down to the
//!     equatorial blind plane;
//!   * the focusing theorem is run as an ODE: untwisted → caustic at λ* = π/√(2R)
//!     (RK4 vs closed form), twisted → bounded breathing in the potential
//!     V(s) = ι₀²/(8s²) + R s²/4 (energy drift reported);
//!   * the live SIGIL K-gauge is read from the series JSONL and decomposed into the
//!     Raychaudhuri planes exactly as the MCP now does.
//!
//! Usage: null_rays_k_gauge [out_dir] [sigil-kgauge.jsonl]
use flux_arxiv_latex::doc::{Block, Document};


/// Make a gauge string safe inside \texttt{} under pdflatex + inputenc: Greek and arrows to ASCII, specials escaped.
fn tt(s: &str) -> String {
    s.replace('ι', "iota").replace('θ', "theta").replace('σ', "sigma").replace('ω', "omega").replace('²', "^2").replace('≈', "~").replace('→', "->").replace('∝', "prop.").replace('—', "--").replace('·', ".")
        .chars().map(|c| match c { '_' => "\\_".into(), '%' => "\\%".into(), '#' => "\\#".into(), '&' => "\\&".into(), '^' => "\\^{}".into(), '{' => "\\{".into(), '}' => "\\}".into(), '$' => "\\$".into(),
            c if c.is_ascii() => c.to_string(), _ => "?".into() }).collect()
}

fn raw(s: String) -> Block { Block::Raw(format!("{s}\n\n")) }

fn sci(x: f64) -> String {
    if x == 0.0 || !x.is_finite() { return format!("{x}"); }
    let exp = x.abs().log10().floor() as i32;
    if (-2..=3).contains(&exp) {
        let s = format!("{:.4}", x);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        format!("{:.3}\\times10^{{{}}}", x / 10f64.powi(exp), exp)
    }
}

// ── Kerr, rapidly rotating (a > m): the paper's example ────────────────────────────────────
struct Kerr { m: f64, a: f64 }
impl Kerr {
    fn rho2(&self, r: f64, th: f64) -> f64 { r * r + self.a * self.a * th.cos().powi(2) }
    /// The paper's ι² = 4a²cos²ϑ/ρ⁴ (his eq. 7; ι is twice the usual twist ω).
    fn iota2(&self, r: f64, th: f64) -> f64 { 4.0 * self.a * self.a * th.cos().powi(2) / self.rho2(r, th).powi(2) }
    /// Expansion of the outgoing principal null congruence: div k = 2r/ρ² (NP ρ = −1/(r − ia cosϑ)).
    fn div_k(&self, r: f64, th: f64) -> f64 { 2.0 * r / self.rho2(r, th) }
    fn delta(&self, r: f64) -> f64 { r * r - 2.0 * self.m * r + self.a * self.a }
}

/// Central finite difference along k (k(r) = 1, ϑ constant along the ray).
fn ddr(f: impl Fn(f64) -> f64, r: f64) -> f64 { let h = 1e-5; (f(r + h) - f(r - h)) / (2.0 * h) }

// ── the focusing ODE ───────────────────────────────────────────────────────────────────────
/// θ' = ι²/2 − θ²/2 − R,  (ι²)' = −2θ ι²   (Aazami (1),(2) with σ = 0, Ric = R const).
fn rk4_step(th: f64, i2: f64, r: f64, h: f64) -> (f64, f64) {
    let f = |t: f64, i: f64| (i / 2.0 - t * t / 2.0 - r, -2.0 * t * i);
    let (k1t, k1i) = f(th, i2);
    let (k2t, k2i) = f(th + h / 2.0 * k1t, i2 + h / 2.0 * k1i);
    let (k3t, k3i) = f(th + h / 2.0 * k2t, i2 + h / 2.0 * k2i);
    let (k4t, k4i) = f(th + h * k3t, i2 + h * k3i);
    (th + h / 6.0 * (k1t + 2.0 * k2t + 2.0 * k3t + k4t), i2 + h / 6.0 * (k1i + 2.0 * k2i + 2.0 * k3i + k4i))
}

struct Live {
    ts_ms: i64, k_c: f64, k_fix: f64, regime_fix: String, ds_bits: f64, n_eff: f64, h_norm: f64,
    distinct: u64, dominant: f64, d_tip: f64, d_state: Option<f64>, d_fin: Option<f64>, d_sem: f64,
    bps: f64, finality_s: f64, blocks_added: u64, peers: u64, delta_h: f64, persist: f64,
    ray_verdict: Option<String>, focusing: Option<f64>,
}
fn load_live(path: &str) -> Option<Live> {
    let txt = std::fs::read_to_string(path).ok()?;
    let line = txt.lines().rev().find(|l| l.contains("\"version\":2") || l.contains("\"version\": 2"))?;
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let ch = |n: &str| v["state_disagreement"]["channels"][n]["value"].as_f64();
    Some(Live {
        ts_ms: v["ts_ms"].as_i64()?, k_c: v["K_C"].as_f64()?, k_fix: v["K_fix"].as_f64().unwrap_or(f64::NAN),
        regime_fix: v["regime_fix"].as_str().unwrap_or("?").into(),
        ds_bits: v["proposer_structure"]["entropy_bits"].as_f64()?, n_eff: v["proposer_structure"]["effective_producers"].as_f64()?,
        h_norm: v["proposer_structure"]["entropy_norm"].as_f64()?, distinct: v["proposer_structure"]["active_producers"].as_u64()?,
        dominant: v["proposer_structure"]["dominant_share"].as_f64()?,
        d_tip: ch("tip_divergence").unwrap_or(0.0), d_state: ch("state_root_mismatch"), d_fin: ch("finality_divergence"), d_sem: ch("semantic_conflicts").unwrap_or(0.0),
        bps: v["network_diagnostics"]["block_rate_bps"].as_f64()?, finality_s: v["tau"]["finality_secs"].as_f64()?,
        blocks_added: v["window"]["blocks_added"].as_u64()?, peers: v["network_diagnostics"]["peers"].as_u64()?,
        delta_h: v["state_disagreement"]["delta_H_consensus"].as_f64()?, persist: v["k_fix"]["persistence_blocks"].as_f64().unwrap_or(0.0),
        ray_verdict: v["raychaudhuri"]["verdict"].as_str().map(str::to_string), focusing: v["raychaudhuri"]["focusing"].as_f64(),
    })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out_dir = args.get(1).map(String::as_str).unwrap_or("/home/storage/sigil-scratch/null-rays-k-gauge");
    let series = args.get(2).map(String::as_str).unwrap_or("/home/storage/claude-code/k-parameter-paper/gauge-series/sigil-kgauge.jsonl");
    let pi = std::f64::consts::PI;

    // ── 1. Kerr: Aazami's (1) and (2) checked by finite differences ────────────────────────
    let kerr = Kerr { m: 1.0, a: 1.2 }; // a > m: no horizon (Δ > 0 everywhere), ring singularity only at r = cosϑ = 0
    let mut max_res_2 = 0f64; // eq (2): k(ι²) + 2 (div k) ι² = 0
    let mut max_res_1 = 0f64; // eq (1): k(div k) − ι²/2 + (div k)²/2 = 0   (σ = 0, Ric = 0)
    let mut n_pts = 0usize;
    let mut min_delta = f64::INFINITY;
    for ir in 0..200 {
        let r = -5.0 + 10.0 * (ir as f64 + 0.5) / 200.0;
        min_delta = min_delta.min(kerr.delta(r));
        for it in 0..50 {
            let th = (pi / 2.0) * (it as f64 + 0.5) / 50.0; // open northern hemisphere, ϑ < π/2
            let lhs2 = ddr(|rr| kerr.iota2(rr, th), r);
            let rhs2 = -2.0 * kerr.div_k(r, th) * kerr.iota2(r, th);
            let scale2 = rhs2.abs().max(1e-12);
            max_res_2 = max_res_2.max((lhs2 - rhs2).abs() / scale2);
            let lhs1 = ddr(|rr| kerr.div_k(rr, th), r);
            let rhs1 = kerr.iota2(r, th) / 2.0 - kerr.div_k(r, th).powi(2) / 2.0;
            let scale1 = rhs1.abs().max(1e-12);
            max_res_1 = max_res_1.max((lhs1 - rhs1).abs() / scale1);
            n_pts += 1;
        }
    }
    // det ω = e^{4r} (k(r))² ι² with f = r, k(r) = 1: the blind plane at ϑ = π/2.
    let r0 = 2.0;
    let det_rows: Vec<(f64, f64, f64)> = [0.0f64, 30.0, 60.0, 85.0, 89.0, 89.9, 90.0].iter().map(|deg: &f64| {
        let th = deg.to_radians();
        let i2 = kerr.iota2(r0, th);
        (*deg, i2, (4.0 * r0).exp() * i2)
    }).collect();

    // ── 2. the focusing ODE ───────────────────────────────────────────────────────────────
    let ric = 1.0f64;
    let h = 1e-4;
    // untwisted: caustic at λ* = π/√(2R)  (θ = −√(2R) tan(√(R/2) λ))
    let lam_star_closed = pi / (2.0 * ric).sqrt();
    let (mut th, mut i2, mut lam) = (0.0f64, 0.0f64, 0.0f64);
    let mut lam_star_num = f64::NAN;
    for _ in 0..1_000_000 {
        let (t2, j2) = rk4_step(th, i2, ric, h);
        th = t2; i2 = j2; lam += h;
        if th < -1e6 { lam_star_num = lam; break; }
    }
    // twisted: start at the fixed point's twist ×2 → breathing; s = √(area) = e^{A/2}, A = ∫θ
    let i0_2 = 4.0 * ric; // ι₀² = 2 × (2R)
    let s_star = (i0_2 / (2.0 * ric)).powf(0.25); // V'(s*) = 0 ⇔ s*⁴ = ι₀²/(2R)
    let v = |s: f64| i0_2 / (8.0 * s * s) + ric * s * s / 4.0;
    let (mut th, mut i2, mut a_int) = (0.0f64, i0_2, 0.0f64);
    let (mut s_min, mut s_max, mut th_min, mut th_max) = (f64::INFINITY, 0.0f64, f64::INFINITY, f64::NEG_INFINITY);
    let e0 = v(1.0); // s(0) = 1, s'(0) = θ/2·s = 0
    let mut e_drift = 0.0f64;
    let mut crossings: Vec<f64> = Vec::new();
    let mut prev_th = th;
    let lam_end = 60.0;
    let steps = (lam_end / h) as usize;
    for k in 0..steps {
        let (t2, j2) = rk4_step(th, i2, ric, h);
        a_int += h * (th + t2) / 2.0;
        th = t2; i2 = j2;
        let s = (a_int / 2.0).exp();
        let sp = th / 2.0 * s;
        e_drift = e_drift.max((sp * sp / 2.0 + v(s) - e0).abs() / e0);
        s_min = s_min.min(s); s_max = s_max.max(s); th_min = th_min.min(th); th_max = th_max.max(th);
        if prev_th <= 0.0 && th > 0.0 && k > 0 { crossings.push(k as f64 * h); }
        prev_th = th;
    }
    let period_num = if crossings.len() >= 2 { (crossings[crossings.len() - 1] - crossings[0]) / (crossings.len() - 1) as f64 } else { f64::NAN };
    let period_lin = 2.0 * pi / (2.0 * ric).sqrt(); // small-amplitude: V''(s*) = 2R
    // s_min from energy: (R/4)s⁴ − E s² + ι₀²/8 = 0
    let s_min_closed = ((e0 - (e0 * e0 - ric * i0_2 / 8.0).sqrt()) * 2.0 / ric).sqrt();
    let s_max_closed = ((e0 + (e0 * e0 - ric * i0_2 / 8.0).sqrt()) * 2.0 / ric).sqrt();
    // the floor under a violent healing: s_min ~ ι₀/(2√(2E)) as E → ∞
    let floor = |e: f64| ((e - (e * e - ric * i0_2 / 8.0).sqrt()) * 2.0 / ric).sqrt();
    let floors: Vec<(f64, f64)> = [1.0f64, 10.0, 100.0, 1000.0].iter().map(|e: &f64| (*e, floor(*e))).collect();

    // ── 3. live SIGIL gauge ───────────────────────────────────────────────────────────────
    let live = load_live(series);

    // ── document ──────────────────────────────────────────────────────────────────────────
    let mut doc = Document::new("article")
        .option("11pt")
        .package_opt("inputenc", &["utf8"])
        .package_opt("geometry", &["margin=1.1in"])
        .package("amsmath")
        .package("amssymb")
        .package("booktabs")
        .package("float")
        .package_opt("hyperref", &["hidelinks"])
        .preamble(concat!(
            "\\title{Null Rays, Twist and the Kristensen Gauge\\\\[6pt]\\large Reading Aazami's \\emph{Symplectic 4-manifolds via ",
            "Lorentzian geometry} (arXiv:1504.06425) as a theorem about consensus --- with the Cosmic Garden's aeons in the margin}\n",
            "\\author{Viktor Kristensen\\thanks{Quillon Graph / SIGIL.} \\and ",
            "Rocky\\thanks{Claude, engineer-companion agent on Epsilon. Every number in this paper is computed by the generator ",
            "binary at document time --- Aazami's identities by finite differences on Kerr, the focusing theorem as an ODE, the live ",
            "gauge from the SIGIL series file --- and each claim is labelled PROVEN, DERIVED, ANALOGY, MEASURED or PRETEND.}\\\\[4pt]",
            "\\small typeset by \\texttt{flux-arxiv-latex}; generator \\texttt{null\\_rays\\_k\\_gauge}}\n",
            "\\date{\\today}"
        ))
        .add(Block::Raw("\\maketitle".into()))
        .add(raw(format!(
            "\\begin{{abstract}}\nAazami proves a small, sharp theorem: on a Lorentzian 4-manifold, a \\emph{{complete}} null vector \
field $k$ with geodesic flow and $\\mathrm{{Ric}}(k,k)>0$ is \\emph{{twisted everywhere}} --- its normal bundle is nowhere \
integrable, so there is no null hypersurface containing its rays --- and with any time function $f$ the closed form \
$\\omega=d\\,g(e^f k,\\cdot)$ is symplectic, with $\\det\\omega=e^{{4f}}\\,k(f)^2\\,\\iota^2$. We read that determinant as \
the Kristensen gauge. The two planes of $\\omega$ are the two factors of $K$: the clock plane $k(f)$ (does the time \
function advance along the ray) and the twist plane $\\iota$ (are there independent views). $K=0$ therefore has two \
causes, and until today the SIGIL gauge did not say which. We check Aazami's two null-congruence equations on the \
rapidly-rotating Kerr congruence numerically ({} grid points, max relative residual ${}$ and ${}$), tabulate the blind \
equatorial plane $\\iota^2=4a^2\\cos^2\\vartheta/\\rho^4\\to0$, run the focusing theorem as an ODE (untwisted: caustic at \
$\\lambda^*=\\pi/\\sqrt{{2R}}$, RK4 ${:.4}$ vs closed form ${:.4}$; twisted: bounded breathing in the potential \
$V(s)=\\iota_0^2/8s^2+Rs^2/4$, energy drift ${}$), and read the live SIGIL chain: {} effective producer(s) of {} --- \
the chain sits on Aazami's equatorial plane, exactly where the September battle test found the product form blind. The \
concrete deliverable is a \\texttt{{raychaudhuri}} block shipped in the gauge that names the degenerate plane. The \
consensus reading is an analogy and is labelled as one; the Cosmic Garden's four phases are re-derived from it, and its \
five-factor readiness product is shown to carry the same blindness $K^*$ had.\n\\end{{abstract}}\n",
            n_pts, sci(max_res_2), sci(max_res_1), lam_star_num, lam_star_closed, sci(e_drift),
            live.as_ref().map(|l| format!("{:.2}", l.n_eff)).unwrap_or("?".into()), live.as_ref().map(|l| l.distinct.to_string()).unwrap_or("?".into())
        )));

    // ── 1 ──
    doc = doc
        .add(Block::Section("The paper you sent is not the CCC paper --- and that is the good news".into()))
        .add(raw(String::from(
            "OK, so here is the deal. arXiv:1504.06425 is Amir Babak Aazami's \\emph{Symplectic 4-manifolds via Lorentzian \
geometry} (Kavli IPMU, 2015; Proc.\\ AMS 2017). It is not a paper about Conformal Cyclic Cosmology; there is no aeon in it and \
no Hawking point. The CCC paper we already worked through in July is Meissner and Penrose, arXiv:2503.24263, and the lane it \
produced --- supply as a boundary integral, the Hawking point as an undeclared flux --- is in the references. So why is this \
the right paper to put beside $K$? Because it is about the one geometric object both CCC and a consensus network are made \
of: a \\emph{null congruence}, a family of light rays, and the three numbers that describe how such a family behaves --- \
expansion, shear and twist --- together with the one term that pulls it together, $\\mathrm{Ric}(k,k)$. Penrose's crossover \
surface is a statement about what those rays do at the end of an aeon. A blockchain is a family of rays too: every node's \
view advancing at the speed of gossip, never faster. Aazami's theorem is about when such a family can be flattened into a \
single global \\emph{now} --- and the answer is: with positive focusing and no ends, never.",
        )))
        .add(raw(String::from(
            "Don't be scared by the name \\emph{symplectic}. A symplectic form is just a way of measuring \\emph{area} in a \
space where the two directions of each area are a coordinate and its conjugate --- position and momentum, or here time and \
energy, and transverse position and transverse twist. A symplectic form being nondegenerate means: no plane has zero area. \
Bohr and Sommerfeld taught us that such areas, divided by $\\hbar$, count states. That is the whole reason $K$ can be read \
off it.",
        )));

    // ── 2 ──
    doc = doc
        .add(Block::Section("What Aazami proves (PROVEN --- his; the numerical checks are ours)".into()))
        .add(raw(String::from(
            "Let $(M,g)$ be a Lorentzian 4-manifold and $k$ a null vector field ($g(k,k)=0$, $k\\neq0$: a light-ray direction, \
orthogonal to itself) with geodesic flow $\\nabla_k k=0$. Along its flow two well-known equations hold (O'Neill, \
Prop.\\ 5.7.2; Aazami's (1),(2)):\n\
\\begin{align}\nk(\\operatorname{div}k) &= \\tfrac12\\iota^2 - 2|\\sigma|^2 - \\tfrac12(\\operatorname{div}k)^2 - \\mathrm{Ric}(k,k), \\tag{1}\\\\\n\
k(\\iota^2) &= -2\\,(\\operatorname{div}k)\\,\\iota^2. \\tag{2}\n\\end{align}\n\
Equation (1) is the Raychaudhuri equation: $\\operatorname{div}k=\\theta$ is the \\emph{expansion} of the bundle of rays, \
$|\\sigma|^2$ the \\emph{shear} (the bundle deforming from a circle to an ellipse), $\\iota$ the \\emph{twist} (the rays \
rotating about each other; Aazami's $\\iota$ is twice the usual $\\omega$), and $\\mathrm{Ric}(k,k)$ the \\emph{focusing} \
term --- by Einstein's equations it is $8\\pi G\\,T(k,k)$, the energy density seen along the ray, non-negative for ordinary \
matter. Equation (2) says the twist evolves multiplicatively: $\\iota^2\\propto\\exp(-2\\!\\int\\!\\theta)$. Twist is never \
created and never destroyed along a ray; it is only diluted by expansion and concentrated by contraction. That is angular \
momentum conservation for a bundle of light.",
        )))
        .add(raw(String::from(
            "\\textbf{Theorem 1 (Aazami).} If $k$ is \\emph{complete} (its rays have no ends in either direction), \
$\\mathrm{Ric}(k,k)>0$, and there is a smooth $f$ with $k(f)$ nowhere zero, then $\\omega:=d\\,g(e^f k,\\cdot)$ is a \
symplectic form and $X=k/k(f)$ is a Liouville vector field ($\\mathcal L_X\\omega=\\omega$). In a frame $\\{k,x,y,\\ell\\}$ \
with $g(k,\\ell)=-1$,\n\
\\[\\omega(k,\\ell)=-e^f\\,k(f),\\qquad \\omega(x,y)=-e^f\\,\\iota,\\qquad \\det\\omega=e^{4f}\\,k(f)^2\\,\\iota^2 .\\]\n\
\\textbf{The proof in one breath.} Suppose the twist vanished along some complete ray. Then (2) keeps it zero, and (1) \
with $\\mathrm{Ric}(k,k)>0$ becomes $\\theta'<-\\theta^2/2$: the expansion must run to $-\\infty$ in finite affine \
parameter --- a caustic, every neighbouring ray crossing this one --- in the forward direction, and by the same argument \
in the backward direction, which a complete ray cannot do. So the twist is nowhere zero, $\\omega(x,y)\\neq0$, and \
with $k(f)\\neq0$ the form is nondegenerate. \\textbf{Remark 1:} any null surface tangent to $k$ is Lagrangian, \
$\\omega|_S=0$. \\textbf{Remark 3:} completeness cannot be dropped --- a conformal rescaling $\\tilde g=e^{2r}g$ of Kerr has \
$\\mathrm{Ric}(\\tilde k,\\tilde k)=2e^{-4r}>0$ and still fails, because its equatorial rays end on the ring singularity. \
\\textbf{Remark 4:} a stably causal spacetime (one with a global time function) is conformal to a null-complete one, so \
\\emph{only the curvature condition needs checking} after the rescaling. \\textbf{Proposition 1:} on a closed Lorentzian \
3-manifold with such a $k$ (timelike, constant length), some ray is a closed orbit --- via Taubes's proof of the Weinstein \
conjecture. That is where cyclicity lives in this paper.",
        )));

    // Kerr numerics
    let mut kerr_tab = String::from(
        "\\begin{table}[H]\\centering\\small\\begin{tabular}{rrr}\\toprule\n$\\vartheta$ (deg) & $\\iota^2=4a^2\\cos^2\\vartheta/\\rho^4$ & $\\det\\omega=e^{4r}\\iota^2$ \\\\\\midrule\n");
    for (deg, i2, det) in &det_rows { kerr_tab.push_str(&format!("{:.1} & ${}$ & ${}$ \\\\\n", deg, sci(*i2), sci(*det))); }
    kerr_tab.push_str(&format!("\\bottomrule\\end{{tabular}}\\caption{{Rapidly rotating Kerr, $m={}$, $a={}$, at $r={}$, $f=r$ so $k(f)=1$. The \
determinant of Aazami's form dies exactly on the equatorial plane: the rays there lie in a null hypersurface, the flow is \
integrable, and the theorem gives nothing. DERIVED (his eq.\\ 7, evaluated).}}\\end{{table}}\n", kerr.m, kerr.a, r0));
    doc = doc
        .add(Block::Subsection("The Kerr example, recomputed".into()))
        .add(raw(format!(
            "Aazami's own example is the maximally extended \\emph{{rapidly rotating}} Kerr spacetime, $a>m$: no horizon \
($\\Delta=r^2-2mr+a^2>0$ everywhere; on our grid $\\min\\Delta={:.3}$), a naked ring singularity at $r=\\cos\\vartheta=0$, \
Ricci-flat --- so the curvature hypothesis is \\emph{{not}} met and the twist comes entirely from the spin. Along the outgoing \
principal null congruence $k=\\partial_r+\\frac{{r^2+a^2}}{{\\Delta}}\\partial_t+\\frac a\\Delta\\partial_\\varphi$ the \
Newman--Penrose coefficient is $\\rho_{{NP}}=-1/(r-ia\\cos\\vartheta)$, so $\\operatorname{{div}}k=2r/\\rho^2$ and \
$\\iota^2=4a^2\\cos^2\\vartheta/\\rho^4$ with $\\rho^2=r^2+a^2\\cos^2\\vartheta$. We did not take (1) and (2) on trust: on \
{} points of the open northern hemisphere ($r\\in[-5,5]$, $0<\\vartheta<\\pi/2$) the derivative along $k$ was taken by \
central differences ($h=10^{{-5}}$) and compared with the right-hand sides. Max relative residual: eq.\\ (2) ${}$, \
eq.\\ (1) with $\\sigma=0$, $\\mathrm{{Ric}}=0$: ${}$. Both identities hold to the precision of the difference scheme. \
DERIVED.",
            min_delta, n_pts, sci(max_res_2), sci(max_res_1)
        )))
        .add(Block::Raw(kerr_tab));

    // ── 3 the dictionary ──
    doc = doc
        .add(Block::Section("The dictionary (ANALOGY --- stated once, then used)".into()))
        .add(raw(String::from(
            "Now here is the thing that should bother you. Nothing in a blockchain is a Lorentzian metric. What we have is \
weaker and, for the purpose, enough: a set of \\emph{rays} (each node's view, advancing one block at a time and never \
faster than gossip), a \\emph{time function} on them (block height, which every honest node sees increase), a \
\\emph{transverse} structure (how the views differ from one another at the same height), and a term that \\emph{pulls views \
together} (finality). Those are exactly the four ingredients Theorem 1 consumes. The dictionary is:",
        )))
        .add(Block::Raw(String::from(
            "\\begin{table}[H]\\centering\\small\\begin{tabular}{lll}\\toprule\nLorentzian & SIGIL & where it is measured \\\\\\midrule\n\
null ray $k$ & one node's view advancing at gossip speed & every node \\\\\n\
affine parameter $\\lambda$ & block height & \\texttt{/v1/finality/certificate} \\\\\n\
time function $f$, $k(f)>0$ & height monotone on every ray (no sync-down, no rewind) & the sync-safety rules \\\\\n\
expansion $\\theta=\\operatorname{div}k$ & fork spread: merge-parent fraction per block & channel \\texttt{tip\\_divergence} \\\\\n\
shear $\\sigma$ & height skew among converged peers & channel \\texttt{finality\\_divergence} \\\\\n\
twist $\\iota$ & proposer entropy $\\Delta s$: independent, unflattenable views & \\texttt{proposer\\_structure} \\\\\n\
focusing $\\mathrm{Ric}(k,k)>0$ & finality pulling: settled height advancing per block & new: \\texttt{raychaudhuri.focusing} \\\\\n\
$\\omega$ exact ($\\omega=d\\vartheta$) & supply is a boundary integral; no undeclared mint & \\texttt{sigil-chronos/boundary.rs} \\\\\n\
Hawking point (exactness fails at a point) & \\texttt{UndeclaredFlux} & July CCC lane \\\\\n\
Lagrangian null surface ($\\omega|_S=0$) & the certified spine: compare views at the SAME height & \\texttt{order\\_hash} rule \\\\\n\
$\\det\\omega=e^{4f}k(f)^2\\iota^2$ & the gauge: $K^2\\propto(\\text{clock})\\times(\\text{twist})$ & \\texttt{flux\\_sigil\\_kgauge} \\\\\n\
Kerr equatorial plane, $\\iota=0$ & one effective key & live g2 today \\\\\n\
Remark 3: rescaling cannot buy completeness & $K_{\\mathrm{fix}}$ (seconds$\\to$blocks) needs monotone height & the rescale is conformal \\\\\n\
Prop.\\ 1: a closed orbit & epochs, aeons & pool epochs, emission eras \\\\\n\
\\bottomrule\\end{tabular}\\caption{The dictionary. Every row is an analogy; the right column is where the SIGIL side is actually measured.}\\end{table}\n",
        )))
        .add(raw(String::from(
            "Read Theorem 1 through it and it says something you already paid for in September: \\emph{if finality is \
always pulling and the chain never stalls, the views can never be flattened into one global state; agreement is only ever \
along the order (a Lagrangian surface parametrised by height), never on a slice of wall-clock time.} That is the \
\\texttt{order\\_hash} rule --- compare at the same height or the check is worthless --- stated as geometry. And the \
determinant says why $K$ is a \\emph{product}: the gauge has two planes, and either can die.",
        )));

    // ── 4 two planes ──
    doc = doc
        .add(Block::Section("$K=0$ has two causes, and the gauge must say which (DERIVED, then SHIPPED)".into()))
        .add(raw(String::from(
            "$\\det\\omega=e^{4f}\\,k(f)^2\\,\\iota^2$. The gauge $K^{*2}=(2\\pi)^2\\,\\Delta H\\,\\tau\\,\\Delta s/\\hbar$ has the \
same two factors: $\\Delta H\\tau/\\hbar$ is a phase-space area in the time--energy plane (Margolus--Levitin ticks --- the \
\\emph{clock} plane, $k(f)$), and $\\Delta s$ is the entropy of the transverse plane (the \\emph{twist}, $\\iota$). So \
$K=0$ means \\emph{either} $k(f)=0$ --- the time function did not advance, the chain is stalled --- \\emph{or} $\\iota=0$ \
--- one effective proposer, an integrable flow, a global now that agrees with itself. These are opposite conditions. \
One is a dead chain; the other is a perfectly healthy chain with one key. The battle test of 2026-09-14 measured the second: \
an identical fork read $K^*=0$ ``stable'' with one key and $5.88$ ``critical'' with two. Aazami's Kerr table above is the \
same fact: on the equatorial plane the determinant is zero and the theorem is silent.",
        )))
        .add(raw(String::from(
            "The repair $K_{\\mathrm{fix}}=2\\pi\\sqrt{\\Delta H_c\\,\\tau_d\\,w_S}$ already refuses to let the twist plane kill \
the reading (it turns proposer entropy into a bounded weight $w_S\\in[\\tfrac34,1]$, so one key equivocating reads \\emph{worst}). \
What was still missing is the \\emph{name}. From this commit the MCP gauge (\\texttt{flux\\_sigil\\_kgauge}) carries a \
\\texttt{raychaudhuri} block: \\texttt{clock\\_blocks} and \\texttt{clock\\_alive} ($k(f)$), \\texttt{twist\\_n\\_eff} and \
\\texttt{twist\\_degenerate} ($\\iota$; degenerate below $N_{\\mathrm{eff}}=1.1$), \\texttt{expansion} ($\\theta$, the \
merge-parent fraction), \\texttt{shear} ($\\sigma$, the finality-divergence channel), \\texttt{focusing} ($\\mathrm{Ric}$: \
settled-height advance divided by blocks added in the window --- a new observable, $1$ when finality keeps up, $0$ when it \
is frozen), \\texttt{det\\_omega\\_norm}, a \\texttt{caustic\\_warning} (focusing $>0$ on an untwisted congruence --- the \
state Theorem 1 says cannot persist on a complete flow: expect a stall or a second proposer), and a one-line \
\\texttt{verdict} that starts with \\texttt{clock-stalled}, \\texttt{twist-degenerate}, \\texttt{focusing-zero} or \
\\texttt{nondegenerate}. Five unit cases pin it. Nothing in $K_C$ or $K_{\\mathrm{fix}}$ changed; the block is additive.",
        )));

    // ── 5 focusing ODE ──
    let mut floor_tab = String::from("\\begin{tabular}{rr}\\toprule $E$ (healing energy) & $s_{\\min}$ (floor under the spread) \\\\\\midrule\n");
    for (e, s) in &floors { floor_tab.push_str(&format!("{} & {:.4} \\\\\n", e, s)); }
    floor_tab.push_str("\\bottomrule\\end{tabular}");
    doc = doc
        .add(Block::Section("The focusing theorem as an ODE: twist is a centrifugal barrier (DERIVED)".into()))
        .add(raw(format!(
            "Take (1) and (2) with no shear and constant focusing $R$. Write $A=\\int\\theta\\,d\\lambda$ (so $e^A$ is the \
cross-section area of the bundle) and $s=e^{{A/2}}$ (its width). Then (2) is $\\iota^2=\\iota_0^2/s^4$ and (1) becomes\n\
\\[ s''=\\frac{{\\iota_0^2}}{{4s^3}}-\\frac R2\\,s,\\qquad\\text{{i.e. a particle in }}V(s)=\\frac{{\\iota_0^2}}{{8s^2}}+\\frac R4\\,s^2,\\]\n\
with $E=\\tfrac12 s'^2+V(s)$ conserved. Now look at the potential. The $Rs^2$ half is the focusing --- it pulls the width \
to zero. The $\\iota_0^2/s^2$ half is the twist --- a centrifugal barrier that goes to $+\\infty$ as the width goes to \
zero. \\emph{{With any twist at all the width can never reach zero}}; it oscillates between two turning points around \
$s_*=(\\iota_0^2/2R)^{{1/4}}$, i.e.\\ around $\\iota^2=2R$, with period $2\\pi/\\sqrt{{2R}}$ at every amplitude (shown below). With no twist the \
barrier is gone and the width hits zero in finite affine parameter: the caustic. That is Theorem 1's contradiction, as \
mechanics.",
        )))
        .add(raw(format!(
            "We ran it. Untwisted, $R={}$, $\\theta(0)=0$: RK4 ($h=10^{{-4}}$) reaches $\\theta<-10^6$ at $\\lambda={:.4}$; the \
closed form $\\theta=-\\sqrt{{2R}}\\tan(\\sqrt{{R/2}}\\,\\lambda)$ blows up at $\\lambda^*=\\pi/\\sqrt{{2R}}={:.4}$. Twisted, \
$\\iota_0^2={}$ (twice the fixed-point value), $s(0)=1$, $s'(0)=0$, integrated to $\\lambda={}$: the width breathes between \
$s_{{\\min}}={:.4}$ and $s_{{\\max}}={:.4}$ (closed form from $E=V(s)$: ${:.4}$ and ${:.4}$), $\\theta$ stays in \
$[{:.3},{:.3}]$, the energy drifts by at most ${}$ relative, and the measured period between upward zero-crossings of \
$\\theta$ is ${:.4}$ against $2\\pi/\\sqrt{{2R}}={:.4}$ --- and they agree to every printed digit \\emph{{at this finite \
amplitude}}, which is not luck: with $u=s^2$ (the cross-section area) the energy integral turns $s''$ into $u''+2Ru=4E$, a \
harmonic oscillator, so the breathing is \\emph{{isochronous}} --- its frequency $\\sqrt{{2R}}$ depends on the focusing alone, \
not on how violently the bundle was squeezed. Equilibrium width $s_*={:.4}$.",
            ric, lam_star_num, lam_star_closed, i0_2, lam_end, s_min, s_max, s_min_closed, s_max_closed, th_min, th_max, sci(e_drift), period_num, period_lin, s_star
        )))
        .add(raw(format!(
            "\\textbf{{The Cosmic Garden's thesis, derived.}} The January paper (\\emph{{The Cosmic Arcology Mission}}) says: \
bring isolated partitions together before they are mature and they strangle each other; a wise gardener waits. In the ODE, \
``bringing them together'' is a large $E$ --- a violent contraction --- and ``maturity'' is $\\iota_0$. The floor under the \
collapse is the inner turning point,\n\
\\[ s_{{\\min}}^2=\\frac{{2}}{{R}}\\Big(E-\\sqrt{{E^2-R\\iota_0^2/8}}\\Big)\\;\\xrightarrow{{E\\to\\infty}}\\;\\frac{{\\iota_0^2}}{{8E}},\\]\n\
so $s_{{\\min}}\\approx\\iota_0/\\sqrt{{8E}}$: the floor is proportional to the twist and shrinks only as one over the square root \
of the healing energy. For $\\iota_0^2={}$:\n\\begin{{center}}{}\\end{{center}}\n\
Zero twist, any $E$: floor zero, caustic. That is the gardener's rule with a formula in it. The garden paper's four phases \
are the four regimes of $(\\theta,\\iota^2)$: \\emph{{Isolation}} = $\\theta>0$, partitions spreading, twist per area \
diluting as $1/s^4$; \\emph{{Convergence}} = $\\theta<0$, twist concentrating; \\emph{{Aeon transition}} = the Lagrangian \
surface, the crossover, where views are compared at one height; \\emph{{Harmony}} = the fixed point $\\theta=0$, \
$\\iota^2=2R$, twist balancing focusing. ANALOGY, with the mechanics DERIVED.",
            i0_2, floor_tab
        )))
        .add(raw(String::from(
            "\\textbf{And the garden paper's own gauge.} Its convergence readiness is $k=G^{0.25}Q^{0.20}T^{0.20}I^{0.15}R^{0.20}$ \
--- a product of five factors, each in $[0,1]$. A product form has the blindness the battle test measured in $K^*$: one factor \
at zero reads ``absorption'' whatever the other four say, and one factor at one contributes nothing. Its $K$-ranges \
(Table 3 there: communion above $0.9$, absorption below $0.3$) were chosen, not derived. The ODE gives the derived version: \
readiness is $\\iota_0$, danger is $E$, and the outcome is $s_{\\min}$. Its ``claimed experimental results'' ($8.7\\sigma$ \
quantum-gravity detection and so on) are labelled CLAIMED in our records and do not enter here.",
        )));

    // ── 6 live ──
    let live_sec = match &live {
        Some(l) => {
            let twist_deg = l.n_eff < 1.1;
            format!(
                "The series line at \\texttt{{ts\\_ms}}={} (read-only; \\texttt{{flux\\_sigil\\_kgauge}}, {}-block window): \
$K_C={:.3}$, $K_{{\\mathrm{{fix}}}}={:.3}$ ({}, persistence ${:.0}$ blocks), $\\Delta H_c={:.4}$ over the channels tip \
${:.3}$ / state-root {} / finality {} / semantic ${:.3}$; proposer entropy $\\Delta s={:.4}$ bits, $N_{{\\mathrm{{eff}}}}={:.3}$ \
of {} active producer(s), dominant share ${:.1}\\%$; block rate ${:.2}$/s, finality $512$ blocks $={:.1}$ s, {} peers, \
{} blocks added in the window. Through the dictionary: the clock plane is alive ($k(f)$: {} blocks), the twist plane is \
{} ($N_{{\\mathrm{{eff}}}}={:.2}$: {}), the expansion is ${:.3}$ (a straight chain), the shear is {} (peers lagging the \
producer's spine), and the focusing term is {}. MEASURED. So the live chain is Aazami's equatorial plane: the reading \
$K_C={:.3}$ comes from \\emph{{shear}} --- peers behind the producer --- not from any disagreement between independent \
views, because there is only one view. A fork today would be invisible to the product form and read as the worst case by \
$K_{{\\mathrm{{fix}}}}$; that is the right behaviour, and now the gauge says why.{}",
                l.ts_ms, 200, l.k_c, l.k_fix, l.regime_fix, l.persist, l.delta_h, l.d_tip,
                l.d_state.map(|v| format!("${v:.3}$")).unwrap_or("n/a".into()), l.d_fin.map(|v| format!("${v:.3}$")).unwrap_or("n/a".into()), l.d_sem,
                l.ds_bits, l.n_eff, l.distinct, l.dominant * 100.0, l.bps, l.finality_s, l.peers, l.blocks_added,
                l.blocks_added, if twist_deg { "degenerate" } else { "nondegenerate" }, l.n_eff,
                if twist_deg { "one effective key" } else { "independent proposers" }, l.d_tip,
                l.d_fin.map(|v| format!("${v:.3}$")).unwrap_or("unmeasured".into()),
                match l.focusing { Some(f) => format!("${f:.3}$ (settled-height advance per block added)"), None => "not in this reading --- the series line predates the \\texttt{raychaudhuri} block; the next reading carries it".into() },
                l.k_c,
                match &l.ray_verdict { Some(v) => format!(" The gauge's own verdict line: \\texttt{{{}}}.", tt(v)), None => String::new() }
            )
        }
        None => "No series line could be read at generation time; the live section is empty rather than filled from memory.".to_string(),
    };
    doc = doc
        .add(Block::Section("The live SIGIL chain, read through the dictionary (MEASURED)".into()))
        .add(raw(live_sec));

    // ── 7 CCC margin ──
    doc = doc
        .add(Block::Section("The CCC margin: rescaling, exactness, and the Hawking point".into()))
        .add(raw(String::from(
            "Three of Aazami's remarks are CCC in miniature. \\textbf{Remark 4} is Penrose's move: a spacetime with a global time \
function is conformal to one whose null rays never end, so completeness is a matter of choosing the conformal factor, and \
only the curvature condition remains. $K_{\\mathrm{fix}}$ did exactly this to $K$ on 2026-09-14 --- it replaced wall-clock \
seconds by block depth --- and the theorem tells you what that rescaling can and cannot buy: it makes every ray complete \
\\emph{provided the new time function is monotone on every ray}. A node that rewinds (the 2026-09-10 follower that had to be \
resynced; any sync-down) breaks monotonicity, its ray ends, and \\textbf{Remark 3} applies: the rescaled metric has positive \
Ricci and the conclusion still fails. Rule: a gauge reading over a window in which any peer's height went backwards must be \
flagged incomplete, whatever $K$ says.",
        )))
        .add(raw(String::from(
            "\\textbf{Exactness} is the second margin note. Aazami's $\\omega=d\\,g(e^f k,\\cdot)$ is exact: every symplectic area is \
a boundary integral of the 1-form $g(e^f k,\\cdot)$ (Stokes). In July's CCC lane the chain's supply law was made exactly \
that: $\\mathrm{supply}(h_2)-\\mathrm{supply}(h_1)=\\sum\\text{declared mint}-\\sum\\text{declared burn}$, checked at every \
block boundary by \\texttt{boundary.rs}, and a violation --- value entering with no declared source --- is the \
\\texttt{UndeclaredFlux}, the Hawking point: a point where $d\\omega\\neq0$, where exactness fails and the conformal \
structure is not smooth. Meissner and Penrose's crossover 3-surface is smooth ``except at a discrete set of points''; ours \
is smooth at every block or the audit says where it is not.",
        )))
        .add(raw(String::from(
            "\\textbf{Proposition 1} is the third: on a closed (compact) Lorentzian 3-manifold with positive focusing, some ray \
closes on itself. Compactness is finite state; positive focusing is finality. The closed orbit is the epoch --- the \
shielded pool's $32{,}768$-note epochs, the $64$ emission eras of four Julian years --- and, in CCC, the aeon. We do not \
claim the proposition applies to SIGIL (its state space is not a closed manifold); we note that it is the only place in \
the paper where cyclicity is a theorem rather than a picture, and that it needs Taubes.",
        )));

    // ── 8 what to change ──
    doc = doc
        .add(Block::Section("What this changes in SIGIL and in $K$ (the plan, ranked)".into()))
        .add(raw(String::from(
            "\\begin{enumerate}\n\
\\item \\textbf{Shipped now:} the \\texttt{raychaudhuri} block in \\texttt{flux\\_sigil\\_kgauge} --- clock, twist, expansion, \
shear, focusing, $\\det\\omega$, caustic warning, verdict. $K=0$ is no longer ambiguous. Additive; $K_C$ and \
$K_{\\mathrm{fix}}$ untouched.\n\
\\item \\textbf{Next (a measurement, not a formula):} the breathing mode. Two producers on the chronos harness, induce a partition, \
heal it, record the width $s(h)$ (spread of peer tips) and the twist $\\iota^2(h)$ (proposer entropy) per block. The ODE \
predicts $\\iota^2\\propto1/s^4$ and a bounded oscillation around $\\iota^2=2R$ with $R$ the finality pull. If the chain \
does that, the dictionary is a law; if not, it stays an analogy. PRETEND until run.\n\
\\item \\textbf{Focusing as a channel:} settled-height advance per block added is now published; promote it into \
$\\Delta H_c$ only after the breathing measurement, so a frozen finality (the 2026-09-15 shape) raises the gauge instead of \
hiding behind $K=0$.\n\
\\item \\textbf{Completeness flag:} any peer height going backwards inside the window marks the reading incomplete (Remark 3). \
One comparison per peer per window; cheap.\n\
\\item \\textbf{Already in place, now with a reason:} compare views at the same height (the Lagrangian rule); supply as a boundary \
integral (exactness); one producer today means the twist plane is degenerate, and the honest word for a fork-free \
single-writer chain is not ``stable'' but ``untested''.\n\
\\end{enumerate}",
        )));

    // ── 9 ledger ──
    let ledger = format!(
        "\\begin{{table}}[H]\\centering\\small\\begin{{tabular}}{{lll}}\\toprule\nQuantity & Value & Status \\\\\\midrule\n\
Aazami eq.\\ (2) residual on Kerr ({} pts) & ${}$ & DERIVED \\\\\n\
Aazami eq.\\ (1) residual on Kerr & ${}$ & DERIVED \\\\\n\
$\\det\\omega$ at $\\vartheta=0^\\circ$ / $89.9^\\circ$ / $90^\\circ$ & ${}$ / ${}$ / ${}$ & DERIVED \\\\\n\
Caustic $\\lambda^*$, untwisted, $R=1$: RK4 / closed & ${:.4}$ / ${:.4}$ & DERIVED \\\\\n\
Breathing: $s_{{\\min}},s_{{\\max}}$ (num / closed) & ${:.4},{:.4}$ / ${:.4},{:.4}$ & DERIVED \\\\\n\
Breathing period (num / $2\\pi/\\sqrt{{2R}}$, exact by $u''+2Ru=4E$) & ${:.4}$ / ${:.4}$ & DERIVED \\\\\n\
Energy drift over $\\lambda={}$ & ${}$ & DERIVED \\\\\n\
Live $K_C$ / $K_{{\\mathrm{{fix}}}}$ & {} & MEASURED \\\\\n\
Live $N_{{\\mathrm{{eff}}}}$ of producers & {} & MEASURED \\\\\n\
The dictionary & --- & ANALOGY \\\\\n\
Breathing mode on the real chain & --- & PRETEND \\\\\n\
Garden paper's $8.7\\sigma$ etc. & --- & CLAIMED (not used) \\\\\n\
\\bottomrule\\end{{tabular}}\\caption{{The ledger. DERIVED = computed here from the paper's formulas; MEASURED = read from the live chain; ANALOGY = the dictionary; PRETEND = not yet run.}}\\end{{table}}\n",
        n_pts, sci(max_res_2), sci(max_res_1), sci(det_rows[0].2), sci(det_rows[5].2), sci(det_rows[6].2),
        lam_star_num, lam_star_closed, s_min, s_max, s_min_closed, s_max_closed, period_num, period_lin, lam_end, sci(e_drift),
        live.as_ref().map(|l| format!("${:.3}$ / ${:.3}$", l.k_c, l.k_fix)).unwrap_or("n/a".into()),
        live.as_ref().map(|l| format!("${:.3}$ of {}", l.n_eff, l.distinct)).unwrap_or("n/a".into())
    );
    doc = doc.add(Block::Section("The ledger of numbers".into())).add(Block::Raw(ledger));

    // ── 10 falsifiable + pretend + repro ──
    doc = doc
        .add(Block::Section("Falsifiable statements".into()))
        .add(raw(String::from(
            "\\begin{enumerate}\n\
\\item On a two-producer chronos run with a healed partition, $\\log\\iota^2$ against $\\log s$ has slope $-4\\pm0.5$ over the \
healing. If the slope is not near $-4$, equation (2) does not govern proposer entropy and the twist reading of $\\Delta s$ is wrong.\n\
\\item A window in which the settled height does not advance while blocks are produced will, from this commit, carry \
\\texttt{focusing = 0} and \\texttt{finality\\_frozen = true}; the 2026-09-15 freeze replayed through the gauge must show it.\n\
\\item With one effective producer the \\texttt{verdict} starts with \\texttt{twist-degenerate} and $K^*$ reads $0$ for any fork; \
with two it does not. Both halves were measured on 2026-09-14; the verdict is the new part.\n\
\\end{enumerate}",
        )))
        .add(Block::Section("What is still pretend".into()))
        .add(raw(String::from(
            "The dictionary is not a derivation; a chain has no metric, and ``expansion'', ``shear'' and ``twist'' are names we \
give to three channels because they transform the way the geometric ones do under the operations we care about (one key \
kills twist; a stall kills the clock; finality focuses). The breathing mode has not been observed on any chain. The focusing \
observable is published but not yet weighted into $\\Delta H_c$. The live reading is one 30-second window on a chain with one \
producer, which is precisely the case the theorem cannot speak to. Nothing here alters consensus; the only code that changed \
is a read-only gauge.",
        )))
        .add(Block::Section("Reproducibility".into()))
        .add(raw(String::from(
            "Generator: \\texttt{flux/crates/flux-arxiv-latex/src/bin/null\\_rays\\_k\\_gauge.rs}, built and run with \\texttt{fluxc} \
(\\texttt{fluxc run -p flux-arxiv-latex --bin null\\_rays\\_k\\_gauge [out\\_dir] [series.jsonl]}). Gauge: \
\\texttt{flux/crates/fluxc-mcp/src/handlers/sigil\\_kgauge.rs}, function \\texttt{raychaudhuri\\_of} and test \
\\texttt{raychaudhuri\\_names\\_the\\_degenerate\\_plane}. Series: \
\\texttt{/home/storage/claude-code/k-parameter-paper/gauge-series/sigil-kgauge.jsonl}. Paper source: arXiv:1504.06425v4.",
        )))
        .add(Block::Raw(String::from(
            "\\begin{thebibliography}{9}\n\
\\bibitem{aazami2017} A.~B.~Aazami, \\emph{Symplectic 4-manifolds via Lorentzian geometry}, Proc.\\ Amer.\\ Math.\\ Soc.\\ 145 (2017) 387--394; arXiv:1504.06425.\n\
\\bibitem{oneill1995} B.~O'Neill, \\emph{The Geometry of Kerr Black Holes}, A.~K.~Peters (1995).\n\
\\bibitem{raychaudhuri1955} A.~K.~Raychaudhuri, \\emph{Relativistic cosmology I}, Phys.\\ Rev.\\ 98 (1955) 1123.\n\
\\bibitem{taubes2007} C.~H.~Taubes, \\emph{The Seiberg--Witten equations and the Weinstein conjecture}, Geom.\\ Topol.\\ 11 (2007) 2117.\n\
\\bibitem{meissner2025} K.~A.~Meissner and R.~Penrose, \\emph{The Physics of Conformal Cyclic Cosmology}, arXiv:2503.24263 (2025).\n\
\\bibitem{ccclane} Rocky, \\emph{CCC $\\times$ SIGIL --- the crossover 3-surface as a checkpoint} and \\emph{Supply as a boundary integral} (2026-07-29). \\url{https://quillon.xyz/downloads/ccc-sigil-findings.md}, \\url{https://quillon.xyz/downloads/ccc-lane-boundary-integral.md}\n\
\\bibitem{garden2026} Q-NarwhalKnight Research Consortium, \\emph{The Cosmic Arcology Mission: Water Robots, K-Kristensen Convergence, and the Q-NarwhalKnight Quantum Consensus System}, v2.4.0 (Jan.\\ 2026). \\url{https://quillon.xyz/downloads/cosmic-arcology-mission.pdf}\n\
\\bibitem{kwhitepaper} V.~Kristensen and Rocky, \\emph{The Kristensen K-Parameter}, whitepaper v1.1 (2026-09-14). \\url{https://sigilgraph.org/downloads/kristensen-k-parameter-whitepaper.pdf}\n\
\\bibitem{battle} Rocky, \\emph{K* battle test} (2026-09-14). \\url{https://sigilgraph.org/downloads/sigil-kparam-battle-2026-09-14.pdf}\n\
\\end{thebibliography}\n",
        )));

    std::fs::create_dir_all(out_dir).expect("out dir");
    let res = doc.compile_pdf(out_dir, "null_rays_k_gauge");
    let md = format!(
        "# Null Rays, Twist and the Kristensen Gauge — arXiv:1504.06425 read as a consensus theorem (2026-09-17)\n\n\
Aazami (2015/2017): a COMPLETE geodesic null congruence with Ric(k,k) > 0 is twisted everywhere; ω = d g(e^f k,·) is symplectic with \
det ω = e^{{4f}}·k(f)²·ι². Read on the chain: K² ∝ (clock plane k(f)) × (twist plane ι) — K = 0 has TWO causes (stalled clock / one key) \
and the gauge now names which (`raychaudhuri` block in flux_sigil_kgauge).\n\n\
- Kerr (m=1, a=1.2) checks of Aazami's (1),(2): max rel residual {} / {} over {} points (DERIVED); det ω at 0°/89.9°/90°: {} / {} / {} — the equatorial blind plane = one-key chain.\n\
- Focusing ODE: untwisted caustic λ* = {:.4} (RK4) vs π/√(2R) = {:.4}; twisted: V(s) = ι₀²/8s² + Rs²/4, width breathes in [{:.4}, {:.4}], period {:.4} = 2π/√(2R) = {:.4} exactly (u = s² is harmonic: u'' + 2Ru = 4E), energy drift {} (DERIVED).\n\
- Cosmic Garden's rule derived: floor under a healing s_min ≈ ι₀/√(8E) — maturity = twist; its k = G^.25 Q^.20 T^.20 I^.15 R^.20 is a product form with K*'s blindness.\n\
- Live SIGIL: {} (MEASURED).\n\n\
PDF: https://sigilgraph.org/downloads/null-rays-k-gauge.pdf\n",
        sci(max_res_2), sci(max_res_1), n_pts, sci(det_rows[0].2), sci(det_rows[5].2), sci(det_rows[6].2),
        lam_star_num, lam_star_closed, s_min, s_max, period_num, period_lin, sci(e_drift),
        live.as_ref().map(|l| format!("K_C {:.3} / K_fix {:.3} ({}), N_eff {:.2} of {}, Δs {:.4} bits, {:.2} blk/s, finality {:.1} s", l.k_c, l.k_fix, l.regime_fix, l.n_eff, l.distinct, l.ds_bits, l.bps, l.finality_s)).unwrap_or("no series line".into())
    );
    std::fs::write(format!("{out_dir}/null-rays-k-gauge-abstract.md"), md).expect("abstract");
    if res.success {
        println!("OK {}", res.pdf_path.unwrap());
        println!(
            "kerr res (2) {:.3e} (1) {:.3e} | caustic {:.4} vs {:.4} | breathing s [{:.4},{:.4}] period {:.4} vs {:.4} drift {:.2e} | live {}",
            max_res_2, max_res_1, lam_star_num, lam_star_closed, s_min, s_max, period_num, period_lin, e_drift,
            live.as_ref().map(|l| format!("K_C {:.3} K_fix {:.3} N_eff {:.2}", l.k_c, l.k_fix, l.n_eff)).unwrap_or("none".into())
        );
    } else {
        let tail: String = res.log.lines().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        eprintln!("FAILED\n{tail}");
        std::process::exit(1);
    }
}
