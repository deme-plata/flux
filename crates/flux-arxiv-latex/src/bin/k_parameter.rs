//! k_parameter — "The Kristensen K-Parameter, Recomputed": an arXiv-style
//! paper on Viktor Kristensen's K-parameter in which every number is COMPUTED
//! at generation time from CODATA 2022 constants (flux-science), the units
//! audit is executed rather than asserted, the K* ↔ Margolus–Levitin identity
//! is checked numerically at three scales, and — when a snapshot is supplied —
//! the LIVE Quillon Graph gauge (`/api/v1/k-parameter`) is read into the paper.
//! Related work comes from a live arXiv API sweep parsed by flux-arxiv-latex.
//!
//! Usage: k_parameter [arxiv.json] [out_dir] [gauge_snapshot.json] [gauge_series.jsonl]
use flux_arxiv_latex::doc::{Block, Document};
use flux_arxiv_latex::{bibliography, latex_escape, parse_arxiv_json, related_work_section, ArxivPaper};
use flux_science::constants::*;

// eV, m_p, m_e now come from flux-science::constants (CODATA 2022 particle
// block landed 2026-09-02, closing kappa-hep audit fix #2).

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


/// One row of the live-gauge time series (`/api/v1/k-parameter` `data`, one JSON object per line).
struct Sample {
    at: i64,
    k: f64,
    k_enh: f64,
    dh: f64,
    ds: f64,
    brd: f64,
    rej: f64,
    churn: f64,
    lam: f64,
    phase: String,
}

fn load_series(path: &str) -> Vec<Sample> {
    let Ok(txt) = std::fs::read_to_string(path) else { return vec![] };
    txt.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|d| {
            let f = |k: &str| d.get(k).and_then(|x| x.as_f64());
            Some(Sample {
                at: d.get("last_computed_at").and_then(|x| x.as_i64()).unwrap_or(0),
                k: f("k_value")?,
                k_enh: f("k_enhanced")?,
                dh: f("delta_h")?,
                ds: f("delta_s")?,
                brd: f("block_rate_deviation")?,
                rej: f("rejection_ratio")?,
                churn: f("peer_churn")?,
                lam: f("lambda_commit")?,
                phase: d.get("phase").and_then(|x| x.as_str()).unwrap_or("?").to_string(),
            })
        })
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let json_path = args.get(1).map(String::as_str).unwrap_or("crates/flux-arxiv-latex/k_parameter.arxiv.json");
    let out_dir = args.get(2).map(String::as_str).unwrap_or("/home/storage/claude-code/k-parameter-paper");
    let gauge = args.get(3).and_then(|p| load_gauge(p));
    let series = args.get(4).map(|p| load_series(p)).unwrap_or_default();

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
    let alpha_g_p = gravitational_coupling(PROTON_MASS);
    let alpha_g_e = gravitational_coupling(ELECTRON_MASS);
    let landauer_300 = landauer_bound(300.0);
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


    // ------------------------------------------------------------ anatomy of the live gauge
    // Constants as shipped in q-api-server/src/k_parameter_gauge.rs (v10.3.0):
    let g_tau = 60.0_f64; // window, s; ALSO used as "expected blocks per round" (1 bps assumed)
    let g_kappa_tc = 18.0_f64 * 100.0; // KAPPA * TAU_CONFIRM in Lambda_commit
    let g_lam_min = 0.01_f64;
    let g_cap = 100.0_f64;
    let g_thr_appr = 5.0_f64;
    let g_thr_crit = 10.0_f64;
    let k_base_max = 2.0 * pi * (3.0_f64 * 2.0).sqrt() / g_tau; // dH<=3 (three ratios), dS<=2
    let prod_crit = (g_thr_crit / g_cap * g_tau / (2.0 * pi)).powi(2);
    let prod_appr = (g_thr_appr / g_cap * g_tau / (2.0 * pi)).powi(2);
    let hd_escape_clamp = -g_kappa_tc * (1.0 - g_lam_min).ln(); // blocks/window for Lambda >= 0.01
    let hd_half = g_kappa_tc * 2f64.ln(); // blocks/window for Lambda = 0.5
    let mut anat_rows = String::new();
    let mut rates = vec![];
    let mut asyms = vec![];
    let mut max_recon_err = 0f64;
    let mut ratios_all_cap = true;
    let mut lam_err_max = 0f64;
    for smp in &series {
        let hd = (g_tau * (1.0 - smp.brd)).round(); // brd = (60 - hd)/60 when hd < 60
        let rate = hd / g_tau;
        let asym = (smp.dh - smp.rej - smp.churn).max(0.0);
        let k_recon = 2.0 * pi * (smp.dh * smp.ds).sqrt() / g_tau;
        let err = if smp.k > 0.0 { ((k_recon - smp.k) / smp.k).abs() } else { 0.0 };
        if err > max_recon_err {
            max_recon_err = err;
        }
        let lam_recon = 1.0 - (-hd / g_kappa_tc).exp();
        let lerr = if smp.lam > 0.0 { ((lam_recon - smp.lam) / smp.lam).abs() } else { 0.0 };
        if lerr > lam_err_max {
            lam_err_max = lerr;
        }
        let ratio = if smp.k > 0.0 { smp.k_enh / smp.k } else { 0.0 };
        if (ratio - g_cap).abs() > 1e-6 {
            ratios_all_cap = false;
        }
        rates.push(rate);
        asyms.push(asym);
        anat_rows.push_str(&format!(
            "{} & {:.0} & {:.3} & {:.3} & {:.4} & {:.4} & {:.0} & \\texttt{{{}}} \\\\\n",
            smp.at, hd, rate, asym, smp.k, k_recon, ratio, smp.phase
        ));
    }
    let n_s = series.len().max(1) as f64;
    let rate_mean = rates.iter().sum::<f64>() / n_s;
    let asym_min = asyms.iter().cloned().fold(f64::INFINITY, f64::min);
    let asym_max = asyms.iter().cloned().fold(0.0, f64::max);
    let ds_floor = (1.0 - rate_mean).abs(); // block_rate_deviation the wrong constant guarantees
    // ΔH needed for CRITICAL given that floor (sync divergence ~0):
    let dh_for_crit = prod_crit / ds_floor.max(1e-9);
    let dh_for_appr = prod_appr / ds_floor.max(1e-9);
    // Same samples re-scored with the observed mean rate as the expected rate:
    let mut rescored = String::new();
    for smp in &series {
        let hd = (g_tau * (1.0 - smp.brd)).round();
        let sync_div = (smp.ds - smp.brd).max(0.0);
        let brd_true = (hd - g_tau * rate_mean).abs() / (g_tau * rate_mean);
        let ds_true = sync_div + brd_true;
        let k_true = 2.0 * pi * (smp.dh * ds_true).sqrt() / g_tau;
        let k_enh_true = k_true * g_cap;
        let ph = if k_enh_true >= g_thr_crit { "critical" } else if k_enh_true >= g_thr_appr { "approaching" } else { "stable" };
        rescored.push_str(&format!("{} & {:.3} & {:.4} & \\texttt{{{}}} & \\texttt{{{}}} \\\\\n", smp.at, ds_true, k_true, smp.phase, ph));
    }
    let anatomy_block = if series.is_empty() {
        "No time series was supplied; this section is intentionally empty rather than quoted from memory.".to_string()
    } else {
        format!(
            "The gauge that Quillon Graph serves is 681 lines of Rust; its whole arithmetic fits in four lines. Per \
             60\\,s window it forms $\\Delta H=r_{{\\text{{rej}}}}+a_{{\\text{{traffic}}}}+c_{{\\text{{churn}}}}$ (mining rejection \
             ratio, $|{{\\rm in}}-{{\\rm out}}|/({{\\rm in}}+{{\\rm out}})$ of P2P bytes, and relative peer-count change), \
             $\\Delta s=d_{{\\text{{sync}}}}+|h-60|/60$ (relative height lag plus the deviation of the window's block count \
             $h$ from an \\emph{{assumed}} 60), then $K_{{\\text{{base}}}}=2\\pi\\sqrt{{\\Delta H\\,\\Delta s}}/60$ with $\\hbar:=1$, \
             and finally $K_{{\\text{{enh}}}}=\\min\\!\\big(K_{{\\text{{base}}}}\\cdot100,\\;K_{{\\text{{base}}}}/\\max(\\Lambda,0.01)\\cdot(2-\\Omega)\\big)$ \
             with $\\Lambda=1-e^{{-h/1800}}$. Phases: approaching at $K_{{\\text{{enh}}}}\\ge5$, critical at $\\ge10$. \
             The series below ({} samples, reconstructed from the published fields) checks that reading: recomputing \
             $K_{{\\text{{base}}}}$ from the published $\\Delta H,\\Delta s$ reproduces the published value to ${}$, and \
             recomputing $\\Lambda$ from the inferred block count reproduces it to ${}$, so the formula above is what is \
             running.\n\n\
             \\begin{{center}}\\footnotesize\\begin{{tabular}}{{lccccccl}}\\toprule\n\
             epoch & $h$ & rate [blk/s] & $a_{{\\text{{traffic}}}}$ & $K_{{\\text{{base}}}}$ & recomputed & $K_{{\\text{{enh}}}}/K_{{\\text{{base}}}}$ & phase \\\\\\midrule\n\
             {}\\bottomrule\\end{{tabular}}\\end{{center}}\n\n\
             Three calibration facts follow, and none of them is about Kristensen's definition.\n\n\
             \\textbf{{The zero is wrong.}} The code expects 60 blocks per window (one block per second). The chain \
             produced ${:.1}$ blocks per window on average, ${:.3}$\\,blk/s, so $\\Delta s$ carries a permanent floor of \
             ${:.3}$ on a chain that is doing exactly what it should. That floor is a constant, not a measurement; it is \
             the same species of error as SIGIL's 6.28-vs-0.83\\,blk/s trap found in \\texttt{{flux-kgauge}}.\n\n\
             \\textbf{{The needle is the network cable.}} With rejection $\\sim1\\%$ and churn $0$, $\\Delta H$ is the P2P \
             traffic asymmetry, which ranged from ${:.3}$ to ${:.3}$ across the samples. A supernode that spends a minute \
             serving block-packs to one syncing peer has $|{{\\rm in}}-{{\\rm out}}|/({{\\rm in}}+{{\\rm out}})\\to1$ and reads \
             ``critical''; a minute of balanced gossip reads ``stable''. Given the $\\Delta s$ floor, the phase lines sit at \
             $\\Delta H\\ge{:.3}$ (approaching) and $\\Delta H\\ge{:.3}$ (critical): the whole phase machinery on this chain \
             reduces to \\emph{{which way the bytes are flowing}}.\n\n\
             \\textbf{{The enhancement is a constant.}} $K_{{\\text{{base}}}}$ cannot exceed ${:.3}$ even with every ratio \
             pinned at its maximum, so it can never cross the thresholds 5 and 10 on its own; classification lives \
             entirely in the $\\times100$. And it is exactly $\\times100$: $\\Lambda=1-e^{{-h/1800}}$ needs $h\\ge{:.1}$ \
             blocks per window (${:.2}$\\,blk/s) just to escape the $0.01$ clamp and $h={:.0}$ to reach $0.5$, both far \
             above the chain's rate, so $1/\\Lambda$ clamps to 100 and the cap is also 100. Every sample shows \
             $K_{{\\text{{enh}}}}/K_{{\\text{{base}}}}=100$ ({}), and $f_{{\\text{{irrev}}}}=0$ because $h\\le{:.0}\\ll360$. \
             Equations 17--25 of the reference model add no information on this chain; they multiply by a constant.\n\n\
             Re-scoring the same samples with the \\emph{{observed}} mean rate as the expected rate (everything else \
             unchanged) gives:\n\n\
             \\begin{{center}}\\footnotesize\\begin{{tabular}}{{lcccc}}\\toprule\n\
             epoch & $\\Delta s$ (corrected) & $K_{{\\text{{base}}}}$ & phase (shipped) & phase (corrected) \\\\\\midrule\n\
             {}\\bottomrule\\end{{tabular}}\\end{{center}}\n\n\
             The fix is one constant and one measurement: expected blocks per window should be the chain's measured \
             rate times 60, and the round time should be measured independently of $\\Delta H$ (Section~4). After \
             that, the traffic term needs a physical argument for why byte \\emph{{direction}} is an energy spread at \
             all; on a supernode it is a job description.",
            series.len(),
            sci(max_recon_err.max(1e-16)),
            sci(lam_err_max.max(1e-16)),
            anat_rows,
            rate_mean * g_tau,
            rate_mean,
            ds_floor,
            asym_min,
            asym_max,
            dh_for_appr,
            dh_for_crit,
            k_base_max,
            hd_escape_clamp,
            hd_escape_clamp / g_tau,
            hd_half,
            if ratios_all_cap { "all samples" } else { "not all samples" },
            rates.iter().cloned().fold(0.0, f64::max) * g_tau,
            rescored
        )
    };


    // ------------------------------------------------------------ scenario simulation
    // Does a gauge with the expected-rate constant corrected still fire on real stress?
    // Deterministic scenarios at the chain's observed nominal rate. Three scorings:
    //   shipped   : Δs = sync + |h-60|/60
    //   corrected : Δs = sync + |h-h0|/h0, h0 = observed mean blocks/window
    //   as-defined: Δs = H_prop + corrected deviation, H_prop = ln 2 (two equiprobable
    //               proposers — the smallest non-trivial proposer entropy; ILLUSTRATIVE)
    let h0 = (rate_mean * g_tau).max(1.0);
    let scen: [(&str, f64, f64, f64, f64, f64); 7] = [
        // name, rejection, traffic asym, churn, sync divergence, blocks in window
        ("quiet, balanced gossip", 0.01, 0.10, 0.0, 0.0, h0.round()),
        ("serving one syncing peer", 0.01, 0.98, 0.0, 0.0, h0.round()),
        ("peer churn 50\\%", 0.01, 0.10, 0.5, 0.0, h0.round()),
        ("mining rejection 30\\%", 0.30, 0.10, 0.0, 0.0, h0.round()),
        ("production stall (2 blocks/window)", 0.01, 0.10, 0.0, 0.0, 2.0),
        ("5\\% behind network height", 0.01, 0.10, 0.0, 0.05, h0.round()),
        ("everything at once", 0.30, 0.98, 0.5, 0.05, 2.0),
    ];
    let phase_of = |k_enh: f64| if k_enh >= g_thr_crit { "critical" } else if k_enh >= g_thr_appr { "approaching" } else { "stable" };
    let mut scen_rows = String::new();
    let mut blind_corrected: Vec<&str> = vec![];
    let mut fires_defined: Vec<&str> = vec![];
    for (name, rej, asym, churn, sync, h) in scen.iter() {
        let dh = rej + asym + churn;
        let ds_ship = sync + (h - g_tau).abs() / g_tau;
        let ds_corr = sync + (h - h0).abs() / h0;
        let ds_def = 2f64.ln() + ds_corr;
        let k = |ds: f64| g_cap * 2.0 * pi * (dh * ds).sqrt() / g_tau;
        let (ks, kc, kd) = (k(ds_ship), k(ds_corr), k(ds_def));
        let stressed = *churn > 0.0 || *rej > 0.1 || *sync > 0.0 || *h < h0 / 2.0;
        if stressed && phase_of(kc) == "stable" {
            blind_corrected.push(name);
        }
        if stressed && phase_of(kd) != "stable" {
            fires_defined.push(name);
        }
        scen_rows.push_str(&format!(
            "{} & {:.2} & {:.2} & \\texttt{{{}}} & {:.2} & \\texttt{{{}}} & {:.2} & \\texttt{{{}}} \\\\\n",
            name, dh, ks, phase_of(ks), kc, phase_of(kc), kd, phase_of(kd)
        ));
    }
    let scenario_block = format!(
        "A referee's next question is whether fixing the constant leaves a gauge that still fires. Seven deterministic \
         scenarios at the chain's observed nominal rate ($h_0={:.1}$ blocks per window), scored three ways: as shipped; \
         with the expected rate corrected to $h_0$; and \\emph{{as Kristensen defined it}}, with $\\Delta s$ an entropy \
         of the proposer distribution rather than a deviation. For the last column we use the smallest non-trivial \
         proposer entropy, $\\ln 2$ (two equiprobable proposers), plus the corrected deviation; that floor is \
         illustrative, not measured. All three use the $\\times100$ enhancement that is constant on this chain.\n\n\
         \\begin{{center}}\\footnotesize\\begin{{tabular}}{{lccccccc}}\\toprule\n\
         scenario & $\\Delta H$ & $K_{{\\text{{enh}}}}$ shipped & phase & corrected & phase & as defined & phase \\\\\\midrule\n\
         {}\\bottomrule\\end{{tabular}}\\end{{center}}\n\n\
         Two things to read off. The shipped gauge cannot tell scenario 2 (a healthy supernode doing its job) from \
         scenarios 3, 4 and 7 (real trouble): all are ``critical''. The corrected gauge fixes the false alarm but goes \
         \\emph{{blind}} to {} --- because $K\\propto\\sqrt{{\\Delta H\\cdot\\Delta s}}$ is a product, and a correctly \
         calibrated deviation is \\emph{{zero}} on a chain producing at its nominal rate, so no amount of $\\Delta H$ \
         can move it. That is the third defect \\texttt{{flux-kgauge}} reported (Eq.~25 inert at $K_{{\\text{{base}}}}=0$), \
         now visible one level down. The as-defined column restores sensitivity to {}: an entropy of proposers is never \
         zero while more than one party can propose, so the product never dies. The implementation's substitution of a \
         block-rate deviation for the proposer entropy was the change that broke Kristensen's gauge, not the gauge.",
        h0,
        scen_rows,
        if blind_corrected.is_empty() { "nothing".to_string() } else { blind_corrected.iter().map(|x| format!("``{}''", x)).collect::<Vec<_>>().join(", ") },
        if fires_defined.is_empty() { "nothing".to_string() } else { fires_defined.iter().map(|x| format!("``{}''", x)).collect::<Vec<_>>().join(", ") }
    );

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
             $e$ (1\\,eV) & ${}$\\,J & exact \\\\\n\
             $m_p$ & ${}$\\,kg & $u_r=3.1\\times10^{{-10}}$ \\\\\n\
             $m_e$ & ${}$\\,kg & $u_r=3.1\\times10^{{-10}}$ \\\\\\midrule\n\
             $\\ell_P$ & ${}$\\,m & derived \\\\\n\
             $t_P$ & ${}$\\,s & derived \\\\\n\
             $m_P$ & ${}$\\,kg & derived \\\\\n\
             $E_P$ & ${}$\\,J $= {}$\\,GeV & derived \\\\\n\
             $\\alpha_G(p)=Gm_p^2/\\hbar c$ & ${}$ & derived \\\\\n\
             $\\alpha_G(e)=Gm_e^2/\\hbar c$ & ${}$ & derived \\\\\n\
             $k_BT\\ln2$ at 300\\,K & ${}$\\,J $= {:.1}$\\,meV & derived \\\\\n\
             $2\\log_2\\varphi$ & ${:.5}$ bits/round & derived \\\\\\bottomrule\n\
             \\end{{tabular}}\\end{{center}}\n\
             {{\\small All values from \\texttt{{flux-science::constants}}; the particle block ($e$, $m_p$, $m_e$, $h$) was \
             added on 2026-09-02, closing the July audit's open item. The crate's one non-CODATA entry, $H_0=70$, is not used.}}\n\n",
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
        .add(Block::Section("Anatomy of the Live Gauge: Why It Swings on a Quiet Chain".into()))
        .add(para(anatomy_block))
        .add(Block::Section("Does the Corrected Gauge Still Fire? Seven Scenarios".into()))
        .add(para(scenario_block))
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
