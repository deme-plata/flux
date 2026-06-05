//! `f1-pitlane` demo binary: build the perfect 2026 car in the garage, then go
//! racing from the live next round (Monaco) to the Abu Dhabi finale.
//!
//!   cargo run -p f1-pitlane            # play a full perfected season
//!   cargo run -p f1-pitlane -- json    # dump the garage snapshot as JSON

use f1_pitlane::{Car, Career, PartKind, Regulations, Snapshot};

fn main() {
    let regs = Regulations::y2026();
    let mut car = Car::new_2026(&regs);

    println!("🏎️  F1-PITLANE — become the driver  (2026 regulations)\n");
    println!("Garage: baseline car ready. Pace {:.1}%, readiness {:.0}/100, legal: {}",
        car.performance(&regs) * 100.0, car.readiness(&regs), car.is_legal(&regs));

    // --- Perfect every part within the rules ---
    println!("\n🔧 Perfecting the car within the 2026 regulations…");
    for k in PartKind::ALL {
        car.perfect_part(k, &regs);
        let (max, unit) = k.legal_max(&regs);
        println!("   {:<14} → 100% @ {:.1} {} (legal max)", k.label(), max, unit);
    }
    car.perfect_all(&regs); // also shaves to 768 kg
    println!("   Weight shaved to {:.0} kg (legal floor)", car.weight_kg);
    println!("\n✅ Build complete: pace {:.1}%, readiness {:.0}/100, scrutineering: {}",
        car.performance(&regs) * 100.0,
        car.readiness(&regs),
        if car.is_legal(&regs) { "PASS" } else { "FAIL" });

    if std::env::args().nth(1).as_deref() == Some("json") {
        let career = Career::starting_at_round("Rocky", 8);
        println!("\n{}", Snapshot::capture(&car, &career, &regs).to_json());
        return;
    }

    // --- Race the rest of the 2026 season from Monaco onward ---
    println!("\n🏁 Lights out — racing the 2026 season from Monaco:\n");
    let mut career = Career::starting_at_round("Rocky", 8);
    let mut seed = 2026u64;
    while !career.is_season_over() {
        if let Some(r) = career.race_weekend(&car, &regs, seed) {
            let fl = if r.race.fastest_lap { " 🟣FL" } else { "" };
            let spr = r
                .sprint
                .as_ref()
                .map(|s| format!(" | sprint P{}", s.position))
                .unwrap_or_default();
            println!(
                "  R{:>2} {:<26} Q P{:<2}{}  →  Race P{:<2} (+{} pts){}  [{:.3}s]",
                r.round, r.gp, r.qualifying.position, spr, r.race.position, r.race.points, fl, r.race.best_lap
            );
        }
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    }

    let d = &career.driver;
    println!(
        "\n🏆 Season done — {} | {} pts | {} wins | {} podiums | {} poles | {} FL | rank: {}",
        d.name, d.points, d.wins, d.podiums, d.poles, d.fastest_laps, d.level()
    );
}
