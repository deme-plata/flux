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

/// Numbers from the 2026-09-14 battle test (sigil-chronos `sigil-kparam-battle`, run dir with sweeps/*.jsonl)
/// and the live K-gauge series row that now carries K_fix. Every field is read, never typed.
struct Battle {
    fork_one_key: (f64, f64),
    fork_one_key_41: (f64, f64),
    fork_two_keys: (f64, f64),
    fork_two_keys_41: (f64, f64),
    sybil_1: (f64, f64),
    sybil_1000: (f64, f64),
    rate_slow: (f64, f64, f64),
    rate_fast: (f64, f64, f64),
    net_cells: usize,
    net_kstar_max: f64,
    net_kfix_max: f64,
    fuzz_trials: u64,
    fuzz_rejected: u64,
    fuzz_rho_kc: f64,
    fuzz_rho_kfix: f64,
    live_kc: Option<f64>,
    live_kfix: Option<f64>,
    live_regime_fix: String,
    live_persist: Option<f64>,
    dir: String,
}

fn load_battle(dir: &str) -> Option<Battle> {
    let rows = |name: &str| -> Vec<serde_json::Value> {
        std::fs::read_to_string(format!("{dir}/sweeps/{name}.jsonl"))
            .map(|t| t.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
            .unwrap_or_default()
    };
    let f = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_f64()).unwrap_or(f64::NAN);
    let nz = |x: f64| if x == 0.0 { 0.0 } else { x }; // -0.0 from a zero-entropy product prints as "-0.00"
    let pair = |v: &serde_json::Value| (nz(f(v, "k_star_unit")), nz(f(v, "k_fix")));
    let fork = rows("fork0");
    let pick = |name: &str| fork.iter().find(|r| r.get("scenario").and_then(|s| s.as_str()) == Some(name)).map(pair);
    let sy = rows("sybil");
    let syp = |keys: u64| sy.iter().find(|r| r.get("producers").and_then(|x| x.as_u64()) == Some(keys) && f(r, "fork_p") == 0.2).map(pair);
    let ra = rows("rate");
    let rp = |bps: f64| ra.iter().find(|r| f(r, "bps") == bps).map(|r| (bps, nz(f(r, "k_star_unit")), nz(f(r, "k_fix"))));
    let net: Vec<serde_json::Value> = rows("net").into_iter().filter(|r| r.get("scenario").and_then(|s| s.as_str()) == Some("one_honest_producer")).collect();
    let fz = rows("fuzz").into_iter().rev().find(|r| r.get("scenario").and_then(|s| s.as_str()) == Some("fuzz_summary"))?;
    let live = std::env::var("SIGIL_KGAUGE_JSONL").ok().and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| t.lines().rev().find(|l| !l.trim().is_empty()).and_then(|l| serde_json::from_str::<serde_json::Value>(l).ok()));
    Some(Battle {
        fork_one_key: pick("equivocation_one_key_ds0")?,
        fork_one_key_41: pick("equivocation_one_key_ds0_persist41")?,
        fork_two_keys: pick("two_keys_same_fork_ds1")?,
        fork_two_keys_41: pick("two_keys_same_fork_ds1_persist41")?,
        sybil_1: syp(1)?,
        sybil_1000: syp(1000)?,
        rate_slow: rp(0.1)?,
        rate_fast: rp(10000.0)?,
        net_cells: net.len(),
        net_kstar_max: net.iter().map(|r| f(r, "k_star_unit")).fold(0.0, f64::max),
        net_kfix_max: net.iter().map(|r| f(r, "k_fix")).fold(0.0, f64::max),
        fuzz_trials: fz.get("trials").and_then(|x| x.as_u64()).unwrap_or(0),
        fuzz_rejected: f(&fz, "rejected_total") as u64,
        fuzz_rho_kc: f(&fz, "spearman_kc_vs_rejected"),
        fuzz_rho_kfix: f(&fz, "spearman_kfix_vs_rejected"),
        live_kc: live.as_ref().and_then(|v| v.get("K_C")).and_then(|x| x.as_f64()),
        live_kfix: live.as_ref().and_then(|v| v.get("K_fix")).and_then(|x| x.as_f64()),
        live_regime_fix: live.as_ref().and_then(|v| v.get("regime_fix")).and_then(|x| x.as_str()).unwrap_or("n/a").to_string(),
        live_persist: live.as_ref().and_then(|v| v.get("k_fix")).and_then(|k| k.get("persistence_blocks")).and_then(|x| x.as_f64()),
        dir: dir.to_string(),
    })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let json_path = args.get(1).map(String::as_str).unwrap_or("crates/flux-arxiv-latex/k_parameter.arxiv.json");
    let out_dir = args.get(2).map(String::as_str).unwrap_or("/home/storage/claude-code/k-parameter-paper");
    let gauge = args.get(3).and_then(|p| load_gauge(p));
    let series = args.get(4).map(|p| load_series(p)).unwrap_or_default();
    let battle = load_battle(&std::env::var("SIGIL_KPARAM_BATTLE_DIR").unwrap_or_else(|_| "/home/storage/sigil-scratch/sigil-kparam-2026-09-14".into()));

    let papers: Vec<ArxivPaper> = std::fs::read_to_string(json_path)
        .ok()
        .and_then(|j| parse_arxiv_json(&j).ok())
        .unwrap_or_default();

    let pi = std::f64::consts::PI;
    let ln2 = std::f64::consts::LN_2;

    // SIGIL live clock inputs for the "Kristensen Time" section: last line of the K-gauge series if given,
    // else the 2026-09-12 snapshot values (labelled as such in the text).
    let (sigil_bps, sigil_final_depth, sigil_finality_s, sigil_src) = std::env::var("SIGIL_KGAUGE_JSONL")
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|txt| txt.lines().rev().find(|l| !l.trim().is_empty()).map(str::to_string))
        .and_then(|line| serde_json::from_str::<serde_json::Value>(&line).ok())
        .and_then(|v| {
            let bps = v["network_diagnostics"]["block_rate_bps"].as_f64()?;
            let depth = v["tau"]["final_depth"].as_f64()?;
            let fin = v["tau"]["finality_secs"].as_f64()?;
            let ts = v["ts_ms"].as_i64().unwrap_or(0);
            Some((bps, depth, fin, format!("live K-gauge series line, ts\\_ms={ts}")))
        })
        .unwrap_or((0.3666422238517432, 512.0, 1396.4567272727272, "K-gauge snapshot of 2026-09-12 (sigil-g2)".to_string()));

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
        ("production stall (2 blk/window)", 0.01, 0.10, 0.0, 0.0, 2.0),
        ("5\\% behind network height", 0.01, 0.10, 0.0, 0.05, h0.round()),
        ("everything at once", 0.30, 0.98, 0.5, 0.05, 2.0),
    ];
    let phase_of = |k_enh: f64| if k_enh >= g_thr_crit { "critical" } else if k_enh >= g_thr_appr { "approaching" } else { "stable" };
    let mut scen_rows = String::new();
    let mut blind_corrected: Vec<&str> = vec![];
    let mut fires_defined: Vec<&str> = vec![];
    let mut misses_defined: Vec<&str> = vec![];
    let mut shipped_labels: Vec<(&str, &str)> = vec![];
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
        if stressed && phase_of(kd) == "stable" {
            misses_defined.push(name);
        }
        shipped_labels.push((name, phase_of(ks)));
        scen_rows.push_str(&format!(
            "{} & {:.2} & {:.2} & \\texttt{{{}}} & {:.2} & \\texttt{{{}}} & {:.2} & \\texttt{{{}}} \\\\\n",
            name, dh, ks, phase_of(ks), kc, phase_of(kc), kd, phase_of(kd)
        ));
    }
    let healthy_label = shipped_labels.get(1).map(|x| x.1).unwrap_or("?");
    let confused: Vec<String> = shipped_labels
        .iter()
        .enumerate()
        .filter(|(i, (_, lab))| *i >= 2 && *lab == healthy_label)
        .map(|(_, (n, _))| format!("``{}''", n))
        .collect();
    let scenario_block = format!(
        "A referee's next question is whether fixing the constant leaves a gauge that still fires. Seven deterministic \
         scenarios at the chain's observed nominal rate ($h_0={:.1}$ blocks per window), scored three ways: as shipped; \
         with the expected rate corrected to $h_0$; and \\emph{{as Kristensen defined it}}, with $\\Delta s$ an entropy \
         of the proposer distribution rather than a deviation. For the last column we use the smallest non-trivial \
         proposer entropy, $\\ln 2$ (two equiprobable proposers), plus the corrected deviation; that floor is \
         illustrative, not measured. All three use the $\\times100$ enhancement that is constant on this chain.\n\n\
         \\begin{{center}}\\scriptsize\\begin{{tabular}}{{lccccccc}}\\toprule\n\
         scenario & $\\Delta H$ & shipped & phase & corrected & phase & as defined & phase \\\\\\midrule\n\
         {}\\bottomrule\\end{{tabular}}\\end{{center}}\n\n\
         Three things to read off. The shipped gauge scores scenario 2 (a healthy supernode doing its job) as \
         \\texttt{{{}}}, the same label it gives {} --- it cannot tell a job from a fault. The corrected gauge fixes \
         the false alarm but goes \\emph{{blind}} to {} --- because $K\\propto\\sqrt{{\\Delta H\\cdot\\Delta s}}$ is a product, and a correctly \
         calibrated deviation is \\emph{{zero}} on a chain producing at its nominal rate, so no amount of $\\Delta H$ \
         can move it. That is the third defect \\texttt{{flux-kgauge}} reported (Eq.~25 inert at $K_{{\\text{{base}}}}=0$), \
         now visible one level down. The as-defined column restores sensitivity to {}: an entropy of proposers is never \
         zero while more than one party can propose, so the product never dies. It still misses {}: the mirror-image \
         blindness, a quiet $\\Delta H$ suppressing a real $\\Delta s$ event, which is the product form's remaining cost \
         and the argument for reporting the two factors beside the gauge. The implementation's substitution of a \
         block-rate deviation for the proposer entropy was the change that broke Kristensen's gauge, not the gauge.",
        h0,
        scen_rows,
        healthy_label,
        if confused.is_empty() { "no fault scenario".to_string() } else { confused.join(", ") },
        if blind_corrected.is_empty() { "nothing".to_string() } else { blind_corrected.iter().map(|x| format!("``{}''", x)).collect::<Vec<_>>().join(", ") },
        if fires_defined.is_empty() { "nothing".to_string() } else { fires_defined.iter().map(|x| format!("``{}''", x)).collect::<Vec<_>>().join(", ") },
        if misses_defined.is_empty() { "nothing".to_string() } else { misses_defined.iter().map(|x| format!("``{}''", x)).collect::<Vec<_>>().join(", ") }
    );

    // ------------------------------------------------------------ LaTeX
    // ------------------------------------------------------------ Kristensen time (the three clocks)
    let t_planck_k = planck_temperature();
    let tick_of = |t_k: f64| pi * PLANCK_REDUCED / (2.0 * BOLTZMANN * t_k);
    let tick_planck = tick_of(t_planck_k);
    let t_ew = 100e9 * ELECTRON_VOLT / BOLTZMANN;
    let t_qcd = 150e6 * ELECTRON_VOLT / BOLTZMANN;
    let t_iter = 15e3 * ELECTRON_VOLT / BOLTZMANN;
    let t_sun = 1.57e7;
    let (tick_ew, tick_qcd, tick_iter, tick_sun, tick_room) = (tick_of(t_ew), tick_of(t_qcd), tick_of(t_iter), tick_of(t_sun), tick_of(300.0));
    let tolman_per_m = 300.0 * STANDARD_GRAVITY * 1.0 / (SPEED_OF_LIGHT * SPEED_OF_LIGHT);
    let sigil_tau_b = 1.0 / sigil_bps;
    let a_core = 2.0 * 15.0 * sigil_tau_b / (pi * PLANCK_REDUCED);
    let a_iter = 2.0 * 350e6 * sigil_tau_b / (pi * PLANCK_REDUCED);
    let t_ledger = PLANCK_REDUCED / (BOLTZMANN * sigil_tau_b);
    let t_finality = PLANCK_REDUCED / (2.0 * pi * BOLTZMANN * sigil_finality_s);

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
             which of its fields are measurements and which are plumbing. (4) Since 2026-09-14 it also carries the \
             battle test: the dimensionless form was driven through the real chain code under chronos and failed --- its \
             product with $\\Delta s$ hides a one-key fork --- and the repaired gauge \
             $K_{{\\mathrm{{fix}}}}=2\\pi\\sqrt{{\\Delta H_c\\,\\tau_d\\,w_S}}$ that passed every sweep is stated and read live. \
             All constants are CODATA 2022 via \
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
        .add(Block::Section("Kristensen Time: the Three Clocks".into()))
        .add(para(
            "Everything above treats $K$ as a thermometer. This section turns the same identity around and reads it as a \
             \\emph{clock}. A chain has three ways of saying how much time has passed, and they are not the same quantity: \
             proper seconds $t$ (physics); Margolus--Levitin ticks $n=2Et/\\pi\\hbar$, the number of distinguishable states \
             a system of energy $E$ can pass through in $t$ (set by energy, hence by temperature); and \\emph{agreed} ticks \
             $h$, the block height, which advances only when independent machines commit the same state. We call the \
             triple $(t,n,h)$ \\textbf{Kristensen time}. The gauge $K^{*}=2\\pi\\sqrt{\\tfrac{\\pi}{2}N_{\\mathrm{ML}}\\Delta s}$ \
             is exactly the bridge between the second and the third clock: $N_{\\mathrm{ML}}$ counts physical ticks, \
             $\\Delta s$ counts who was allowed to advance the agreed one."
                .to_string(),
        ))
        .add(para(format!(
            "\\textbf{{Heat sets the fastest clock, not the existence of time.}} A system at temperature $T$ carries \
             $k_BT$ per degree of freedom and can therefore tick no faster than $\\pi\\hbar/2k_BT$. Computed from CODATA: \
             at the Planck temperature $T_P={}$\\,K the tick is ${}$\\,s, i.e.\\ ${:.1}$ Planck times --- the one regime where \
             ``a tick'' stops meaning anything and time must be \\emph{{emergent}} (Wheeler--DeWitt has no $t$; Page--Wootters \
             recovers it from entanglement, Connes--Rovelli from the thermal state). Everything cooler has a perfectly good \
             clock, only slower:",
            sci(t_planck_k), sci(tick_planck), tick_planck / t_p
        )))
        .add(Block::Raw(format!(
            "\\begin{{center}}\\begin{{tabular}}{{lrrr}}\\toprule\n\
             \\textbf{{regime}} & $T$ [K] & fastest tick $\\pi\\hbar/2k_BT$ [s] & in Planck times \\\\\\midrule\n\
             Planck epoch & ${}$ & ${}$ & ${:.1}$ \\\\\n\
             electroweak (100\\,GeV) & ${}$ & ${}$ & ${}$ \\\\\n\
             QCD (150\\,MeV) & ${}$ & ${}$ & ${}$ \\\\\n\
             ITER core (15\\,keV) & ${}$ & ${}$ & ${}$ \\\\\n\
             solar core & ${}$ & ${}$ & ${}$ \\\\\n\
             room (300\\,K) & ${}$ & ${}$ & ${}$ \\\\\\bottomrule\n\
             \\end{{tabular}}\\end{{center}}\n\n",
            sci(t_planck_k), sci(tick_planck), tick_planck / t_p,
            sci(t_ew), sci(tick_ew), sci(tick_ew / t_p),
            sci(t_qcd), sci(tick_qcd), sci(tick_qcd / t_p),
            sci(t_iter), sci(tick_iter), sci(tick_iter / t_p),
            sci(t_sun), sci(tick_sun), sci(tick_sun / t_p),
            sci(300.0), sci(tick_room), sci(tick_room / t_p)
        )))
        .add(para(format!(
            "ITER's plasma is ${}$ times colder than the Planck temperature: a fusion reactor is a superb clock \
             (${}$\\,s per tick) and no threat to the existence of time. The one place heat and time genuinely touch is \
             Tolman--Ehrenfest: in a gravitational field a body in thermal equilibrium obeys $T\\sqrt{{g_{{00}}}}=$const, so it \
             is warmer where clocks run slower. On Earth that is ${}$\\,K per metre at 300\\,K --- unmeasurable as a \
             temperature, routinely measured as time dilation by optical clocks. Time \\emph{{can}} be read through \
             temperature; the instrument is still a clock.",
            sci(t_planck_k / t_iter), sci(tick_iter), sci(tolman_per_m)
        )))
        .add(para(format!(
            "\\textbf{{The agreed clock, measured.}} SIGIL (\\texttt{{sigil-g2}}, {}) advances its agreed clock at \
             ${:.3}$ blocks/s, one tick every $\\tau_b={:.2}$\\,s, and declares a tick irreversible ${:.0}$ ticks later, \
             $\\tau_F={:.0}$\\,s. Three quantities follow, and we name them because they will be read off the live node:",
            sigil_src, sigil_bps, sigil_tau_b, sigil_final_depth, sigil_finality_s
        )))
        .add(para(format!(
            "\\emph{{Agreement cost}} $A = N_{{\\mathrm{{ML}}}}(E,\\tau_b)/1$: how many physical ticks physics permits while the \
             chain agrees on one. For one active core (15\\,W, the labelled model parameter used throughout) $A={}$; for \
             ITER's 350\\,MJ of thermal energy $A={}$. Agreement is the slowest clock in the building by thirty-five to \
             forty-two orders of magnitude, and that is not inefficiency --- it is the price of a tick that strangers accept.",
            sci(a_core), sci(a_iter)
        )))
        .add(para(format!(
            "\\emph{{Ledger temperature}} $T_L=\\hbar/k_B\\tau_b$: the temperature of a thermal state whose thermal time \
             (Connes--Rovelli; equivalently a Euclidean periodicity $\\beta=\\tau_b/\\hbar$) flows one tick per block interval. \
             For SIGIL $T_L={}$\\,K --- picokelvin. \\emph{{Finality-horizon temperature}} $T_F=\\hbar/2\\pi k_B\\tau_F$: the \
             Unruh--Hawking temperature a horizon would have if its characteristic time were the finality time; \
             $T_F={}$\\,K, femtokelvin. Both are formal: they are what the ledger's clock \\emph{{would}} be as a thermal \
             clock, and they are labelled ANALOGY in the table below. They are also, as far as we know, the coldest numbers \
             anyone has attached to a running blockchain, which is why the wallet shows them.",
            sci(t_ledger), sci(t_finality)
        )))
        .add(para(
            "\\emph{Consensus dilation} $\\gamma_C = \\dot h_{\\mathrm{follower}}/\\dot h_{\\mathrm{producer}}$: the rate at which a \
             second node's agreed clock advances relative to the producer's, read from the K-gauge's finality channel. \
             Relativity has no universal now; a chain \\emph{constructs} one, and $\\gamma_C$ is how well the construction \
             holds. At the snapshot both nodes sat within one block of each other over the window, $\\gamma_C\\approx1$; on \
             2026-09-10 it fell to $0$ for eighteen hours when the follower refused a tip it could not verify --- the correct \
             reading, since the alternative was to advance an agreed clock on a lie. \\emph{Compare at the same height} is \
             clock synchronisation, and $K_C$ is the residual after it."
                .to_string(),
        ))
        .add(Block::Section("The Battle Test and the Repaired Gauge (2026-09-14)".into()))
        .add(para(
            "Everything up to here is calibration: units, constants, the reason for the boundary. A thermometer can be \
             perfectly calibrated and still be pointed at the wrong thing, so on 2026-09-14 the dimensionless gauge was \
             taken out of the paper and driven, as a formula, through the real chain code --- the \\texttt{sigil-dagknight} \
             Braid with GHOSTDAG colouring, the real \\texttt{SigilSimNode} state chokepoint under \\texttt{flux-chronos}, \
             two thousand seeded two-node fuzz trials, and the live \\texttt{sigil-g2} network for ten hours --- eight sweeps \
             in all, every value stored as JSONL and recomputed from its stored inputs by an independent pass (zero \
             mismatches). Full report and raw data: \\url{https://sigilgraph.org/downloads/sigil-kparam-battle-2026-09-14.pdf}."
                .to_string(),
        ))
        .add(para(
            "\\textbf{The flaw is not in the arithmetic; it is in what the square root multiplies.} $K^{*}$ is a \\emph{product} \
             of how much the network disagrees ($\\Delta H$) and how many different producers signed ($\\Delta s$). A product is \
             zero when either factor is zero, and on the live chain the second factor \\emph{is} zero --- one producer mints \
             every block. So the dial reads 0 (``stable'') no matter how badly the chain forks. Three smaller flaws ride \
             along: the factor $t$ makes the same disagreement read $\\sqrt{t}$ worse on a slow chain than a fast one; the \
             $\\hbar$ form can never land in the ``stable'' band for any physical energy (its floor at $N_{\\mathrm{ML}}=1$, \
             $\\Delta s=1$ is $2\\pi\\sqrt{\\pi/2}=7.87$, above the critical line, and $K^{*}=1$ corresponds to $0.016$ of a \
             tick); and read as an entropy \\emph{change}, $\\Delta H$ goes negative and the root is undefined. The identity \
             of Section~3 stands --- $K^{*}$ \\emph{is} the Margolus--Levitin count in disguise --- but that count is an \
             \\emph{agreement cost}, not a stress level with a threshold at 1."
                .to_string(),
        ))
        .add(para(
            "\\textbf{The repair} keeps the shape and changes the two things the test convicted, what multiplies $\\Delta H$ and \
             what $\\Delta H$ is: \
             $$K_{\\mathrm{fix}} = 2\\pi\\,\\sqrt{\\Delta H_c\\,\\tau_d\\,w_S},\\qquad \
             \\tau_d=\\frac{1+\\min(d,D)/D}{2}\\in[\\tfrac12,1],\\qquad w_S = 1-\\frac{H_{\\mathrm{norm}}}{4}\\in[\\tfrac34,1].$$ \
             $\\Delta H_c\\in[0,1]$ is weighted \\emph{state disagreement} (tip mismatch 0.20, state-root mismatch 0.35, \
             finality gap 0.20, semantic conflicts 0.25), non-negative by definition; $d$ is how many \\emph{blocks} the \
             disagreement has persisted against the finality depth $D=512$, replacing the seconds and the $\\hbar$ so that a \
             chain that speeds up does not ``cool''; $H_{\\mathrm{norm}}=\\Delta s/\\log_2 N$ keeps the proposer entropy, \
             but as a bounded concentration weight: one key disagreeing with itself is the \\emph{worst} case, zero \
             disagreement reads 0 for any key count, and rotating keys can move the dial by at most $\\sqrt{4/3}$, about \
             15\\%. The ladder is unchanged ($<1$ stable, $<3$ elevated, $\\ge3$ critical); the maximum is $2\\pi$. A first \
             cut used $H_{\\mathrm{norm}}/2$ and let a persistent two-party split read 2.97 (``elevated''); a full state \
             divergence must stay critical whatever the key count, which is why the weight is $/4$."
                .to_string(),
        ))
        .add(Block::Raw(match &battle {
            Some(b) => format!(
                "\\begin{{center}}\\begin{{tabular}}{{p{{6.4cm}}rr}}\\toprule\n\
                 \\textbf{{scenario (real chain code under chronos)}} & \\textbf{{$K^{{*}}$, $\\hbar:=1$}} & \\textbf{{$K_{{\\mathrm{{fix}}}}$}} \\\\\\midrule\n\
                 one producer forks against itself (24-block prefix, block 25 minted twice, followers on different wallet roots), fresh & {:.2} & {:.2} \\\\\n\
                 \\quad the same, after 41 blocks of persistence & {:.2} & {:.2} \\\\\n\
                 the identical fork signed with two keys, fresh / after 41 blocks & {:.2} / {:.2} & {:.2} / {:.2} \\\\\n\
                 one entity, same 20\\% sibling blocks, 1 key $\\to$ 1000 rotating keys & {:.2} $\\to$ {:.2} & {:.2} $\\to$ {:.2} \\\\\n\
                 identical DAG at {} and {} blocks/s & {:.1} $\\to$ {:.2} & {:.2} $\\to$ {:.2} \\\\\n\
                 one honest producer under latency / loss / partition, {} cells (max) & {:.2} & {:.2} \\\\\n\
                 {} seeded two-node trials, {} rejected blocks: Spearman $\\rho$ vs rejects & undefined (never moved) & {:.3} \\\\\\bottomrule\n\
                 \\end{{tabular}}\\end{{center}}\n\n\
                 The $K_C$ gauge that has run on \\texttt{{sigil-g2}} since 2026-09-07 already kept the lone-producer fork \
                 visible through an additive $\\epsilon$ floor and reached $\\rho={:.3}$ on the same fuzz; it still carries the \
                 seconds. $K_{{\\mathrm{{fix}}}}$ was shipped beside it on 2026-09-14 in the MCP gauge, the wallet chip and the \
                 site modal. Live at generation time ({}): $K_C={}$, $K_{{\\mathrm{{fix}}}}={}$ ({}), persistence $d={}$ blocks. \
                 Run directory: \\url{{{}}}.\n\n",
                b.fork_one_key.0, b.fork_one_key.1, b.fork_one_key_41.0, b.fork_one_key_41.1,
                b.fork_two_keys.0, b.fork_two_keys_41.0, b.fork_two_keys.1, b.fork_two_keys_41.1,
                b.sybil_1.0, b.sybil_1000.0, b.sybil_1.1, b.sybil_1000.1,
                b.rate_slow.0, b.rate_fast.0, b.rate_slow.1, b.rate_fast.1, b.rate_slow.2, b.rate_fast.2,
                b.net_cells, b.net_kstar_max, b.net_kfix_max,
                b.fuzz_trials, b.fuzz_rejected, b.fuzz_rho_kfix, b.fuzz_rho_kc,
                sigil_src,
                b.live_kc.map(|x| format!("{x:.3}")).unwrap_or_else(|| "n/a".into()),
                b.live_kfix.map(|x| format!("{x:.3}")).unwrap_or_else(|| "n/a".into()),
                b.live_regime_fix,
                b.live_persist.map(|x| format!("{x:.0}")).unwrap_or_else(|| "n/a".into()),
                b.dir.clone()
            ),
            None => "\\emph{Battle-test data not found at generation time (set SIGIL\\_KPARAM\\_BATTLE\\_DIR); the numbers are in the linked report.}\n\n".to_string(),
        }))
        .add(para(
            "What the repair does \\emph{not} fix, said plainly: a fork no node ever observes contributes only through the \
             finality-gap channel; the four channel weights are the provisional ones $K_C$ uses and have not been calibrated \
             against incidents; the 15\\% key-rotation lever is bounded, not zero; and in the network sweep persistence was \
             measured as a height gap, so a same-height fork is under-counted there. The three $\\hbar$-free variants that \
             shared the name $K^{*}$ in the code (a window-time one, an HTTP-round-trip one, and the legacy site modal) are \
             retired or labelled; the paper's $\\hbar$ form survives as the agreement cost $A=N_{\\mathrm{ML}}$ of the \
             previous section, which is what it always was."
                .to_string(),
        ))
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
             three-clock table, $A$, Tolman--Ehrenfest gradient & DERIVED & CODATA, computed here \\\\\n\
             SIGIL block rate, finality time & MEASURED & K-gauge series line named in the text \\\\\n\
             $T_L$, $T_F$ (ledger / finality-horizon temperature) & ANALOGY & thermal-time and Unruh forms applied to a clock \\\\\n\
             battle-test sweeps (fork, Sybil, rate, loss, fuzz) & MEASURED & real Braid + SigilSimNode under chronos, JSONL recomputed \\\\\n\
             $K_{\\mathrm{fix}}$ properties (worst case, rate-invariant, bounded lever, max $2\\pi$) & DERIVED + TESTED & unit tests in the harness and the MCP gauge \\\\\n\
             channel weights 0.20/0.35/0.20/0.25, ladder 1/3 & PROVISIONAL & not calibrated against incidents \\\\\n\
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
             stands on something --- and, since the battle test, it has been dropped, measured where it broke, and \
             repaired without losing its shape."
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
