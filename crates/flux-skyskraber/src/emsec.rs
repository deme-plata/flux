//! # emsec — the tower's Emanations Security posture, scored from the blueprint
//!
//! OK, so here's the deal. The other organs measure whether the building *runs*
//! well: do the lifts flow, does payroll clear, is the gold witnessed. This one
//! measures something the building can pass every one of those checks and still
//! fail at: whether its **secrets stay inside its walls**.
//!
//! It is the [`crate::tower`] blueprint read through the SIGIL Nation EMSEC
//! Doctrine v0 — the four principles distilled from the TEMPEST handbook
//! (NSTISSAM TEMPEST/2-91):
//!
//! * **P1 — RED/BLACK separation.** The RED zones (custody + the signing bank
//!   organ) must not sit skin-to-skin with BLACK zones (leased offices, the
//!   public lobby) with no GRAY buffer between them.
//! * **P2 — inspectable space.** Security is bought in *metres*: how close can
//!   an adversary stand to the leak. Below-grade RED is earth-shielded; RED that
//!   faces a facade at a short standoff relies on shielding it may not have.
//! * **P3 — signal averaging.** Repetition is the enemy. A perfectly periodic
//!   public reference — the light column pulsing once per block — lets an
//!   eavesdropper phase-lock and average *other* emanations down to certainty.
//!   The residual error is the handbook's own majority-vote binomial,
//!   [`averaging_residual`], evaluated here for real.
//! * **P4 — fail-closed.** An emanation attack leaves no trace, so you cannot
//!   detect it, only prevent it and *verify* the prevention. Controls that shout
//!   and refuse to run in an unsafe state, counted honestly — including the two
//!   that are coded but **not yet active** on the live chain.
//!
//! Like [`crate::culture`], the posture refuses self-report: every component is
//! derived from the blueprint and a small, explicit config of what has actually
//! been built. It cannot be flattered — only hardened.

use crate::tower::{TowerSpec, Zone};
use serde::{Deserialize, Serialize};

/// The three emanation zones. RED carries the secret in the clear; BLACK is
/// public/encrypted; GRAY is the controlled buffer (and the inspectable space)
/// that must lie between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecurityZone {
    Red,
    Gray,
    Black,
}

/// Map a building zone to its emanation class. The vault (custody) and the bank
/// hall (the signing ledger organ) are RED. The robot bays, mechanical and
/// refuge decks are the GRAY infrastructure buffer. Everything a stranger can
/// occupy — lobby, offices, auditorium, sky gardens — is BLACK.
pub fn classify(zone: Zone) -> SecurityZone {
    match zone {
        Zone::GoldVault | Zone::BankHall => SecurityZone::Red,
        Zone::RobotBay | Zone::Mechanical | Zone::SkyBridge => SecurityZone::Gray,
        Zone::Lobby | Zone::Offices | Zone::Auditorium | Zone::SkyGarden => SecurityZone::Black,
    }
}

/// What has actually been built, versus what the doctrine wants. The posture is
/// blueprint × this config — so hardening is visible as a number, not a claim.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmsecConfig {
    /// Distance (m) from the nearest above-ground RED floor to where an
    /// adversary can stand. The doctrine target is ≥ 30 m (P2).
    pub standoff_m: f64,
    /// Is the signing key in an HSM with RED/BLACK power+cabling isolation, or
    /// are keys software-held and sharing infrastructure with BLACK? (P1)
    pub hsm_red_black_isolated: bool,
    /// Is the light-column beacon jittered / decoupled from block cadence, or a
    /// raw once-per-block pulse an attacker can phase-lock to? (P3)
    pub beacon_masked: bool,
    /// How many times the beacon fires per day (once per block). Live SIGIL
    /// cadence ≈ 0.83 blk/s ⇒ ~71.7k pulses/day of averaging reference.
    pub beacon_pulses_per_day: u64,
    /// Adversary's per-pulse bit-error Q. 0.49 = a leak that *looks like* pure
    /// noise in a single shot — and still averages to certainty over the day.
    pub eavesdropper_bit_error: f64,
    /// Producer-signature verification actually switched on? (coded, not active
    /// on the live chain — P1/P4)
    pub producer_sig_active: bool,
    /// Cross-node integrity actually being verified at the tip? (heartbeat says
    /// "NOT VERIFYING" today — P4)
    pub crossnode_integrity_verified: bool,
}

impl EmsecConfig {
    /// The tower **as it stands today** — the honest, unflattering baseline:
    /// software keys, an unmasked per-block beacon, an urban ~18 m plaza
    /// standoff, and the two coded-but-inactive integrity controls.
    pub fn doctrine_v0() -> Self {
        Self {
            standoff_m: 18.0,
            hsm_red_black_isolated: false,
            beacon_masked: false,
            beacon_pulses_per_day: 71_700,
            eavesdropper_bit_error: 0.49,
            producer_sig_active: false,
            crossnode_integrity_verified: false,
        }
    }

    /// The tower **hardened to doctrine v0**: HSM RED/BLACK isolation, a masked
    /// beacon, ≥ 30 m standoff, and both integrity controls switched on.
    pub fn doctrine_v0_hardened() -> Self {
        Self {
            standoff_m: 30.0,
            hsm_red_black_isolated: true,
            beacon_masked: true,
            beacon_pulses_per_day: 71_700,
            eavesdropper_bit_error: 0.49,
            producer_sig_active: true,
            crossnode_integrity_verified: true,
        }
    }
}

/// The four principles, each scored 0–1.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmsecComponents {
    /// P1 — RED zones buffered from BLACK, and keys in isolated hardware.
    pub red_black_separation: f64,
    /// P2 — RED shielded by earth or bought distance.
    pub inspectable_space: f64,
    /// P3 — resistance to signal averaging of a periodic reference.
    pub averaging_resistance: f64,
    /// P4 — fraction of fail-closed controls actually active.
    pub fail_closed: f64,
}

/// The tower's emanation-security posture: one number, its four parts, a plain
/// verdict, and the specific findings a red-pen review would write in the margin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmsecPosture {
    pub total: f64,
    pub components: EmsecComponents,
    pub verdict: String,
    pub findings: Vec<String>,
}

fn clamp01(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

/// Lanczos ln Γ(x) — g=7, 9 coefficients. Accurate to ~1e-13 for x ≥ 0.5,
/// which is all we ask of it (binomial coefficients over positive integers).
fn ln_gamma(x: f64) -> f64 {
    const G: f64 = 7.0;
    const C: [f64; 9] = [
        0.999_999_999_999_809_93,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_13,
        -176.615_029_162_140_59,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];
    if x < 0.5 {
        // Reflection formula for the left half-plane.
        let pi = std::f64::consts::PI;
        pi.ln() - (pi * x).sin().abs().ln() - ln_gamma(1.0 - x)
    } else {
        let x = x - 1.0;
        let t = x + G + 0.5;
        let mut a = C[0];
        for (i, &c) in C.iter().enumerate().skip(1) {
            a += c / (x + i as f64);
        }
        0.5 * (2.0 * std::f64::consts::PI).ln() + (x + 0.5) * t.ln() - t + a.ln()
    }
}

/// The handbook's result, evaluated exactly (§3-5): if the same bit is opsnapt
/// N times over a symmetric channel with per-shot error `q`, majority-voting the
/// N copies leaves the attacker with this residual error probability.
///
/// It is the upper binomial tail P(X ≥ ⌊N/2⌋+1) for X ~ Binom(N, q), plus half
/// the tie mass when N is even. `q → 0.5` is a fair coin (residual → 0.5); any
/// `q < 0.5` collapses to 0 as N grows — which is exactly why repetition is the
/// enemy. Computed in log-space so N in the tens of thousands does not overflow.
pub fn averaging_residual(q: f64, n: u64) -> f64 {
    if q <= 0.0 {
        return 0.0;
    }
    if q >= 1.0 {
        return 1.0;
    }
    if n == 0 {
        return 0.5;
    }
    let nf = n as f64;
    let lq = q.ln();
    let lp = (1.0 - q).ln();
    let lgn = ln_gamma(nf + 1.0);
    let lterm = |i: u64| -> f64 {
        lgn - ln_gamma(i as f64 + 1.0) - ln_gamma((n - i) as f64 + 1.0)
            + i as f64 * lq
            + (n - i) as f64 * lp
    };
    // Smallest integer strictly greater than N/2 — the count that wins the vote.
    let m = n / 2 + 1;
    let mut max_l = f64::NEG_INFINITY;
    for i in m..=n {
        let l = lterm(i);
        if l > max_l {
            max_l = l;
        }
    }
    let mut acc = 0.0;
    for i in m..=n {
        acc += (lterm(i) - max_l).exp();
    }
    let mut residual = max_l.exp() * acc;
    if n % 2 == 0 {
        // Even N: a tie at exactly N/2 is resolved by a coin flip.
        residual += 0.5 * lterm(n / 2).exp();
    }
    residual.min(1.0)
}

/// Every distinct level's set of emanation classes (twin spires share a level).
fn classes_at(spec: &TowerSpec, level: i32) -> Vec<SecurityZone> {
    let mut v: Vec<SecurityZone> = spec
        .floors
        .iter()
        .filter(|f| f.level == level)
        .map(|f| classify(f.zone))
        .collect();
    v.sort_by_key(|z| *z as u8);
    v.dedup();
    v
}

/// Score the tower's emanation-security posture: blueprint × what's been built.
pub fn assess(spec: &TowerSpec, cfg: &EmsecConfig) -> EmsecPosture {
    let mut findings: Vec<String> = Vec::new();

    // Distinct RED levels (the vault plates and the bank-hall plates).
    let mut red_levels: Vec<i32> = spec
        .floors
        .iter()
        .filter(|f| classify(f.zone) == SecurityZone::Red)
        .map(|f| f.level)
        .collect();
    red_levels.sort();
    red_levels.dedup();
    let red_total = red_levels.len().max(1) as f64;

    // ── P1: RED/BLACK separation ────────────────────────────────────────────
    // A RED floor is "shielded" if it is below grade (earth) OR neither vertical
    // neighbour is BLACK. A RED face onto BLACK with no GRAY buffer is a leak
    // path straight out of the handbook.
    let mut shielded = 0.0;
    for &l in &red_levels {
        let below_grade = l < 0;
        let neighbour_black = classes_at(spec, l - 1).contains(&SecurityZone::Black)
            || classes_at(spec, l + 1).contains(&SecurityZone::Black);
        if below_grade || !neighbour_black {
            shielded += 1.0;
        } else {
            findings.push(format!(
                "P1: RED floor L{l} ({}) presents a bare face to a BLACK zone with no GRAY buffer — add a buffer deck or move custody below grade",
                zone_name_at(spec, l)
            ));
        }
    }
    let geometry = shielded / red_total;
    let red_black_separation = geometry * if cfg.hsm_red_black_isolated { 1.0 } else { 0.7 };
    if !cfg.hsm_red_black_isolated {
        findings.push(
            "P1: signing keys are software-held (no HSM RED/BLACK power+cabling isolation) — the key can modulate onto shared infrastructure".into(),
        );
    }

    // ── P2: inspectable space ────────────────────────────────────────────────
    // Below-grade RED is earth-shielded (full credit). Above-grade RED lives or
    // dies by standoff; a waterfront edge buys distance an antenna cannot cross.
    let below_grade_red = red_levels.iter().filter(|&&l| l < 0).count() as f64;
    let above_grade_red = red_total - below_grade_red;
    let mut above_credit = clamp01(cfg.standoff_m / 30.0);
    if spec.waterfront {
        above_credit = clamp01(above_credit + 0.2);
    }
    let inspectable_space = (below_grade_red + above_grade_red * above_credit) / red_total;
    if above_grade_red > 0.0 && cfg.standoff_m < 30.0 {
        findings.push(format!(
            "P2: {above_grade_red:.0} RED floor(s) sit above ground at {:.0} m standoff (target ≥ 30 m) — they depend on shielding, not distance",
            cfg.standoff_m
        ));
    }

    // ── P3: signal-averaging resistance ──────────────────────────────────────
    // With no beacon there is nothing periodic to phase-lock to (full credit).
    // A masked beacon defeats averaging (effective N = 1). A raw per-block pulse
    // hands the attacker `beacon_pulses_per_day` reference cycles — the residual
    // collapses toward zero and the day's other emanations average out with it.
    let q = cfg.eavesdropper_bit_error;
    let single_shot = averaging_residual(q, 1); // = q, the no-averaging baseline
    let averaging_resistance = if !spec.facade.light_column {
        1.0
    } else if cfg.beacon_masked {
        1.0
    } else {
        let residual = averaging_residual(q, cfg.beacon_pulses_per_day);
        let r = if single_shot > 0.0 { residual / single_shot } else { 0.0 };
        findings.push(format!(
            "P3: the light column fires ~{} unmasked pulses/day — a periodic public reference; attacker residual error collapses {:.2}→{:.1e}, i.e. averaging wins. Jitter the beacon or decouple it from block cadence",
            cfg.beacon_pulses_per_day, single_shot, residual
        ));
        clamp01(r)
    };

    // ── P4: fail-closed controls ─────────────────────────────────────────────
    // Counted honestly. Four are built; two are coded-but-inactive on the live
    // chain and cost real posture until switched on.
    let controls: [(bool, &str); 6] = [
        (true, "vault refuses withdrawal without clearance"),
        (true, "blueprint validate() red-pen rejects a bad tower"),
        (true, "node refuses to start below the disk floor"),
        (true, "integrity heartbeat shouts when it cannot verify"),
        (cfg.producer_sig_active, "producer-signature verification active"),
        (cfg.crossnode_integrity_verified, "cross-node integrity verified at the tip"),
    ];
    let active = controls.iter().filter(|(on, _)| *on).count() as f64;
    let fail_closed = active / controls.len() as f64;
    for (on, name) in controls.iter() {
        if !*on {
            findings.push(format!("P4: fail-closed control INACTIVE — {name}"));
        }
    }

    let components = EmsecComponents {
        red_black_separation,
        inspectable_space,
        averaging_resistance,
        fail_closed,
    };
    let total = (components.red_black_separation
        + components.inspectable_space
        + components.averaging_resistance
        + components.fail_closed)
        / 4.0;
    let verdict = match total {
        t if t >= 0.9 => "hardened — emanations posture inside doctrine",
        t if t >= 0.75 => "defensible, with named gaps",
        t if t >= 0.5 => "porous — posture debt accruing",
        _ => "the tower stands; its secrets do not",
    }
    .to_string();

    EmsecPosture { total, components, verdict, findings }
}

fn zone_name_at(spec: &TowerSpec, level: i32) -> &'static str {
    match spec.floors.iter().find(|f| f.level == level).map(|f| f.zone) {
        Some(Zone::GoldVault) => "GoldVault",
        Some(Zone::BankHall) => "BankHall",
        Some(_) | None => "RED",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tower::TowerSpec;

    #[test]
    fn averaging_residual_matches_the_handbook() {
        // No averaging: residual is just the single-shot error.
        assert!((averaging_residual(0.49, 1) - 0.49).abs() < 1e-9);
        // A fair-coin leak stays a fair coin no matter how many copies.
        assert!((averaging_residual(0.5, 10_001) - 0.5).abs() < 1e-6);
        // Repetition is the enemy: monotone collapse toward 0 as N grows.
        let a = averaging_residual(0.49, 101);
        let b = averaging_residual(0.49, 1_001);
        let c = averaging_residual(0.49, 10_001);
        assert!(a > b && b > c, "a={a} b={b} c={c}");
        assert!(c < 0.05, "c={c}"); // the 2.3% row from the doctrine table
        // Q=0.45 collapses far faster.
        assert!(averaging_residual(0.45, 1_001) < 1e-3);
    }

    #[test]
    fn canonical_tower_is_porous_by_default() {
        let spec = TowerSpec::quillon_default();
        let p = assess(&spec, &EmsecConfig::doctrine_v0());
        // The unmasked beacon alone should drag averaging_resistance to ~0.
        assert!(p.components.averaging_resistance < 0.01, "avg={}", p.components.averaging_resistance);
        assert!(p.total < 0.6, "total={}", p.total);
        assert!(p.verdict.contains("porous") || p.verdict.contains("secrets do not"));
        assert!(!p.findings.is_empty());
    }

    #[test]
    fn hardening_strictly_raises_the_posture() {
        let spec = TowerSpec::quillon_default();
        let v0 = assess(&spec, &EmsecConfig::doctrine_v0()).total;
        let hard = assess(&spec, &EmsecConfig::doctrine_v0_hardened());
        assert!(hard.total > v0 + 0.3, "v0={v0} hard={}", hard.total);
        assert!(hard.total >= 0.9, "hardened total={}", hard.total);
        // Masking the beacon fully restores P3.
        assert!((hard.components.averaging_resistance - 1.0).abs() < 1e-9);
    }

    #[test]
    fn below_grade_vault_beats_above_grade_bank_hall() {
        let spec = TowerSpec::quillon_default();
        // The vault (below grade) is shielded; the bank hall faces the lobby and
        // the auditorium, so P1 geometry cannot be perfect even with an HSM.
        let hard = assess(&spec, &EmsecConfig::doctrine_v0_hardened());
        assert!(hard.components.red_black_separation < 1.0);
        assert!(hard.components.red_black_separation > 0.6);
        // A residual finding must name the architectural (not config) gap.
        assert!(hard.findings.iter().any(|f| f.contains("BankHall")));
    }

    #[test]
    fn guardian_ring_plus_hardened_config_is_perfect() {
        let spec = TowerSpec::quillon_hardened();
        let p = assess(&spec, &EmsecConfig::doctrine_v0_hardened());
        assert!((p.total - 1.0).abs() < 1e-9, "total={}", p.total);
        assert!((p.components.red_black_separation - 1.0).abs() < 1e-9);
        assert!((p.components.inspectable_space - 1.0).abs() < 1e-9);
        assert!((p.components.averaging_resistance - 1.0).abs() < 1e-9);
        assert!((p.components.fail_closed - 1.0).abs() < 1e-9);
        assert!(p.findings.is_empty(), "a perfect posture leaves no findings: {:?}", p.findings);
        assert!(p.verdict.contains("hardened"));
    }

    #[test]
    fn guardian_ring_closes_the_p1_geometry_gap_the_config_cannot() {
        // The canonical tower cannot reach P1 = 1.0 even fully config-hardened —
        // the bank hall touches BLACK. The guardian ring is what closes it.
        let canonical = assess(&TowerSpec::quillon_default(), &EmsecConfig::doctrine_v0_hardened());
        let guardian = assess(&TowerSpec::quillon_hardened(), &EmsecConfig::doctrine_v0_hardened());
        assert!(canonical.components.red_black_separation < 1.0);
        assert!((guardian.components.red_black_separation - 1.0).abs() < 1e-9);
    }

    #[test]
    fn classification_is_total_and_sane() {
        assert_eq!(classify(Zone::GoldVault), SecurityZone::Red);
        assert_eq!(classify(Zone::BankHall), SecurityZone::Red);
        assert_eq!(classify(Zone::Offices), SecurityZone::Black);
        assert_eq!(classify(Zone::RobotBay), SecurityZone::Gray);
    }
}
