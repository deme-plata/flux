//! `skyskraber-whitepaper` — the whitepaper generator.
//!
//! The flux-arxiv discipline applied to this crate's own paper: run the
//! deterministic day, fold the state, recompute the physics from CODATA
//! constants, substitute every `@@KEY@@` in the template, and REFUSE to emit
//! if any placeholder is left unfilled. No pasted figures.
//!
//!   skyskraber-whitepaper [output.tex]     (default: skyskraber-whitepaper.tex)

use flux_skyskraber::elevator::{leg_time_s, CarKind};
use flux_skyskraber::physics;
use flux_skyskraber::science;
use flux_skyskraber::sim;
use flux_skyskraber::tower::{Spire, TowerSpec, Zone};

const TEMPLATE: &str = include_str!("../../whitepaper/template.tex");

/// 147500 → "147{,}500" (LaTeX thousands separators).
fn thou(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push_str("{,}");
        }
        out.push(ch);
    }
    out
}

/// 1.4735e-22 → "1.47\times 10^{-22}".
fn sci(x: f64, decimals: usize) -> String {
    let e = x.abs().log10().floor() as i32;
    let m = x / 10f64.powi(e);
    format!("{m:.decimals$}\\times 10^{{{e}}}")
}

/// Today as YYYY-MM-DD from the system clock (civil-from-days, no deps).
fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after 1970")
        .as_secs();
    let mut z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    z -= era * 146_097;
    let doe = z;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

fn main() {
    let out_path = std::env::args().nth(1).unwrap_or_else(|| "skyskraber-whitepaper.tex".into());

    eprintln!("▸ living the deterministic day (seed 42)…");
    let day = sim::run_day(42);
    let spec = TowerSpec::quillon_default();
    let bridge = physics::bridge_movement(&spec).expect("twin spires have a bridge");
    let evac = physics::evacuation(&spec);
    let annex = science::annex(&spec);

    // Areas, summed live from the spec — the paper's GFA equation is real.
    let area = |f: &dyn Fn(&flux_skyskraber::tower::Floor) -> bool| -> u64 {
        spec.floors.iter().filter(|fl| f(fl)).map(|fl| fl.area_m2 as u64).sum()
    };
    let a_base = area(&|f| f.level < 0);
    let a_lobby = area(&|f| f.zone == Zone::Lobby);
    let a_bank = area(&|f| f.zone == Zone::BankHall);
    let a_aud = area(&|f| f.zone == Zone::Auditorium);
    let a_body = area(&|f| f.level >= 9 && f.level <= 45 && f.spire.is_none()
        && matches!(f.zone, Zone::Offices | Zone::Mechanical));
    let a_spire_a = area(&|f| f.spire == Some(Spire::A));
    let a_spire_b = area(&|f| f.spire == Some(Spire::B));
    let a_bridge = area(&|f| f.zone == Zone::SkyBridge);
    let gfa = a_base + a_lobby + a_bank + a_aud + a_body + a_spire_a + a_spire_b + a_bridge;

    let bridge_z = *spec.levels_of(Zone::SkyBridge).last().unwrap() as f64 * 4.0;
    let rho = day.transport.mean_route_s / (180.0 * 4.0 * day.transport.mean_batch);

    let subs: Vec<(&str, String)> = vec![
        ("@@VERSION@@", env!("CARGO_PKG_VERSION").into()),
        ("@@DATE@@", today()),
        ("@@FPRINT@@", day.report_hash.clone()),
        ("@@COMMIT8@@", day.state_commitment[..16].to_string()),
        ("@@SERVED@@", day.transport.served.to_string()),
        ("@@AVGWAIT@@", format!("{:.1}", day.transport.avg_wait_s)),
        ("@@P95@@", day.transport.p95_wait_s.to_string()),
        ("@@ROBOTSHARE@@", format!("{:.0}", day.transport.robot_share * 100.0)),
        ("@@EB@@", format!("{:.2}", day.transport.mean_batch)),
        ("@@ETROUTE@@", format!("{:.0}", day.transport.mean_route_s)),
        ("@@RHO@@", format!("{rho:.3}")),
        ("@@BOI@@", format!("{:.3}", day.operating.total)),
        ("@@TREASURY@@", thou(day.treasury_uqug as u64)),
        ("@@PLATES@@", spec.floors.len().to_string()),
        ("@@A_BASEMENTS@@", thou(a_base)),
        ("@@A_LOBBY@@", thou(a_lobby)),
        ("@@A_BANK@@", thou(a_bank)),
        ("@@A_AUD@@", thou(a_aud)),
        ("@@A_BODY@@", thou(a_body)),
        ("@@A_SPIREA@@", thou(a_spire_a)),
        ("@@A_SPIREB@@", thou(a_spire_b)),
        ("@@A_BRIDGE@@", thou(a_bridge)),
        ("@@GFA@@", thou(gfa)),
        ("@@ROOF_A@@", format!("{:.0}", spec.top_level() as f64 * 4.0)),
        ("@@ROOF_B@@", format!("{:.0}", spec.spire_top(Spire::B).unwrap() as f64 * 4.0)),
        ("@@HEIGHT@@", format!("{:.0}", science::tower_height_m(&spec))),
        ("@@LEG70R@@", format!("{:.1}", leg_time_s(70, CarKind::RobotFreight))),
        ("@@LEG70H@@", format!("{:.1}", leg_time_s(70, CarKind::HumanCab))),
        ("@@BRIDGE_Z@@", format!("{bridge_z:.0}")),
        ("@@DRIFT_A@@", format!("{:.2}", bridge.drift_at_bridge_a_m)),
        ("@@DRIFT_B@@", format!("{:.2}", bridge.drift_at_bridge_b_m)),
        ("@@DIFF@@", format!("{:.2}", bridge.differential_m)),
        ("@@JOINT@@", format!("{:.2}", bridge.joint_budget_m)),
        ("@@EVAC_OCC@@", thou(evac.occupants as u64)),
        ("@@EVAC_FLOW@@", thou(evac.flow_time_s.round() as u64)),
        ("@@EVAC_DESCENT@@", thou(evac.descent_time_s.round() as u64)),
        ("@@EVAC_MIN@@", format!("{:.0}", evac.estimate_s / 60.0)),
        ("@@PHOTON_US@@", format!("{:.3}", annex.photon_transit_us)),
        ("@@DILRATE@@", sci(annex.dilation_rate, 2)),
        ("@@GAIN_US@@", format!("{:.0}", annex.crown_gain_us_per_century)),
        ("@@RS@@", sci(annex.vault_schwarzschild_m, 2)),
        ("@@RS_PLANCK@@", sci(annex.vault_rs_planck_lengths, 1)),
        ("@@ALPHA_INV@@", format!("{:.9}", annex.alpha_inv)),
        ("@@FLATTERY_PCT@@", format!("{:.3}", annex.joke_flattery_rel * 100.0)),
    ];

    let mut out = TEMPLATE.replace(
        "TEMPLATE — do not compile this file directly.",
        "GENERATED by skyskraber-whitepaper — do not hand-edit; edit template.tex.",
    );
    for (key, val) in &subs {
        assert!(out.contains(*key), "template lost placeholder {key}");
        out = out.replace(key, val);
    }
    // The discipline's teeth: refuse to emit a paper with an unfilled number.
    if let Some(pos) = out.find("@@") {
        let ctx = &out[pos..out.len().min(pos + 40)];
        panic!("unfilled placeholder near: {ctx}");
    }

    std::fs::write(&out_path, &out).expect("write tex");
    eprintln!("▸ generated {out_path} — {} substitutions, all placeholders filled", subs.len());
    eprintln!("  day fingerprint {} · commitment {}…", day.report_hash, &day.state_commitment[..16]);
    eprintln!("  BOI {:.3} · p95 {}s · GFA {gfa} m² · α⁻¹ {}", day.operating.total, day.transport.p95_wait_s, annex.alpha_inv);
}
