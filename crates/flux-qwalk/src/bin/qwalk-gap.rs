//! qwalk-gap — measure the classical spectral-gap exponent `ν` and put it
//! side-by-side with the values published in arXiv:2607.22818, Fig. 6.
//!
//! Everything printed is computed here, now, from a seed. Nothing is quoted
//! from the paper except the reference column, which is labelled as such.
//!
//! ```text
//!   QWALK_INSTANCES=6 QWALK_NMAX=10 qwalk-gap
//! ```

use flux_qwalk::{
    absolute_spectral_gap, discriminant, fit_nu, uniform_gap_low_temperature_limit, Pcg32,
    Proposal, SkInstance,
};

/// Fitted exponents reported by the paper (Fig. 6), for the reference column.
/// (beta, local, uniform, hamiltonian) — the Hamiltonian column is quantum and
/// is shown only to make clear which number this crate cannot check.
const PAPER: &[(f64, f64, f64, f64)] = &[
    (1.0, 0.468, 0.802, 0.241),
    (2.0, 0.723, 0.924, 0.300),
    (4.0, 1.174, 0.968, 0.329),
    (10.0, 2.646, 0.991, 0.340),
    (20.0, 4.371, 0.999, 0.339),
];

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn main() {
    let instances = env_usize("QWALK_INSTANCES", 6);
    let n_min = env_usize("QWALK_NMIN", 5);
    let n_max = env_usize("QWALK_NMAX", 10);
    let seed = env_usize("QWALK_SEED", 20260728) as u64;

    println!("flux-qwalk — independent replication of the CLASSICAL baseline");
    println!("paper: arXiv:2607.22818 (Incudini & Mazzola, 2026-07-28)");
    println!(
        "config: n = {n_min}..={n_max}, {instances} SK instances/size, seed {seed}, \
         exact diagonalization\n"
    );

    // (proposal, beta) -> Vec<(n, arithmetic mean gap, geometric mean gap)>
    let mut table: Vec<(Proposal, f64, Vec<(usize, f64, f64)>)> = Vec::new();
    for prop in [Proposal::Local, Proposal::Uniform] {
        for (beta, _, _, _) in PAPER {
            table.push((prop, *beta, Vec::new()));
        }
    }

    for n in n_min..=n_max {
        // One RNG stream per size: instance k at size n is the same instance
        // for every beta and both proposals, so the columns are comparable.
        let mut rng = Pcg32::new(seed.wrapping_add(n as u64));
        let mut sums: Vec<(f64, f64)> = vec![(0.0, 0.0); table.len()];

        for _ in 0..instances {
            let inst = SkInstance::sample(n, &mut rng);
            let energies = inst.energies();
            for (slot, (prop, beta, _)) in table.iter().enumerate() {
                let d = discriminant(&inst, &energies, *beta, *prop);
                let g = absolute_spectral_gap(&d);
                sums[slot].0 += g;
                sums[slot].1 += g.log2();
            }
        }

        let m = instances as f64;
        for (slot, entry) in table.iter_mut().enumerate() {
            entry.2.push((n, sums[slot].0 / m, (sums[slot].1 / m).exp2()));
        }
        println!("  n = {n:>2}  ({} configurations)  done", 1usize << n);
    }

    println!("\n{}", "=".repeat(78));
    println!("FITTED GAP EXPONENT  nu   in   delta(n) = C * 2^(-nu*n),  fit over n={n_min}..={n_max}");
    println!("{}", "=".repeat(78));
    println!(
        "{:<9} {:<10} {:>12} {:>12} {:>12} {:>10}",
        "proposal", "beta", "nu (arith)", "nu (geom)", "paper nu", "delta"
    );
    println!("{}", "-".repeat(78));

    for (prop, beta, pts) in &table {
        let arith: Vec<(usize, f64)> = pts.iter().map(|(n, a, _)| (*n, *a)).collect();
        let geo: Vec<(usize, f64)> = pts.iter().map(|(n, _, g)| (*n, *g)).collect();
        let (_, nu_a) = fit_nu(&arith);
        let (_, nu_g) = fit_nu(&geo);

        let row = PAPER.iter().find(|(b, _, _, _)| (b - beta).abs() < 1e-9).unwrap();
        let paper_nu = match prop {
            Proposal::Local => row.1,
            Proposal::Uniform => row.2,
        };
        let name = match prop {
            Proposal::Local => "local",
            Proposal::Uniform => "uniform",
        };
        println!(
            "{:<9} {:<10} {:>12.3} {:>12.3} {:>12.3} {:>10.3}",
            name,
            beta,
            nu_a,
            nu_g,
            paper_nu,
            nu_g - paper_nu
        );
    }

    println!("\n{}", "=".repeat(78));
    println!("ANALYTIC ANCHOR — uniform proposal, low temperature");
    println!("{}", "=".repeat(78));
    println!(
        "At low T the independence sampler must PROPOSE a specific configuration\n\
         out of 2^n to leave the ground state, so delta -> 2^-n and nu -> 1 exactly.\n\
         The paper's fitted uniform nu marches 0.968 -> 0.991 -> 0.999 as beta grows.\n"
    );
    println!("{:<6} {:>16} {:>16} {:>10}", "n", "measured (b=20)", "2^-n", "ratio");
    println!("{}", "-".repeat(52));
    let slot = table
        .iter()
        .position(|(p, b, _)| *p == Proposal::Uniform && (*b - 20.0).abs() < 1e-9)
        .expect("uniform beta=20 row");
    for (n, _, geo) in &table[slot].2 {
        let lim = uniform_gap_low_temperature_limit(*n);
        println!("{:<6} {:>16.3e} {:>16.3e} {:>10.3}", n, geo, lim, geo / lim);
    }

    println!("\n{}", "=".repeat(78));
    println!("WHAT THIS DOES NOT CHECK");
    println!("{}", "=".repeat(78));
    println!(
        "The Hamiltonian-simulation proposal (paper nu ~ 0.33) is quantum: nothing\n\
         here touches it. The sixth-degree headline is nu_classical / (nu_quantum/2);\n\
         this run measures only the numerator. The fault-tolerant gate-count model\n\
         behind the 'less than one day' crossover is likewise untouched.\n\
         Note also that both the paper and this run fit over n=5..10 and extrapolate\n\
         to n=50 — an 8^n dense diagonalization admits no larger honest window."
    );
}
