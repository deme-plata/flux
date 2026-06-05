//! Small deterministic helpers shared across the sim. No external crates — the
//! whole game is reproducible from a seed so tests are stable and the browser /
//! terminal front-ends can replay an identical weekend.
//!
//! Uses **splitmix64** rather than a single xorshift step: splitmix avalanches
//! every call, so even sequential seeds (0, 1, 2, …) at a fixed track position
//! produce well-distributed, decorrelated draws. (A bare xorshift step leaves
//! low-bit-only seed differences in the low bits, clustering the output.)

const GOLDEN: u64 = 0x9E3779B97F4A7C15;

/// One splitmix64 step: advance `state` and return a thoroughly-mixed u64.
pub fn xorshift(state: &mut u64) -> u64 {
    *state = state.wrapping_add(GOLDEN);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

/// Next pseudo-random float in 0.0..1.0.
pub fn rand01(state: &mut u64) -> f64 {
    (xorshift(state) >> 11) as f64 / (1u64 << 53) as f64
}

/// Mix a seed with a salt so independent streams (per round / per lap / per
/// corner) decorrelate while staying fully deterministic. Runs the combined
/// value through one splitmix step so the *initial* state is already avalanched.
pub fn salt(seed: u64, salt: u64) -> u64 {
    let mut z = seed
        .wrapping_mul(GOLDEN)
        ^ salt.wrapping_mul(0xD1B54A32D192ED03).rotate_left(32);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rand01_is_well_spread_for_sequential_seeds() {
        // Regression: a fixed "lap/corner" salt with sequential seeds must still
        // spread across the unit interval, not cluster on one side.
        let mut below = 0;
        let mut above = 0;
        for seed in 0..2000u64 {
            let mut st = salt(seed, 0x109); // fixed position, varying seed
            let r = rand01(&mut st);
            if r < 0.5 { below += 1 } else { above += 1 }
        }
        // Expect roughly 50/50 — certainly not all on one side.
        assert!(below > 700 && above > 700, "rng clustered: below={below} above={above}");
    }

    #[test]
    fn deterministic() {
        let mut a = salt(42, 7);
        let mut b = salt(42, 7);
        assert_eq!(rand01(&mut a), rand01(&mut b));
    }
}
