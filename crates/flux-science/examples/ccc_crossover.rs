//! CCC crossover verification — arXiv:2503.24263 (Meissner & Penrose 2025),
//! "The Physics of Conformal Cyclic Cosmology".
//!
//! Independently recomputes the paper's quantitative chain using flux-science's
//! own CODATA constants (NOT the paper's rounded intermediates), so every number
//! below is reproducible from first principles.
//!
//! Checks, in the paper's own order:
//!   (a) §I  — Hawking lifetime of a 10^15 M_sun cluster black hole ("~10^112 years")
//!   (b) §IV — eq (57) rho_LS  = rho_0 / a_LS^3          claimed 1.2e-20 kg/m^3
//!   (c) §IV — eq (58) delta_rho = 3 M_gc / (4 pi (eta^c_LS c a_LS)^3)  claimed ~1e-22 kg/m^3
//!   (d) §IV — eq (59) delta_T/T = delta_rho / (4 rho_LS)              claimed ~1e-3
//!
//! Run:  fluxc run -p flux-science --example ccc_crossover

use flux_science::blackhole::BlackHoleEvolution;
use flux_science::constants::{schwarzschild_radius, SPEED_OF_LIGHT};

/// Solar mass (kg) — same value flux-science's own constants test pins.
const M_SUN: f64 = 1.989e30;
/// Seconds in a Julian year.
const YEAR_S: f64 = 3.15576e7;

/// Paper inputs, quoted verbatim from arXiv:2503.24263.
mod paper {
    /// eq (56): mass of the largest galaxy clusters, in solar masses.
    pub const M_GC_SUNS: f64 = 1.0e15;
    /// eq (83): scale factor at last scattering.
    pub const A_LS: f64 = 1.0 / 1100.0;
    /// eq (84): present-day density, stated in kg/m^3.
    pub const RHO_0: f64 = 9.0e-27;
    /// eq (102): total conformal time to last scattering, eta^c_LS = eta_LS + eta_G.
    pub const ETA_C_LS: f64 = 4.9e16;
    /// §I: quoted evaporation lifetime for the cluster hole, in years.
    pub const LIFETIME_YEARS_CLAIMED: f64 = 1.0e112;
    /// eq (57) as printed, with its printed unit label kg/m^3.
    pub const RHO_LS_CLAIMED: f64 = 1.2e-20;
    /// eq (58) as printed, kg/m^3.
    pub const DELTA_RHO_CLAIMED: f64 = 1.0e-22;
    /// eq (59) as printed, dimensionless.
    pub const DELTA_T_OVER_T_CLAIMED: f64 = 1.0e-3;
}

/// Report a recomputed value against the paper's claim, in orders of magnitude.
fn check(label: &str, computed: f64, claimed: f64, unit: &str) -> f64 {
    let ratio = computed / claimed;
    let dex = ratio.log10();
    let verdict = if dex.abs() < 0.5 {
        "MATCH"
    } else if dex.abs() < 1.0 {
        "within 1 dex"
    } else {
        "MISMATCH"
    };
    println!(
        "  {label:<34} computed {computed:>12.4e} {unit:<8} | paper {claimed:>10.3e} \
         | ratio {ratio:>10.3e} ({dex:+.2} dex)  [{verdict}]"
    );
    dex
}

fn main() {
    println!("\n=== CCC crossover verification — arXiv:2503.24263 ===");
    println!("    Meissner & Penrose, 'The Physics of Conformal Cyclic Cosmology'");
    println!("    Recomputed from flux-science CODATA constants.\n");

    let m_gc = paper::M_GC_SUNS * M_SUN;
    println!("Cluster black hole: M_gc = {:.4e} M_sun = {:.4e} kg", paper::M_GC_SUNS, m_gc);

    // ---- (a) Hawking evaporation of the dominant cluster black hole -------
    println!("\n(a) §I — Hawking evaporation  [tau = 5120 pi G^2 M^3 / (hbar c^4)]");
    let bh = BlackHoleEvolution::new(m_gc, false);
    let tau_s = bh.lifetime();
    let tau_yr = tau_s / YEAR_S;
    let t_h = bh.hawking_temperature(m_gc);
    let r_s = schwarzschild_radius(m_gc);
    check("evaporation lifetime", tau_yr, paper::LIFETIME_YEARS_CLAIMED, "yr");
    println!("  {:<34} {:.4e} K   (Schwarzschild radius {:.4e} m = {:.3e} ly)",
        "Hawking temperature", t_h, r_s, r_s / (SPEED_OF_LIGHT * YEAR_S));

    // ---- (b) Density at last scattering, eq (57) --------------------------
    println!("\n(b) §IV eq (57) — rho_LS = rho_0 / a_LS^3");
    let rho_ls_si = paper::RHO_0 / paper::A_LS.powi(3);
    println!("  inputs: rho_0 = {:.3e} kg/m^3 (eq 84), a_LS = 1/{:.0} (eq 83)",
        paper::RHO_0, 1.0 / paper::A_LS);
    let dex_rho = check("rho_LS", rho_ls_si, paper::RHO_LS_CLAIMED, "kg/m^3");
    // A 1e-17 kg/m^3 value equals 1e-20 g/cm^3 — the mantissa is identical.
    println!("  {:<34} {:.4e} g/cm^3  <- same value in CGS; mantissa matches eq (57) exactly",
        "rho_LS expressed in g/cm^3", rho_ls_si * 1e-3);

    // ---- (c) Injected density from the prior aeon, eq (58) ----------------
    println!("\n(c) §IV eq (58) — delta_rho = 3 M_gc / (4 pi (eta^c_LS · c · a_LS)^3)");
    let radius = paper::ETA_C_LS * SPEED_OF_LIGHT * paper::A_LS;
    let delta_rho = 3.0 * m_gc / (4.0 * std::f64::consts::PI * radius.powi(3));
    println!("  physical radius R = eta^c_LS·c·a_LS = {:.4e} m ({:.3e} ly)",
        radius, radius / (SPEED_OF_LIGHT * YEAR_S));
    check("delta_rho", delta_rho, paper::DELTA_RHO_CLAIMED, "kg/m^3");

    // ---- (d) Temperature elevation, eq (59) -------------------------------
    // Radiation: rho ~ T^4  =>  delta_rho/rho = 4 delta_T/T.
    println!("\n(d) §IV eq (59) — delta_T/T = delta_rho / (4 rho_LS)");
    let dt_over_t_si = delta_rho / (4.0 * rho_ls_si);
    let dt_over_t_asprinted = delta_rho / (4.0 * paper::RHO_LS_CLAIMED);
    check("delta_T/T  [consistent SI]", dt_over_t_si, paper::DELTA_T_OVER_T_CLAIMED, "");
    check("delta_T/T  [eq-57 as printed]", dt_over_t_asprinted, paper::DELTA_T_OVER_T_CLAIMED, "");

    // ---- Summary ----------------------------------------------------------
    println!("\n=== SUMMARY ===");
    println!("  (a) lifetime  ~10^112 yr ......... REPRODUCED");
    println!("  (c) delta_rho ~10^-22 kg/m^3 ..... REPRODUCED (genuine SI: M in kg, R in m)");
    println!("  (b) rho_LS: recomputed value is {:+.2} dex from the printed eq (57).", dex_rho);
    println!("      rho_0/a_LS^3 = {:.3e} kg/m^3, but eq (57) prints {:.1e} kg/m^3.", rho_ls_si, paper::RHO_LS_CLAIMED);
    println!("      The printed mantissa (1.2) is exactly the CGS value => unit-label slip,");
    println!("      NOT an arithmetic error: 1.2e-20 g/cm^3 == 1.2e-17 kg/m^3.");
    println!("  (d) eq (59) divides a genuine-SI delta_rho by the CGS-valued rho_LS:");
    println!("      consistent SI  -> delta_T/T = {:.2e}", dt_over_t_si);
    println!("      as-printed mix -> delta_T/T = {:.2e}  (the paper's headline ~10^-3)", dt_over_t_asprinted);
    println!("      The two differ by exactly 10^3, the kg/m^3 <-> g/cm^3 factor.\n");
}
