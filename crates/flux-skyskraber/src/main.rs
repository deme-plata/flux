//! `skyskraber` — the tower in your terminal.
//!
//!   skyskraber blueprint        print the validated floor program
//!   skyskraber day [--seed N]   run one deterministic day, print the report
//!   skyskraber api              list the registered MCP/HTTP endpoints

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

fn print_day(seed: u64) {
    let r = sim::run_day(seed);
    println!("═══ one day in {} (seed {seed}) ═══", r.tower);
    println!("  transport  {} rides · avg wait {:.1} · p95 {} ticks · robot share {:.0}%",
        r.transport.served, r.transport.avg_wait_ticks, r.transport.p95_wait_ticks, r.transport.robot_share * 100.0);
    println!("  vault      {} bars · chain intact: {} · head {}…", r.vault_bars, r.vault_chain_intact, &r.vault_head[..16]);
    println!("  bank       treasury {} uQUG · payroll {}/{} paid", r.treasury_uqug, r.payroll_paid, r.payroll_paid + r.payroll_failed);
    println!("  auditorium {} talk(s) held", r.talks_held);
    println!("  culture    {:.3} — {}", r.culture.total, r.culture.verdict);
    println!("    flow {:.2} · fairness {:.2} · wisdom {:.2} · autonomy {:.2} · safety {:.2}",
        r.culture.components.flow, r.culture.components.fairness, r.culture.components.wisdom,
        r.culture.components.autonomy, r.culture.components.safety);
    println!("  cortex     {} rounds · decisions: {:?}", r.cortex_rounds,
        r.decisions.iter().take(6).collect::<Vec<_>>());
    println!("  fingerprint {}", r.report_hash);
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
            print_day(seed);
        }
        Some("api") => print_api(),
        _ => {
            println!("usage: skyskraber <blueprint | day [--seed N] | api>");
        }
    }
}
