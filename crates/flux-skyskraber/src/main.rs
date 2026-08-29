//! `skyskraber` — the tower in your terminal.
//!
//!   skyskraber blueprint        print the validated floor program
//!   skyskraber day [--seed N]   run one deterministic day, print the report
//!   skyskraber api              list the registered MCP/HTTP endpoints

use flux_skyskraber::tower::{TowerSpec, Zone};
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
        Zone::SkyGarden => "🌿 SKY GARDEN",
    }
}

fn print_blueprint() {
    let spec = TowerSpec::quillon_default();
    spec.validate().expect("canonical blueprint validates");
    println!("═══ {} ═══", spec.name);
    println!("levels {}..{} · {} human shafts · {} robot shafts\n", spec.bottom_level(), spec.top_level(), spec.human_shafts, spec.robot_shafts);
    let mut floors = spec.floors.clone();
    floors.sort_by_key(|f| -f.level);
    let mut last: Option<Zone> = None;
    for f in &floors {
        if last != Some(f.zone) {
            println!("  {:>4} │ {}", f.level, zone_glyph(f.zone));
            last = Some(f.zone);
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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("blueprint") => print_blueprint(),
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
