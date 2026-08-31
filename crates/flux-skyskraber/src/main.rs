//! `skyskraber` — the tower in your terminal.
//!
//!   skyskraber blueprint          print the validated floor program
//!   skyskraber day [--seed N]     run one deterministic day, print the report
//!   skyskraber day --hardened     run the guardian-ring tower's day (perfect posture)
//!   skyskraber emsec              the emanations-security posture: porous → perfect
//!   skyskraber api                list the registered MCP/HTTP endpoints

use flux_skyskraber::emsec::{self, EmsecConfig, EmsecPosture};
use flux_skyskraber::tower::{Spire, TowerSpec, Zone};
use flux_skyskraber::{api, sim};

fn zone_glyph(z: Zone) -> &'static str {
    match z {
        Zone::GoldVault => "🟨 GOLD VAULT",
        Zone::RobotBay => "🤖 ROBOT BAY",
        Zone::Lobby => "🚪 LOBBY",
        Zone::BankHall => "🏦 QUILLON BANK",
        Zone::Auditorium => "🎓 GRAND AUDITORIUM",
        Zone::Offices => "💼 offices",
        Zone::Mechanical => "⚙️  mechanical",
        Zone::SkyBridge => "🌉 SKY BRIDGE (garden deck)",
        Zone::SkyGarden => "🌿 SKY GARDEN",
    }
}

fn spire_tag(s: Option<Spire>) -> &'static str {
    match s {
        Some(Spire::A) => "  [spire A ▲]",
        Some(Spire::B) => "  [spire B]",
        None => "",
    }
}

fn print_blueprint() {
    let spec = TowerSpec::quillon_default();
    spec.validate().expect("canonical blueprint validates");
    println!("═══ {} ═══", spec.name);
    println!(
        "levels {}..{} · {} human shafts · {} robot shafts · facade: {}{}{}\n",
        spec.bottom_level(),
        spec.top_level(),
        spec.human_shafts,
        spec.robot_shafts,
        spec.facade.emblem,
        if spec.facade.light_column { " + light column" } else { "" },
        if spec.waterfront { " · waterfront plaza" } else { "" },
    );
    if let Some(split) = spec.split_level() {
        println!(
            "twin spires from level {split}: A tops at {} (beacon), B at {} · sky bridge at {:?}\n",
            spec.spire_top(Spire::A).unwrap_or(split),
            spec.spire_top(Spire::B).unwrap_or(split),
            spec.levels_of(Zone::SkyBridge),
        );
    }
    if let Some(bm) = flux_skyskraber::physics::bridge_movement(&spec) {
        let ev = flux_skyskraber::physics::evacuation(&spec);
        println!(
            "physics v0.1: bridge differential {:.2} m → joint budget {:.2} m · evacuation ~{:.0} min ({} occupants)\n",
            bm.differential_m, bm.joint_budget_m, ev.estimate_s / 60.0, ev.occupants,
        );
    }
    let mut floors = spec.floors.clone();
    floors.sort_by_key(|f| (-f.level, f.spire));
    let mut last: Option<(Zone, Option<Spire>)> = None;
    for f in &floors {
        if last != Some((f.zone, f.spire)) {
            println!("  {:>4} │ {}{}", f.level, zone_glyph(f.zone), spire_tag(f.spire));
            last = Some((f.zone, f.spire));
        }
    }
}

fn print_day(seed: u64, hardened: bool) {
    let r = if hardened { sim::run_day_hardened(seed) } else { sim::run_day(seed) };
    println!("═══ one day in {} (seed {seed}) ═══", r.tower);
    println!("  transport  {} rides · avg wait {:.1}s · p95 {}s · robot share {:.0}% · E[b] {:.2} · E[T_route] {:.0}s",
        r.transport.served, r.transport.avg_wait_s, r.transport.p95_wait_s,
        r.transport.robot_share * 100.0, r.transport.mean_batch, r.transport.mean_route_s);
    println!("  vault      {} bars · witnessed: {} ({} anchors) · head {}…",
        r.vault_bars, r.vault_witnessed, r.vault_anchors, &r.vault_head[..16]);
    println!("  bank       treasury {} uQUG · payroll {}/{} paid", r.treasury_uqug, r.payroll_paid, r.payroll_paid + r.payroll_failed);
    println!("  auditorium {} talk(s) held", r.talks_held);
    println!("  operating  {:.3} — {}", r.operating.total, r.operating.verdict);
    println!("    flow {:.2} · payroll {:.2} · utilization {:.2} · autonomy {:.2} · custody {:.2}",
        r.operating.components.flow, r.operating.components.payroll_reliability,
        r.operating.components.utilization, r.operating.components.autonomy_band,
        r.operating.components.custody_integrity);
    println!("  emsec      {:.3} — {}", r.emsec.total, r.emsec.verdict);
    println!("    red/black {:.2} · inspectable-space {:.2} · averaging-resist {:.2} · fail-closed {:.2}",
        r.emsec.components.red_black_separation, r.emsec.components.inspectable_space,
        r.emsec.components.averaging_resistance, r.emsec.components.fail_closed);
    for f in &r.emsec.findings {
        println!("      ⚠ {f}");
    }
    println!("    hardened target: {:.3} (HSM + masked beacon + 30 m standoff + integrity gaps closed)",
        r.emsec_hardened_total);
    println!("  cortex     {} rounds · decisions: {:?}", r.cortex_rounds,
        r.decisions.iter().take(6).collect::<Vec<_>>());
    println!("  state      commitment {}…", &r.state_commitment[..16]);
    println!("  fingerprint {}", r.report_hash);
}

fn posture_block(title: &str, p: &EmsecPosture) {
    println!("── {title} ──");
    println!("  posture {:.3} — {}", p.total, p.verdict);
    println!(
        "  P1 red/black {:.2} · P2 inspectable-space {:.2} · P3 averaging-resist {:.2} · P4 fail-closed {:.2}",
        p.components.red_black_separation,
        p.components.inspectable_space,
        p.components.averaging_resistance,
        p.components.fail_closed,
    );
    if p.findings.is_empty() {
        println!("  ✓ no findings — every principle at full strength");
    } else {
        for f in &p.findings {
            println!("  ⚠ {f}");
        }
    }
}

fn print_emsec() {
    println!("═══ SIGIL Nation EMSEC Doctrine — the tower's emanations posture ═══\n");
    let before = emsec::assess(&TowerSpec::quillon_default(), &EmsecConfig::doctrine_v0());
    let after = emsec::assess(&TowerSpec::quillon_hardened(), &EmsecConfig::doctrine_v0_hardened());
    posture_block("BEFORE — the tower as it stands (canonical, config v0)", &before);
    println!();
    posture_block("AFTER — the guardian-ring tower, doctrine-hardened", &after);
    println!();
    println!("── the four heroes standing guard ──");
    println!("  P1  guardian ring   — bank hall re-zoned to RED 2–4, GRAY refuge decks at 1 & 5;");
    println!("                        + signing key in an HSM (RED/BLACK power+cabling isolation)");
    println!("  P2  earth & water   — gold (#79) below grade; ≥30 m standoff + waterfront on the RED faces");
    let req_ms = emsec::required_jitter_window_s(&EmsecConfig::doctrine_v0()) * 1000.0;
    println!("  P3  masked beacon   — the light column (#137) still pulses every block, but JITTERED by");
    println!("                        ≥{req_ms:.0} ms of CRYPTO-UNPREDICTABLE offset (a VRF, not a public PRNG):");
    println!("                        the heartbeat lives, the phase-lock an eavesdropper needs dies");
    println!("  P4  fail-closed     — producer-signature verification ON, cross-node integrity verified");
    println!();
    println!("  137 preserved: top 88 + 45 shared plates + 4 basements — untouched by the hardening.");
    println!("  #79 gold and #137 light are guarded on all four principles. Nothing gets in.");
}

fn print_api() {
    println!("═══ flux-skyskraber MCP/HTTP surface (compile-time registered) ═══");
    for e in api::registered_endpoints() {
        println!("  {:>4} {:<38} {}", e.method, e.path, e.summary);
    }
}

/// Not listed in the usage line. Some things about a building you should have
/// to be told by someone who loves it.
fn print_magic() {
    let key = blake3::hash(b"quillonium-sanctum");
    println!("═══ the secret element ═══");
    println!();
    println!("Below ground, level B4, the vault holds element 79 — gold. Matter's");
    println!("way of storing value: heavy, inert, guarded.");
    println!();
    println!("Above, running the full axis of spire A, is the light column. The");
    println!("public story is 'architectural lighting'. The truth: it pulses once");
    println!("per Quillon Graph block — the tower is a full node, and the beacon");
    println!("is its heartbeat. Value stored as INFORMATION, not matter.");
    println!();
    println!("Physicists' folklore names element 137 'Feynmanium' — the last");
    println!("element naive relativistic quantum mechanics permits; past it, the");
    println!("innermost electron would outrun light. It cannot exist as matter.");
    println!("So the tower keeps it the only way it can be kept: as light.");
    println!("Quillonium, Z = 137. And the building signs the joke in its own");
    println!("geometry: top level 88 + 45 shared plates + 4 basements = 137.");
    println!();
    println!("resonance key: {}", hex::encode(&key.as_bytes()[..8]));
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("blueprint") => print_blueprint(),
        Some("magic") => print_magic(),
        Some("day") => {
            let seed = args
                .iter()
                .position(|a| a == "--seed")
                .and_then(|i| args.get(i + 1))
                .and_then(|s| s.parse().ok())
                .unwrap_or(42);
            let hardened = args.iter().any(|a| a == "--hardened");
            print_day(seed, hardened);
        }
        Some("emsec") => print_emsec(),
        Some("api") => print_api(),
        _ => {
            println!("usage: skyskraber <blueprint | day [--seed N] [--hardened] | emsec | api>");
        }
    }
}
