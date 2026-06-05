//! **Crashes — the top feature.**
//!
//! A race here is a living thing: cars lock up, spin, and pile into each other,
//! the track throws yellows, the Safety Car bunches the field, and a red flag
//! can stop everything. When a car crashes *ahead of you*, you get a narrow
//! **reaction window** and must change plan instantly — Evade, Brake, Lift, Pit,
//! or gamble and Hold. The wrong call collects you in the wreck; the right one
//! can leap you up the order while rivals crash out.
//!
//! Everything is deterministic from a seed (via [`crate::util`]) so a chaotic
//! race replays identically and the LLM race engineer can be evaluated fairly.

use crate::garage::Car;
use crate::regs::Regulations;
use crate::track::{Corner, Track};
use crate::util::{rand01, salt};
use serde::{Deserialize, Serialize};

/// What physically happened at a corner this lap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Incident {
    Clean,
    Lockup,          // lost time, recoverable
    Spin,            // big time loss, maybe recoverable
    CrashSingle,     // one car out
    CrashMulti(u8),  // pile-up: N cars collected
    Debris,          // someone else's mess on the racing line
    Mechanical,      // car failure → DNF
}

impl Incident {
    pub fn is_crash(&self) -> bool {
        matches!(self, Incident::CrashSingle | Incident::CrashMulti(_))
    }
    pub fn label(&self) -> String {
        match self {
            Incident::Clean => "clean".into(),
            Incident::Lockup => "lock-up".into(),
            Incident::Spin => "spin".into(),
            Incident::CrashSingle => "CRASH".into(),
            Incident::CrashMulti(n) => format!("PILE-UP ({} cars)", n),
            Incident::Debris => "debris".into(),
            Incident::Mechanical => "mechanical DNF".into(),
        }
    }
}

/// The marshals' flag state — escalates with the size of the incident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Flag {
    Green,
    Yellow,
    DoubleYellow,
    VirtualSafetyCar,
    SafetyCar,
    Red,
}

impl Flag {
    pub fn label(&self) -> &'static str {
        match self {
            Flag::Green => "GREEN",
            Flag::Yellow => "YELLOW",
            Flag::DoubleYellow => "DOUBLE YELLOW",
            Flag::VirtualSafetyCar => "VSC",
            Flag::SafetyCar => "SAFETY CAR",
            Flag::Red => "RED FLAG",
        }
    }
    fn from_incident(inc: &Incident) -> Flag {
        match inc {
            Incident::Clean | Incident::Lockup => Flag::Green,
            Incident::Debris => Flag::Yellow,
            Incident::Spin => Flag::Yellow,
            Incident::CrashSingle => Flag::SafetyCar,
            Incident::CrashMulti(n) if *n >= 4 => Flag::Red,
            Incident::CrashMulti(_) => Flag::SafetyCar,
            Incident::Mechanical => Flag::VirtualSafetyCar,
        }
    }
}

/// The driver's instant response when something kicks off ahead. This is the
/// "rapid change of direction" mechanic — pick fast, pick right.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reaction {
    Hold,     // keep your line, keep the pace (rewarded only if it stays clean)
    Lift,     // ease off — small time loss, decent safety
    Brake,    // hard brake — bigger time loss, high safety
    Evade,    // dart off-line through the gap — best avoidance, needs a sharp car
    Pit,      // dive into the pits — lose track position, immune to the wreck
    Overtake, // send it past the chaos — huge upside, huge risk
}

impl Reaction {
    /// Base chance this reaction avoids being collected, before car sharpness.
    fn avoidance(&self) -> f64 {
        match self {
            Reaction::Hold => 0.45,
            Reaction::Lift => 0.72,
            Reaction::Brake => 0.85,
            Reaction::Evade => 0.80,
            Reaction::Pit => 1.0,
            Reaction::Overtake => 0.55,
        }
    }
    /// Time delta (s) this reaction costs (negative = gained places).
    fn time_cost(&self) -> f64 {
        match self {
            Reaction::Hold => 0.0,
            Reaction::Lift => 1.5,
            Reaction::Brake => 3.5,
            Reaction::Evade => 1.0,
            Reaction::Pit => 22.0,
            Reaction::Overtake => -2.5,
        }
    }
}

/// What happened to the *player* at one corner, given what unfolded ahead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CornerEvent {
    pub lap: u32,
    pub corner: u8,
    pub corner_name: String,
    /// What happened on track (may be triggered by a rival ahead).
    pub incident: Incident,
    pub flag: Flag,
    /// Was the incident *ahead of the player* (i.e. demanded a reaction)?
    pub ahead: bool,
    /// The reaction the player/engineer chose (None if nothing to react to).
    pub reaction: Option<Reaction>,
    /// Did the player get caught up in it?
    pub player_collected: bool,
    /// Net time delta to the player this corner (s).
    pub time_delta: f64,
    pub player_dnf: bool,
}

/// Roll what happens at a single corner this lap. `car`/`regs` modulate risk:
/// a worn, badly set-up car is far more likely to make a mistake; a sharp,
/// perfected car both crashes less and evades better.
pub fn roll_corner(
    corner: &Corner,
    track: &Track,
    car: &Car,
    regs: &Regulations,
    lap: u32,
    seed: u64,
) -> Incident {
    let mut st = salt(seed, (lap as u64) << 8 | corner.number as u64);
    let perf = car.performance(regs).clamp(0.0, 1.1);
    // Worn/ill car raises risk; sharp car lowers it. Tyre condition matters most.
    let condition_factor = 1.6 - 0.8 * perf;
    let danger = corner.base_risk * track.danger * condition_factor;
    let r = rand01(&mut st);
    if r > danger {
        // Mostly clean, but the overtake zones still cough up debris/lock-ups.
        let minor = rand01(&mut st);
        return if corner.overtake_zone && minor < 0.06 {
            Incident::Debris
        } else if minor < 0.03 {
            Incident::Lockup
        } else {
            Incident::Clean
        };
    }
    // Something went wrong. Severity scales with run-off + whether it's a pack zone.
    let sev = rand01(&mut st) * corner.runoff.severity();
    if corner.overtake_zone && sev > 0.85 {
        // Pack racing into a danger zone → multi-car pile-up.
        let cars = 2 + (rand01(&mut st) * 4.0) as u8; // 2..=5 cars
        Incident::CrashMulti(cars)
    } else if sev > 0.9 {
        Incident::Mechanical
    } else if sev > 0.55 {
        Incident::CrashSingle
    } else if sev > 0.3 {
        Incident::Spin
    } else {
        Incident::Lockup
    }
}

/// Resolve the player's fate at a corner where an incident is unfolding *ahead*.
/// Returns the corner event with collected/time/DNF filled in.
pub fn resolve_reaction(
    corner: &Corner,
    incident: Incident,
    reaction: Reaction,
    car: &Car,
    regs: &Regulations,
    lap: u32,
    seed: u64,
) -> CornerEvent {
    let mut st = salt(seed, (lap as u64) << 16 | (corner.number as u64) << 4 | 0xC);
    let perf = car.performance(regs).clamp(0.0, 1.1);
    // A sharper car points better — boosts evasive reactions.
    let sharpness = 0.85 + 0.20 * perf; // 0.85..~1.07
    let mut avoid = (reaction.avoidance() * sharpness).clamp(0.0, 0.99);
    // Bigger incidents are harder to thread; a pile-up fills the track.
    let difficulty = match incident {
        Incident::CrashMulti(n) => 0.10 * n as f64,
        Incident::CrashSingle => 0.12,
        Incident::Debris => 0.05,
        Incident::Spin => 0.08,
        _ => 0.0,
    };
    avoid = (avoid - difficulty).clamp(0.0, 0.99);
    if matches!(reaction, Reaction::Pit) {
        avoid = 1.0; // pitting takes you out of the firing line entirely
    }

    let collected = rand01(&mut st) > avoid;
    let mut time_delta = reaction.time_cost();
    let mut dnf = false;

    if collected {
        // Caught in it. Severity of the crash + run-off decides DNF vs damage.
        let sev = incident_base_severity(incident) * corner.runoff.severity();
        if sev > 0.8 || matches!(incident, Incident::CrashMulti(_)) && rand01(&mut st) > 0.4 {
            dnf = true;
            time_delta = 0.0;
        } else {
            time_delta += 8.0 + sev * 15.0; // damage, limp on
        }
    } else if matches!(reaction, Reaction::Overtake) {
        // Pulled off the brave move past the chaos — gained track position.
        time_delta = reaction.time_cost();
    }

    CornerEvent {
        lap,
        corner: corner.number,
        corner_name: corner.name.clone(),
        incident,
        flag: Flag::from_incident(&incident),
        ahead: true,
        reaction: Some(reaction),
        player_collected: collected,
        time_delta,
        player_dnf: dnf,
    }
}

fn incident_base_severity(inc: Incident) -> f64 {
    match inc {
        Incident::Clean | Incident::Lockup => 0.1,
        Incident::Debris => 0.2,
        Incident::Spin => 0.4,
        Incident::CrashSingle => 0.75,
        Incident::CrashMulti(n) => (0.7 + 0.08 * n as f64).min(1.2),
        Incident::Mechanical => 1.0,
    }
}

/// Suggest the *safe-but-not-timid* default reaction for an incident — the
/// baseline the LLM engineer is graded against (and the auto-pick when the
/// player doesn't choose in time).
pub fn default_reaction(incident: Incident) -> Reaction {
    match incident {
        Incident::CrashMulti(_) => Reaction::Brake,
        Incident::CrashSingle => Reaction::Evade,
        Incident::Spin | Incident::Debris => Reaction::Lift,
        _ => Reaction::Hold,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::monaco;

    fn perfect_car() -> (Car, Regulations) {
        let r = Regulations::y2026();
        let mut c = Car::new_2026(&r);
        c.perfect_all(&r);
        (c, r)
    }

    #[test]
    fn braking_beats_holding_through_a_pileup_on_average() {
        let (car, regs) = perfect_car();
        let corner = &monaco().corners[0]; // Sainte Dévote, overtake zone, barrier
        let inc = Incident::CrashMulti(4);
        let (mut brake_safe, mut hold_safe) = (0, 0);
        for seed in 0..400u64 {
            if !resolve_reaction(corner, inc, Reaction::Brake, &car, &regs, 1, seed).player_collected {
                brake_safe += 1;
            }
            if !resolve_reaction(corner, inc, Reaction::Hold, &car, &regs, 1, seed).player_collected {
                hold_safe += 1;
            }
        }
        assert!(brake_safe > hold_safe, "Brake ({brake_safe}) should survive a pile-up more than Hold ({hold_safe})");
    }

    #[test]
    fn pitting_is_always_immune_to_the_wreck() {
        let (car, regs) = perfect_car();
        let corner = &monaco().corners[8];
        for seed in 0..200u64 {
            let ev = resolve_reaction(corner, Incident::CrashMulti(5), Reaction::Pit, &car, &regs, 3, seed);
            assert!(!ev.player_collected, "pitting must dodge the crash");
            assert!(!ev.player_dnf);
        }
    }

    #[test]
    fn multi_car_crash_flies_the_safety_car_or_red() {
        assert!(matches!(Flag::from_incident(&Incident::CrashMulti(5)), Flag::Red));
        assert!(matches!(Flag::from_incident(&Incident::CrashMulti(2)), Flag::SafetyCar));
        assert!(matches!(Flag::from_incident(&Incident::CrashSingle), Flag::SafetyCar));
    }

    #[test]
    fn worn_car_crashes_more_than_a_perfect_one() {
        let r = Regulations::y2026();
        let track = monaco();
        let corner = &track.corners[8]; // Nouvelle Chicane — high risk
        let fresh = {
            let mut c = Car::new_2026(&r);
            c.perfect_all(&r);
            c
        };
        let worn = {
            let mut c = Car::new_2026(&r);
            c.perfect_all(&r);
            c.apply_wear(40); // trashed tyres + worn parts
            c
        };
        let count = |car: &Car| {
            (0..500u64)
                .filter(|s| roll_corner(corner, &track, car, &r, 1, *s).is_crash())
                .count()
        };
        let fresh_crashes = count(&fresh);
        let worn_crashes = count(&worn);
        assert!(worn_crashes > fresh_crashes,
            "worn car ({worn_crashes}) should crash more than fresh ({fresh_crashes})");
    }

    #[test]
    fn default_reaction_is_cautious_for_big_crashes() {
        assert_eq!(default_reaction(Incident::CrashMulti(4)), Reaction::Brake);
        assert_eq!(default_reaction(Incident::CrashSingle), Reaction::Evade);
    }
}
