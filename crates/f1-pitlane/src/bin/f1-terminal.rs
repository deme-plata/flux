//! F1-PITLANE — terminal prototype (P2).
//!
//! Terminal racing games win on *accuracy and depth*, not pixels. This front-end
//! leans all the way in: a real telemetry strip (tyre wear, ERS deploy, fuel,
//! brake temp, gap to leader), a live flag tower, and — the headline feature —
//! **crashes**. When a car spins or a pile-up erupts *ahead of you*, the race
//! stops for your call: Evade / Brake / Lift / Pit / Hold / Overtake. Pick fast,
//! pick right, or you're collected in the wreck.
//!
//! Modes:
//!   f1-terminal                      perfect the car, race Monaco (auto-driver)
//!   f1-terminal <track> [seed] [temperament]
//!                                    track = monaco|silverstone|flux|custom:<n>
//!                                    temperament = cautious|balanced|aggressive
//!   f1-terminal brief <track>        print the 1M-context track brief (for the AI engineer)
//!   f1-terminal scenario <track> [seed]
//!                                    emit a live incident as JSON for the qwen/DeepSeek engineer

use f1_pitlane::garage::Car;
use f1_pitlane::incident::{resolve_reaction, roll_corner, Flag, Incident, Reaction};
use f1_pitlane::race::Temperament;
use f1_pitlane::regs::Regulations;
use f1_pitlane::track::{flux_ring, generate_custom, monaco, silverstone, Track};
use f1_pitlane::util::{rand01, salt};

// --- ANSI ---
const R: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const YEL: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const MAG: &str = "\x1b[35m";
const GREY: &str = "\x1b[90m";

fn pick_track(name: &str) -> Track {
    match name {
        "monaco" => monaco(),
        "silverstone" | "silver" => silverstone(),
        "flux" | "flux-ring" => flux_ring(),
        s if s.starts_with("custom:") => {
            let seed: u64 = s.trim_start_matches("custom:").parse().unwrap_or(1);
            generate_custom("Custom Circuit", seed, 14)
        }
        _ => monaco(),
    }
}

fn pick_temperament(s: &str) -> Temperament {
    match s {
        "cautious" => Temperament::Cautious,
        "aggressive" | "aggro" => Temperament::Aggressive,
        _ => Temperament::Balanced,
    }
}

fn bar(pct: f64, width: usize, color: &str) -> String {
    let filled = ((pct / 100.0) * width as f64).round() as usize;
    let filled = filled.min(width);
    format!("{}{}{}{}", color, "█".repeat(filled), GREY, "░".repeat(width - filled))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("");

    if cmd == "brief" {
        let t = pick_track(args.get(2).map(|s| s.as_str()).unwrap_or("monaco"));
        println!("{}", t.to_context());
        println!("(context budget: ~{} tokens — fits a 1M window with room for the whole grid's live state)", t.context_tokens());
        return;
    }
    if cmd == "scenario" {
        emit_scenario(&args);
        return;
    }
    if cmd == "field" {
        field_race(&args);
        return;
    }

    let track = pick_track(if cmd.is_empty() { "monaco" } else { cmd });
    let seed: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2026);
    let temperament = pick_temperament(args.get(3).map(|s| s.as_str()).unwrap_or("balanced"));

    let regs = Regulations::y2026();
    let mut car = Car::new_2026(&regs);

    header(&track, temperament);
    garage_screen(&mut car, &regs);
    race(&track, &car, &regs, seed, temperament);
}

fn header(track: &Track, temp: Temperament) {
    println!("\n{}{}╔══════════════════════════════════════════════════════════════╗{}", BOLD, CYAN, R);
    println!("{}{}║   🏎  F1-PITLANE  ·  become the driver  ·  2026 regulations   ║{}", BOLD, CYAN, R);
    println!("{}{}╚══════════════════════════════════════════════════════════════╝{}", BOLD, CYAN, R);
    println!("  Venue : {}{}{} ({})", BOLD, track.name, R, track.country);
    println!("  Layout: {} corners · {} sectors · grip {:.2} · {}danger {:.2}{}",
        track.corners.len(), track.sectors, track.surface_grip,
        if track.danger > 0.8 { RED } else { GREEN }, track.danger, R);
    println!("  Driver: {}Rocky{} · temperament {}{:?}{}\n", BOLD, R, MAG, temp, R);
}

fn garage_screen(car: &mut Car, regs: &Regulations) {
    use f1_pitlane::garage::PartKind;
    println!("{}{}── GARAGE ──────────────────────────────────────────────────────{}", BOLD, YEL, R);
    println!("  Baseline car  pace {}{:.1}%{}  readiness {:.0}/100  scrutineering {}",
        DIM, car.performance(regs) * 100.0, R, car.readiness(regs),
        if car.is_legal(regs) { format!("{}PASS{}", GREEN, R) } else { format!("{}FAIL{}", RED, R) });
    println!("  {}Perfecting every part to the legal 2026 ceiling…{}", DIM, R);
    for k in PartKind::ALL {
        car.perfect_part(k, regs);
    }
    car.perfect_all(regs);
    for k in PartKind::ALL {
        let p = car.part(k);
        let (max, unit) = k.legal_max(regs);
        println!("    {:<14} {} {:>5.0} {}  {}✓ legal max{}",
            k.label(), bar(p.condition, 16, GREEN), max, unit, GREEN, R);
    }
    println!("  Weight shaved to {}{:.0} kg{} (legal floor) · fuel: 100% sustainable",
        BOLD, car.weight_kg, R);
    println!("  {}{}BUILD COMPLETE{} — pace {}{:.1}%{} · readiness {:.0}/100 · {}PASS{}\n",
        BOLD, GREEN, R, BOLD, car.performance(regs) * 100.0, R, car.readiness(regs), GREEN, R);
}

fn flag_tower(flag: Flag) -> String {
    let (c, txt) = match flag {
        Flag::Green => (GREEN, "🟢 GREEN"),
        Flag::Yellow => (YEL, "🟡 YELLOW"),
        Flag::DoubleYellow => (YEL, "🟡🟡 DBL YEL"),
        Flag::VirtualSafetyCar => (YEL, "🟠 VSC"),
        Flag::SafetyCar => (YEL, "🚗 SAFETY CAR"),
        Flag::Red => (RED, "🔴 RED FLAG"),
    };
    format!("{}{}{}", c, txt, R)
}

fn race(track: &Track, base_car: &Car, regs: &Regulations, seed: u64, temp: Temperament) {
    let total_laps = (track.length_km * 78.0 / 3.337).round() as u32; // ~Monaco-distance scaled
    let total_laps = total_laps.clamp(30, 78);
    println!("{}{}── RACE · {} laps ────────────────────────────────────────────{}", BOLD, YEL, total_laps, R);
    println!("  {}Lights out!{}\n", BOLD, R);

    let mut live = base_car.clone();
    let mut position: i32 = 1;
    let mut gap = 0.0f64;
    let mut rivals_out = 0i32;
    let mut crashes = 0;
    let mut safety_cars = 0;
    let mut flag = Flag::Green;
    let mut fuel = 100.0f64;

    'race: for lap in 1..=total_laps {
        // de-escalate flags after an incident lap
        flag = Flag::Green;
        for corner in &track.corners {
            let inc = roll_corner(corner, track, &live, regs, lap, seed);
            match inc {
                Incident::Clean => {}
                Incident::Lockup => {
                    gap += 0.3;
                }
                _ => {
                    // INCIDENT AHEAD — rapid reaction required.
                    let reaction = temp.react(inc);
                    let ev = resolve_reaction(corner, inc, reaction, &live, regs, lap, seed);
                    flag = ev.flag;
                    if inc.is_crash() {
                        crashes += 1;
                    }
                    if matches!(ev.flag, Flag::SafetyCar | Flag::Red) {
                        safety_cars += 1;
                    }
                    narrate_incident(lap, corner.number, &corner.name, inc, reaction, &ev, flag);

                    if !ev.player_collected {
                        rivals_out += match inc {
                            Incident::CrashMulti(n) => n as i32,
                            Incident::CrashSingle => 1,
                            _ => 0,
                        };
                    }
                    gap += ev.time_delta;
                    if ev.player_dnf {
                        println!("\n  {}{}💥 DNF — collected in the {} at {} on lap {}.{}",
                            BOLD, RED, inc.label(), corner.name, lap, R);
                        classify(position, false, crashes, safety_cars, rivals_out);
                        return;
                    }
                }
            }
        }
        live.apply_wear(1);
        fuel = (fuel - 100.0 / total_laps as f64).max(0.0);

        // Recompute running position from pace + chaos survived.
        position = (1 + (gap / 5.0).round() as i32 - rivals_out).clamp(1, 20);

        if lap % 6 == 0 || lap == total_laps || !matches!(flag, Flag::Green) {
            telemetry(lap, total_laps, position, gap, &live, regs, fuel, flag, seed);
        }
        if lap == total_laps {
            break 'race;
        }
    }

    classify(position, true, crashes, safety_cars, rivals_out);
}

fn narrate_incident(lap: u32, cnum: u8, cname: &str, inc: Incident, react: Reaction, ev: &f1_pitlane::incident::CornerEvent, flag: Flag) {
    let inc_color = if inc.is_crash() { RED } else { YEL };
    println!("  {}L{:>2}{} {}T{}{} {} — {}{}{}  {}",
        DIM, lap, R, GREY, cnum, R, cname, BOLD, inc_color, inc.label(), R);
    let outcome = if ev.player_collected {
        format!("{}COLLECTED{}", RED, R)
    } else {
        format!("{}clear!{}", GREEN, R)
    };
    println!("       ⚡ reaction: {}{:?}{}  →  {}   {}  Δ{:+.1}s",
        MAG, react, R, outcome, flag_tower(flag), ev.time_delta);
}

#[allow(clippy::too_many_arguments)]
fn telemetry(lap: u32, total: u32, pos: i32, gap: f64, car: &Car, regs: &Regulations, fuel: f64, flag: Flag, seed: u64) {
    use f1_pitlane::garage::PartKind;
    let tyre = car.part(PartKind::Tyres).condition;
    // synth ERS deploy + brake temp deterministically from lap.
    let mut st = salt(seed, lap as u64);
    let ers = 30.0 + rand01(&mut st) * 70.0;
    let brake = 350.0 + rand01(&mut st) * 650.0;
    let posc = if pos <= 3 { GREEN } else if pos <= 10 { CYAN } else { YEL };
    println!("  {}┌ TELEMETRY ─ lap {}/{} ─────────────────────────────────────{}", GREY, lap, total, R);
    println!("  {}│{} P{}{}{}  gap {:+.1}s   {}   fuel {:.0}%",
        GREY, R, posc, pos, R, gap, flag_tower(flag), fuel);
    println!("  {}│{} tyre {} {:>3.0}%   ERS {} {:>3.0}%   brake {:.0}°C   pace {:.1}%",
        GREY, R,
        bar(tyre, 10, if tyre > 40.0 { GREEN } else { RED }), tyre,
        bar(ers, 10, CYAN), ers,
        brake, car.performance(regs) * 100.0);
    println!("  {}└──────────────────────────────────────────────────────────{}", GREY, R);
}

fn classify(pos: i32, finished: bool, crashes: usize, safety_cars: usize, rivals_out: i32) {
    println!("\n{}{}── CHEQUERED FLAG ──────────────────────────────────────────────{}", BOLD, CYAN, R);
    if finished {
        let medal = match pos { 1 => "🥇 WIN", 2 => "🥈 P2", 3 => "🥉 P3", _ => "" };
        let c = if pos == 1 { GREEN } else { CYAN };
        println!("  Classified: {}{}P{}{} {}", BOLD, c, pos, R, medal);
    } else {
        println!("  Classified: {}DNF{}", RED, R);
    }
    println!("  Race log: {}{} crashes{} · {}{} safety-car/red periods{} · {} rivals eliminated ahead of you",
        RED, crashes, R, YEL, safety_cars, R, rivals_out);
    println!("  {}The car you built survived the chaos. That's the difference between fast and finished.{}\n", DIM, R);
}

/// Emit a live incident scenario as JSON — fed to the qwen3.6 + DeepSeek-V4
/// race engineer so it can recommend a reaction with the whole track in context.
fn emit_scenario(args: &[String]) {
    let track = pick_track(args.get(2).map(|s| s.as_str()).unwrap_or("monaco"));
    let seed: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(2026);
    let regs = Regulations::y2026();
    let mut car = Car::new_2026(&regs);
    car.perfect_all(&regs);
    // find the first lap/corner that throws a crash
    for lap in 1..=20u32 {
        for corner in &track.corners {
            let inc = roll_corner(corner, &track, &car, &regs, lap, seed);
            if inc.is_crash() || matches!(inc, Incident::Spin) {
                let obj = serde_json::json!({
                    "track": track.name,
                    "lap": lap,
                    "corner": corner.number,
                    "corner_name": corner.name,
                    "corner_kind": format!("{:?}", corner.kind),
                    "runoff": format!("{:?}", corner.runoff),
                    "overtake_zone": corner.overtake_zone,
                    "incident_ahead": inc.label(),
                    "tyre_condition_pct": car.part(f1_pitlane::garage::PartKind::Tyres).condition,
                    "legal_reactions": ["Hold","Lift","Brake","Evade","Pit","Overtake"],
                    "track_context": track.to_context(),
                });
                println!("{}", serde_json::to_string_pretty(&obj).unwrap());
                return;
            }
        }
    }
    eprintln!("no crash scenario found for seed {seed}");
}

/// `field <track> [seed] [rain]` — race the whole grid: the player, the agentic-
/// money AI siblings (Rocky the politician-leader, Codex, Grok, DeepSeek…) and
/// random SIGIL-coin human entrants. Everyone finishes; SIGIL pays out 50/30/20.
fn field_race(args: &[String]) {
    use f1_pitlane::drivers::{random_sigil_humans, sigil_human, simulate_field, starting_grid, DriverKind};
    use f1_pitlane::weather::Weather;

    let track = pick_track(args.get(2).map(|s| s.as_str()).unwrap_or("monaco"));
    let seed: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(2026);
    let rain: f64 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let weather = if rain > 0.0 {
        Weather::from_open_meteo(16.0, 90.0, rain * 6.0, if rain > 0.5 { 65 } else { 51 }, 12.0)
    } else {
        Weather::dry_warm()
    };
    let base_lap = 60.0 + track.length_km * 4.5;

    // Player + 5 SIGIL humans, each anteing 25 SIGIL.
    let player = sigil_human("You (Viktor)", "qnkefca1e8c0723", 25.0, 0.90, 0.5, 0.85, 0.92);
    let humans = random_sigil_humans(seed ^ 0x5161, 5, 25.0);
    let grid = starting_grid(player, humans, seed);
    let pool: f64 = f1_pitlane::drivers::prize_pool(&grid);

    println!("\n{}{}╔════════════ F1-PITLANE · FIELD RACE ════════════╗{}", BOLD, CYAN, R);
    println!("{}{}║  {} · {} laps · {}  ║{}", BOLD, CYAN, track.name, track.corners.len(),
        if weather.is_wet() { "WET" } else { "DRY" }, R);
    println!("{}{}╚══════════════════════════════════════════════════╝{}", BOLD, CYAN, R);
    println!("  Weather: {}", weather.describe());
    println!("  SIGIL prize pool: {}{:.0} SIGIL{} (entrants ante 25 each) — pays 50/30/20\n", BOLD, pool, R);

    let results = simulate_field(&grid, &track, (base_lap as u32 / 2).max(40), base_lap, &weather, seed);
    println!("  {}{}POS  DRIVER              TEAM/PERSONA            RESULT        SIGIL{}", BOLD, GREY, R);
    for r in &results {
        let tag = match r.driver.kind {
            DriverKind::AiSibling => format!("{}🤖{}", CYAN, R),
            DriverKind::SigilHuman => format!("{}🧑{}", GREEN, R),
            DriverKind::Npc => format!("{} ·{}", GREY, R),
        };
        let result = if r.finished {
            if r.position == 1 { format!("{}WIN{}", GREEN, R) }
            else { format!("+{:.1}s", r.gap_s) }
        } else {
            format!("{}DNF L{}{}", RED, r.dnf_lap.unwrap_or(0), R)
        };
        let sigil = if r.sigil_won > 0.0 { format!("{}{:.0}{}", BOLD, r.sigil_won, R) } else { "—".into() };
        let namecol = if r.driver.name.contains("Viktor") { format!("{}{}{}", BOLD, r.driver.name, R) } else { r.driver.name.clone() };
        println!("  {}{:>2}{}  {} {:<18} {:<22} {:<12} {}",
            if r.position <= 3 { GREEN } else { R }, r.position, R,
            tag, namecol,
            truncate(&r.driver.persona, 22), result, sigil);
    }
    let winner = &results[0];
    println!("\n  🏆 {}{}{} takes the win and {}{:.0} SIGIL{} from the pool.\n",
        BOLD, winner.driver.name, R, BOLD, winner.sigil_won, R);
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n - 1).collect::<String>()) }
}
