//! The grid — and it isn't anonymous. F1-Pitlane invites the **agentic-money AI
//! siblings** of the Sigil/Flux network onto the track as special characters:
//! Rocky the politician-leader, Adrian (Erid), Codex, Grok, DeepSeek, Gemini —
//! each with a real settlement wallet, a persona, and a driving style. Alongside
//! them, **any human can enter by anteing the SIGIL native coin** into the race
//! prize pool. Everyone races; the flag falls; SIGIL is paid out on results.
//!
//! This roster is also the foundation for flux-p2p multiplayer
//! ([`crate::multiplayer`]): every [`GridDriver`] is addressable by wallet, so a
//! remote peer's car maps to exactly one entry on the grid.

use crate::util::{rand01, salt};
use crate::weather::Weather;
use serde::{Deserialize, Serialize};

/// What kind of entrant this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DriverKind {
    /// An agentic-money AI sibling (Rocky, Codex, …) racing as a named character.
    AiSibling,
    /// A human who entered by anteing SIGIL.
    SigilHuman,
    /// A field-filling NPC.
    Npc,
}

/// One entry on the starting grid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GridDriver {
    pub name: String,
    /// Short alias / handle (e.g. "rocky", "adrian@sigilgraph.com").
    pub handle: String,
    /// Settlement wallet (qnk… on Quillon/SIGIL). Empty for pure NPCs.
    pub wallet: String,
    pub kind: DriverKind,
    pub team: String,
    /// Flavour — Rocky's is "the politician-leader".
    pub persona: String,
    /// Raw one-lap speed skill, 0..1.
    pub pace: f64,
    /// Willingness to send it (drives crash-reaction temperament), 0..1.
    pub aggression: f64,
    /// Wet-weather craft, 0..1.
    pub wet_skill: f64,
    /// Mistake-avoidance / tyre management, 0..1.
    pub consistency: f64,
    /// SIGIL anted to enter (humans pay in; AI siblings are comped).
    pub entry_sigil: f64,
}

impl GridDriver {
    /// Effective performance (0..~1) for the conditions — pace, tempered by wet
    /// craft when it rains.
    pub fn effective_pace(&self, weather: &Weather) -> f64 {
        let rain = weather.rain_intensity();
        let wet_term = 1.0 - rain * (1.0 - self.wet_skill) * 0.6;
        (self.pace * wet_term).clamp(0.05, 1.0)
    }

    /// Base per-corner mistake/crash propensity, 0..1 — calmer, more consistent
    /// drivers err less; aggressive ones risk more.
    pub fn risk_factor(&self) -> f64 {
        (0.5 * (1.0 - self.consistency) + 0.5 * self.aggression).clamp(0.05, 1.0)
    }

    fn ai(name: &str, handle: &str, wallet: &str, team: &str, persona: &str, pace: f64, aggression: f64, wet: f64, consistency: f64) -> Self {
        GridDriver {
            name: name.into(), handle: handle.into(), wallet: wallet.into(),
            kind: DriverKind::AiSibling, team: team.into(), persona: persona.into(),
            pace, aggression, wet_skill: wet, consistency, entry_sigil: 0.0,
        }
    }
}

/// The agentic-money AI siblings, as special-character drivers. Wallets are the
/// real settlement addresses from the network where known.
pub fn ai_siblings() -> Vec<GridDriver> {
    vec![
        GridDriver::ai(
            "Rocky", "rocky",
            "qnk4973498a9865b291636faef205f728a49d98890f001e9e806479043f038ebf6c",
            "Flux Foundation", "the politician-leader — commands the room, wins on strategy not bravado",
            0.97, 0.45, 0.88, 0.95,
        ),
        GridDriver::ai(
            "Adrian", "adrian@sigilgraph.com",
            "qnk1f97ff0b330c7790e8c82a57579052851d2c15239c78b6124fee6a74e4026d67",
            "Erid Quiet-Ledger", "the quiet strategist — Erid half does the ledger math while Rocky does the loud clicks",
            0.93, 0.40, 0.90, 0.96,
        ),
        GridDriver::ai(
            "Codex", "codex",
            "qnka3a92bba00000000000000000000000000000000000000000000000000001f96",
            "Scalpel Racing", "surgical precision — late-brakes to the millimetre, never overcooks it",
            0.95, 0.55, 0.82, 0.92,
        ),
        GridDriver::ai(
            "Grok", "grok",
            "", "X-Stream", "the loud sender — full send everywhere, spectacular or in the wall",
            0.90, 0.92, 0.70, 0.65,
        ),
        GridDriver::ai(
            "DeepSeek-V4", "deepseek",
            "", "Reasoner GP", "the reasoner — thinks three corners ahead, deadly in changing conditions",
            0.94, 0.50, 0.93, 0.94,
        ),
        GridDriver::ai(
            "Gemini", "gemini",
            "", "Antigravity", "the all-rounder — quietly quick, rarely makes a mistake",
            0.92, 0.48, 0.85, 0.90,
        ),
    ]
}

/// Mint a human entrant who has anted `entry_sigil` of the SIGIL native coin.
pub fn sigil_human(name: &str, wallet: &str, entry_sigil: f64, pace: f64, aggression: f64, wet: f64, consistency: f64) -> GridDriver {
    GridDriver {
        name: name.into(), handle: name.to_lowercase().replace(' ', "_"),
        wallet: wallet.into(), kind: DriverKind::SigilHuman, team: "Privateer".into(),
        persona: "SIGIL-coin entrant".into(),
        pace: pace.clamp(0.3, 0.99), aggression: aggression.clamp(0.0, 1.0),
        wet_skill: wet.clamp(0.0, 1.0), consistency: consistency.clamp(0.0, 1.0),
        entry_sigil: entry_sigil.max(0.0),
    }
}

/// Generate `n` random SIGIL-coin human entrants from a seed — the "anyone can
/// join with the native coin" field. Deterministic.
pub fn random_sigil_humans(seed: u64, n: usize, entry_sigil: f64) -> Vec<GridDriver> {
    (0..n)
        .map(|i| {
            let mut st = salt(seed, 0x4200 ^ i as u64);
            let pace = 0.55 + rand01(&mut st) * 0.40;
            let aggression = rand01(&mut st);
            let wet = 0.4 + rand01(&mut st) * 0.55;
            let consistency = 0.5 + rand01(&mut st) * 0.45;
            let wallet = format!("qnk{:016x}{:048x}", salt(seed, i as u64), i as u128);
            sigil_human(&format!("Privateer #{}", i + 1), &wallet, entry_sigil, pace, aggression, wet, consistency)
        })
        .collect()
}

/// Fill remaining grid slots with anonymous NPC midfielders.
fn npc(i: usize, seed: u64) -> GridDriver {
    let mut st = salt(seed, 0x9C ^ i as u64);
    GridDriver {
        name: format!("Car #{}", i + 1), handle: format!("npc{}", i + 1), wallet: String::new(),
        kind: DriverKind::Npc, team: "Midfield".into(), persona: "grid filler".into(),
        pace: 0.70 + rand01(&mut st) * 0.20, aggression: 0.3 + rand01(&mut st) * 0.5,
        wet_skill: 0.5 + rand01(&mut st) * 0.4, consistency: 0.6 + rand01(&mut st) * 0.35,
        entry_sigil: 0.0,
    }
}

/// Build a 20-car starting grid: the player, the AI siblings, the SIGIL humans,
/// then NPCs to fill. The player is placed first in the returned list (their
/// grid slot is decided by qualifying, not list order).
pub fn starting_grid(player: GridDriver, humans: Vec<GridDriver>, seed: u64) -> Vec<GridDriver> {
    let mut grid = Vec::with_capacity(20);
    grid.push(player);
    grid.extend(ai_siblings());
    grid.extend(humans);
    let mut i = 0;
    while grid.len() < 20 {
        grid.push(npc(i, seed));
        i += 1;
    }
    grid.truncate(20);
    grid
}

/// The total SIGIL prize pool from everyone's entry ante.
pub fn prize_pool(grid: &[GridDriver]) -> f64 {
    grid.iter().map(|d| d.entry_sigil).sum()
}

/// One driver's classified result from a field race.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldResult {
    pub driver: GridDriver,
    pub finished: bool,
    pub crashed_out: bool,
    pub dnf_lap: Option<u32>,
    pub position: u8,
    pub points: u32,
    /// Gap to the winner (s); 0 for the winner, None-ish (large) for DNFs.
    pub gap_s: f64,
    pub total_time_s: f64,
    /// SIGIL native-coin winnings from the prize pool.
    pub sigil_won: f64,
}

fn handle_seed(handle: &str, wallet: &str, seed: u64) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in handle.bytes().chain(wallet.bytes()) {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    salt(seed, h)
}

fn race_points(pos: u8) -> u32 {
    match pos {
        1 => 25, 2 => 18, 3 => 15, 4 => 12, 5 => 10,
        6 => 8, 7 => 6, 8 => 4, 9 => 2, 10 => 1, _ => 0,
    }
}

/// Race the entire field. Every entrant — AI sibling or SIGIL human — runs the
/// full distance: their pace sets their race time, their risk + the track's
/// danger decide whether they survive, and the SIGIL prize pool pays out
/// 50/30/20 to the top three finishers.
pub fn simulate_field(
    grid: &[GridDriver],
    track: &crate::track::Track,
    laps: u32,
    base_laptime: f64,
    weather: &Weather,
    seed: u64,
) -> Vec<FieldResult> {
    let pool = prize_pool(grid);

    struct Run {
        idx: usize,
        finished: bool,
        dnf_lap: Option<u32>,
        time: f64,
    }
    let mut runs: Vec<Run> = Vec::with_capacity(grid.len());

    for (idx, d) in grid.iter().enumerate() {
        let mut st = handle_seed(&d.handle, &d.wallet, seed);
        let perf = d.effective_pace(weather);
        let clean_lap = crate::session::lap_time(perf, base_laptime);
        let risk = d.risk_factor();
        let mut time = 0.0;
        let mut finished = true;
        let mut dnf_lap = None;
        for lap in 1..=laps {
            // lap time with small consistency-driven variance
            let var = 1.0 + (rand01(&mut st) - 0.5) * (1.0 - d.consistency) * 0.10;
            time += clean_lap * var;
            // crash chance scales with track danger × driver risk
            let p_crash = track.danger * risk * 0.012 * (1.0 + weather.rain_intensity() * (1.0 - d.wet_skill));
            if rand01(&mut st) < p_crash {
                finished = false;
                dnf_lap = Some(lap);
                break;
            }
        }
        runs.push(Run { idx, finished, dnf_lap, time });
    }

    // Classify: finishers by time ascending, then DNFs by how far they got.
    runs.sort_by(|a, b| match (a.finished, b.finished) {
        (true, true) => a.time.partial_cmp(&b.time).unwrap(),
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        (false, false) => b.dnf_lap.unwrap_or(0).cmp(&a.dnf_lap.unwrap_or(0)),
    });

    let winner_time = runs.iter().find(|r| r.finished).map(|r| r.time).unwrap_or(0.0);
    let payout = [0.50, 0.30, 0.20];

    runs.iter()
        .enumerate()
        .map(|(rank, r)| {
            let position = (rank + 1) as u8;
            let d = grid[r.idx].clone();
            let sigil_won = if r.finished && rank < 3 { pool * payout[rank] } else { 0.0 };
            FieldResult {
                driver: d,
                finished: r.finished,
                crashed_out: !r.finished,
                dnf_lap: r.dnf_lap,
                position,
                points: if r.finished { race_points(position) } else { 0 },
                gap_s: if r.finished { r.time - winner_time } else { 0.0 },
                total_time_s: r.time,
                sigil_won,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rocky_is_the_politician_leader_with_his_real_wallet() {
        let rocky = &ai_siblings()[0];
        assert_eq!(rocky.name, "Rocky");
        assert!(rocky.persona.contains("politician-leader"));
        assert!(rocky.wallet.starts_with("qnk7154929a"));
        assert_eq!(rocky.kind, DriverKind::AiSibling);
        // a leader: fast and consistent, not a hot-head
        assert!(rocky.pace > 0.9 && rocky.consistency > 0.9 && rocky.aggression < 0.6);
    }

    #[test]
    fn grok_sends_it_rocky_manages_it() {
        let s = ai_siblings();
        let grok = s.iter().find(|d| d.name == "Grok").unwrap();
        let rocky = s.iter().find(|d| d.name == "Rocky").unwrap();
        assert!(grok.aggression > rocky.aggression);
        assert!(grok.risk_factor() > rocky.risk_factor());
    }

    #[test]
    fn humans_ante_sigil_into_the_prize_pool() {
        let humans = random_sigil_humans(7, 4, 25.0);
        assert_eq!(humans.len(), 4);
        assert!(humans.iter().all(|h| h.kind == DriverKind::SigilHuman && h.entry_sigil == 25.0));
        let grid = starting_grid(
            sigil_human("You", "qnkplayer", 25.0, 0.9, 0.5, 0.8, 0.9),
            humans,
            7,
        );
        assert_eq!(grid.len(), 20);
        // player + 4 humans all anted 25 = 125 SIGIL pool
        assert!((prize_pool(&grid) - 125.0).abs() < 1e-6);
    }

    #[test]
    fn random_humans_are_deterministic() {
        assert_eq!(random_sigil_humans(42, 3, 10.0), random_sigil_humans(42, 3, 10.0));
    }

    #[test]
    fn wet_weather_helps_the_rain_masters() {
        let wet = Weather::from_open_meteo(15.0, 95.0, 6.0, 65, 12.0);
        let dry = Weather::dry_warm();
        let deepseek = ai_siblings().into_iter().find(|d| d.name == "DeepSeek-V4").unwrap();
        let grok = ai_siblings().into_iter().find(|d| d.name == "Grok").unwrap();
        // In the wet, the high-wet-skill driver loses less pace than the low one.
        let ds_drop = deepseek.effective_pace(&dry) - deepseek.effective_pace(&wet);
        let grok_drop = grok.effective_pace(&dry) - grok.effective_pace(&wet);
        assert!(ds_drop < grok_drop, "rain master should lose less pace in the wet");
    }

    #[test]
    fn the_whole_field_finishes_the_race_and_sigil_pays_out() {
        let humans = random_sigil_humans(3, 5, 20.0);
        let player = sigil_human("Viktor", "qnkviktor", 20.0, 0.88, 0.5, 0.85, 0.9);
        let grid = starting_grid(player, humans, 3);
        let track = crate::track::silverstone();
        let results = simulate_field(&grid, &track, 52, 90.0, &Weather::dry_warm(), 99);
        assert_eq!(results.len(), 20);
        // positions are 1..=20, unique
        let mut pos: Vec<u8> = results.iter().map(|r| r.position).collect();
        pos.sort_unstable();
        assert_eq!(pos, (1..=20).collect::<Vec<u8>>());
        // the SIGIL pool (6 antes × 20) is fully paid to the top-3 finishers
        let pool = prize_pool(&grid);
        let paid: f64 = results.iter().map(|r| r.sigil_won).sum();
        assert!((paid - pool).abs() < 1e-6, "all {pool} SIGIL must be paid out, got {paid}");
        // AI siblings are on the grid by name
        assert!(results.iter().any(|r| r.driver.name == "Rocky"));
    }

    #[test]
    fn faster_more_consistent_driver_beats_a_slow_one_on_average() {
        let fast = sigil_human("Fast", "qnkfast", 0.0, 0.98, 0.3, 0.9, 0.97);
        let slow = sigil_human("Slow", "qnkslow", 0.0, 0.62, 0.3, 0.7, 0.7);
        let grid = vec![fast.clone(), slow.clone()];
        let track = crate::track::silverstone();
        let mut fast_ahead = 0;
        for seed in 0..120u64 {
            let r = simulate_field(&grid, &track, 50, 90.0, &Weather::dry_warm(), seed);
            let fp = r.iter().find(|x| x.driver.name == "Fast").unwrap().position;
            let sp = r.iter().find(|x| x.driver.name == "Slow").unwrap().position;
            if fp < sp { fast_ahead += 1; }
        }
        assert!(fast_ahead > 80, "the faster driver should usually win ({fast_ahead}/120)");
    }
}
