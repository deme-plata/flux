//! flux-md — run a molecular dynamics simulation and grade it against the lab.
//!
//! Usage:
//!   flux-md                       # default: liquid argon, Rahman 1964 state point
//!   flux-md --cells 4 --prod 4000 # 256 atoms, longer production run
//!   flux-md --temp 120 --json     # different state point, machine-readable out
//!
//! The run ends with a VALIDATION block comparing computed observables against
//! published experimental values. A run that disagrees with the laboratory
//! prints FAIL and exits non-zero — this binary is allowed to say it is wrong.

use flux_md::observables::{
    diffusion_coefficient, msd, nm2_ps_to_cm2_s, rdf, reference, RdfAccumulator,
};
use flux_md::{Integrator, Species, System};

struct Args {
    cells: usize,
    temp: f64,
    density: f64,
    dt: f64,
    equil: usize,
    prod: usize,
    seed: u64,
    json: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            cells: 3, // 108 atoms
            temp: reference::ARGON_T_K,
            density: reference::ARGON_RHO_G_CM3,
            dt: 0.004, // 4 fs
            // The run starts from a perfect FCC lattice which must fully MELT
            // before production. Under-equilibrating leaves the system still
            // converting potential into kinetic energy, so the NVE temperature
            // climbs off target (94 K -> 103 K at 3 000 steps) and the run
            // correctly fails its own temperature check. 20 000 steps (80 ps)
            // reaches a stable liquid.
            equil: 20_000,
            prod: 6_000,
            seed: 20260725,
            json: false,
        }
    }
}

fn parse_args() -> Args {
    let mut a = Args::default();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let mut next = |i: &mut usize| -> Option<String> {
            *i += 1;
            argv.get(*i).cloned()
        };
        match argv[i].as_str() {
            "--cells" => a.cells = next(&mut i).and_then(|v| v.parse().ok()).unwrap_or(a.cells),
            "--temp" => a.temp = next(&mut i).and_then(|v| v.parse().ok()).unwrap_or(a.temp),
            "--density" => {
                a.density = next(&mut i).and_then(|v| v.parse().ok()).unwrap_or(a.density)
            }
            "--dt" => a.dt = next(&mut i).and_then(|v| v.parse().ok()).unwrap_or(a.dt),
            "--equil" => a.equil = next(&mut i).and_then(|v| v.parse().ok()).unwrap_or(a.equil),
            "--prod" => a.prod = next(&mut i).and_then(|v| v.parse().ok()).unwrap_or(a.prod),
            "--seed" => a.seed = next(&mut i).and_then(|v| v.parse().ok()).unwrap_or(a.seed),
            "--json" => a.json = true,
            "--help" | "-h" => {
                println!("flux-md — molecular dynamics with laboratory validation\n");
                println!("  --cells N     FCC cells per side (atoms = 4N³) [default 3 = 108]");
                println!("  --temp K      target temperature [94.4]");
                println!("  --density G   g/cm³ [1.374]");
                println!("  --dt PS       timestep in ps [0.004]");
                println!("  --equil N     equilibration steps [20000] (lattice must melt)");
                println!("  --prod N      production steps (NVE) [6000]");
                println!("  --seed N      PRNG seed (runs are reproducible) [20260725]");
                println!("  --json        machine-readable output");
                std::process::exit(0);
            }
            _ => {}
        }
        i += 1;
    }
    a
}

fn main() {
    let args = parse_args();
    let sp = Species::argon();
    let mut sys = System::fcc_lattice(sp, args.cells, args.density, args.seed);
    sys.set_temperature(args.temp, args.seed);
    let integ = Integrator::new(args.dt);

    if !args.json {
        println!("flux-md :: liquid {} — molecular dynamics with real physics", sp.name);
        println!("  atoms        {}", sys.n());
        println!("  box          {:.4} nm  (density {:.4} g/cm3)", sys.box_len, sys.mass_density());
        println!("  cutoff       {:.4} nm", sys.cutoff);
        println!("  timestep     {:.1} fs", args.dt * 1000.0);
        println!("  target T     {:.2} K", args.temp);
        println!("  seed         {} (trajectory is reproducible)\n", args.seed);
        println!("  {:>8}  {:>9}  {:>12}  {:>12}  {:>12}", "step", "T (K)", "E_kin", "E_pot", "E_tot");
    }

    // ---- equilibration: rescaling thermostat, NOT an ensemble sampler ------
    for s in 0..args.equil {
        integ.step(&mut sys);
        sys.rescale_to(args.temp, 0.02);
        if !args.json && s % (args.equil / 4).max(1) == 0 {
            println!(
                "  {:>8}  {:>9.2}  {:>12.2}  {:>12.2}  {:>12.2}   [equil]",
                s,
                sys.temperature(),
                sys.kinetic(),
                sys.potential,
                sys.total_energy()
            );
        }
    }
    sys.remove_com_motion();

    // ---- production: pure NVE, so energy drift is a real diagnostic --------
    let e_start = sys.total_energy();
    let t_start_ref: Vec<_> = sys.pos_unwrapped.clone();
    let mut rdf_acc = RdfAccumulator::new(200, sys.cutoff);
    let mut msd_samples: Vec<(f64, f64)> = Vec::new();
    let mut t_sum = 0.0;
    let mut t_count = 0usize;

    for s in 0..args.prod {
        integ.step(&mut sys);
        t_sum += sys.temperature();
        t_count += 1;
        if s % 20 == 0 {
            rdf_acc.sample(&sys);
        }
        // Skip the early ballistic regime before fitting Einstein's relation.
        if s > args.prod / 5 && s % 10 == 0 {
            let t_ps = s as f64 * args.dt;
            msd_samples.push((t_ps, msd(&t_start_ref, &sys.pos_unwrapped)));
        }
        if !args.json && s % (args.prod / 4).max(1) == 0 {
            println!(
                "  {:>8}  {:>9.2}  {:>12.2}  {:>12.2}  {:>12.2}   [NVE]",
                s,
                sys.temperature(),
                sys.kinetic(),
                sys.potential,
                sys.total_energy()
            );
        }
    }

    // ---- observables --------------------------------------------------------
    let e_end = sys.total_energy();
    let drift = (e_end - e_start).abs() / e_start.abs();
    let t_mean = t_sum / t_count.max(1) as f64;
    let d_nm2ps = diffusion_coefficient(&msd_samples);
    let d_cm2s = nm2_ps_to_cm2_s(d_nm2ps);
    let g = rdf(&rdf_acc, &sys);
    let (peak_r, peak_g) = g
        .iter()
        .fold((0.0, 0.0), |a, &(r, v)| if v > a.1 { (r, v) } else { a });

    // ---- validation against published experiment ----------------------------
    let d_ratio = d_cm2s / reference::ARGON_D_CM2_S;
    let peak_err = (peak_r - reference::ARGON_RDF_FIRST_PEAK_NM).abs();
    let ok_drift = drift < 1e-3;
    let ok_temp = (t_mean - args.temp).abs() / args.temp < 0.05;
    let ok_peak = peak_err < 0.04;
    // Diffusion from a short run of ~100 atoms is only good to a factor of ~2;
    // claiming better would be dishonest, so the gate is stated as such.
    let ok_diff = (0.4..=2.5).contains(&d_ratio);
    let all_ok = ok_drift && ok_temp && ok_peak && ok_diff;

    if args.json {
        println!(
            "{{\"atoms\":{},\"box_nm\":{:.6},\"density_g_cm3\":{:.6},\"T_mean_K\":{:.4},\
             \"energy_drift\":{:.3e},\"D_cm2_s\":{:.4e},\"D_ref_cm2_s\":{:.4e},\"D_ratio\":{:.4},\
             \"rdf_peak_nm\":{:.4},\"rdf_peak_g\":{:.4},\"pass\":{}}}",
            sys.n(),
            sys.box_len,
            sys.mass_density(),
            t_mean,
            drift,
            d_cm2s,
            reference::ARGON_D_CM2_S,
            d_ratio,
            peak_r,
            peak_g,
            all_ok
        );
    } else {
        let mark = |b: bool| if b { "PASS" } else { "FAIL" };
        println!("\n  ---- validation against published experiment ----------------");
        println!("  Rahman, Phys. Rev. 136 A405 (1964); liquid Ar at 94.4 K, 1.374 g/cm3\n");
        println!(
            "  {:<28} {:>12}  {:>14}   {}",
            "observable", "computed", "experiment", "verdict"
        );
        println!(
            "  {:<28} {:>12.4} {:>14.4}   {}",
            "mean temperature (K)",
            t_mean,
            args.temp,
            mark(ok_temp)
        );
        println!(
            "  {:<28} {:>12.2e} {:>14}   {}",
            "NVE energy drift (rel.)",
            drift,
            "< 1e-3",
            mark(ok_drift)
        );
        println!(
            "  {:<28} {:>12.4} {:>14.4}   {}",
            "first g(r) peak (nm)",
            peak_r,
            reference::ARGON_RDF_FIRST_PEAK_NM,
            mark(ok_peak)
        );
        println!(
            "  {:<28} {:>12.3e} {:>14.3e}   {}  ({:.2}x)",
            "self-diffusion (cm2/s)",
            d_cm2s,
            reference::ARGON_D_CM2_S,
            mark(ok_diff),
            d_ratio
        );
        println!("\n  peak height g_max = {peak_g:.2} (a structured liquid, not a gas)");
        println!(
            "\n  {}",
            if all_ok {
                "ALL CHECKS PASSED — this simulation reproduces measured argon."
            } else {
                "CHECKS FAILED — the simulation disagrees with the laboratory."
            }
        );
        println!("  Re-run with the same --seed to reproduce these numbers exactly.");
    }

    if !all_ok {
        std::process::exit(1);
    }
}
