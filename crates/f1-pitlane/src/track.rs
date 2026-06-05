//! Rich, fully-enumerated circuits. Terminal games win on *accuracy and depth*,
//! not pixels — so a track here isn't a background image, it's every corner with
//! its gear, apex speed, run-off type, elevation and **base crash risk**.
//!
//! ## "Fits in 1M context"
//!
//! Every track is bounded so its full description **plus** live race state can be
//! handed to an LLM race engineer (qwen3.6 / DeepSeek-V4) inside a 1M-token
//! window. [`Track::context_tokens`] estimates the budget and
//! [`Track::fits_context`] guarantees it. Real circuits run ~1–3 k tokens; the
//! whole grid's state on top is still a rounding error against 1M — which is the
//! point: the engineer can reason over the *entire* track when a car crashes
//! ahead and a rapid change of plan is needed.

use crate::util::rand01;
use serde::{Deserialize, Serialize};

/// The shape/speed character of a corner — drives apex speed, gear and risk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CornerKind {
    Hairpin,    // 1st/2nd gear, big stop
    Slow,       // 2nd/3rd
    Medium,     // 3rd/4th
    Fast,       // 5th/6th, committed
    Flatout,    // taken flat, kink
    Chicane,    // quick left-right, kerb-dependent
}

impl CornerKind {
    pub fn typical_gear(&self) -> u8 {
        match self {
            CornerKind::Hairpin => 1,
            CornerKind::Slow => 2,
            CornerKind::Medium => 4,
            CornerKind::Fast => 6,
            CornerKind::Flatout => 8,
            CornerKind::Chicane => 3,
        }
    }
    pub fn apex_kmh(&self) -> u32 {
        match self {
            CornerKind::Hairpin => 70,
            CornerKind::Slow => 110,
            CornerKind::Medium => 180,
            CornerKind::Fast => 250,
            CornerKind::Flatout => 320,
            CornerKind::Chicane => 130,
        }
    }
    /// Physically-plausible (corner radius m, arc length m) for the kind. Real
    /// road geometry: a hairpin is tight and short, a flat-out kink is huge.
    pub fn geometry(&self) -> (f64, f64) {
        match self {
            CornerKind::Hairpin => (12.0, 45.0),
            CornerKind::Slow => (45.0, 80.0),
            CornerKind::Medium => (120.0, 130.0),
            CornerKind::Fast => (260.0, 180.0),
            CornerKind::Flatout => (650.0, 120.0),
            CornerKind::Chicane => (28.0, 70.0),
        }
    }
    /// Default straight length feeding into a corner of this kind (m).
    pub fn default_straight(&self) -> f64 {
        match self {
            CornerKind::Hairpin => 250.0,
            CornerKind::Slow => 180.0,
            CornerKind::Medium => 150.0,
            CornerKind::Fast => 220.0,
            CornerKind::Flatout => 400.0,
            CornerKind::Chicane => 200.0,
        }
    }
}

/// What's waiting if you get it wrong — the single biggest driver of crash cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunOff {
    Tarmac,   // forgiving, big asphalt run-off
    Gravel,   // beaches you, likely race over
    Grass,    // slippery, snap risk
    Wall,     // street circuit — instant heavy crash
    Barrier,  // tecpro/armco, heavy crash
}

impl RunOff {
    /// Severity multiplier applied when an incident happens at this corner.
    pub fn severity(&self) -> f64 {
        match self {
            RunOff::Tarmac => 0.35,
            RunOff::Grass => 0.7,
            RunOff::Gravel => 0.9,
            RunOff::Barrier => 1.15,
            RunOff::Wall => 1.4,
        }
    }
}

/// One corner of the circuit — with real road geometry so apex speeds come
/// from physics, not a lookup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Corner {
    pub number: u8,
    pub name: String,
    pub kind: CornerKind,
    pub runoff: RunOff,
    /// 0.0..1.0 baseline chance something goes wrong here on a given lap, before
    /// car condition / weather / battles are factored in.
    pub base_risk: f64,
    /// Metres of elevation change through the corner (+up / -down).
    pub elevation_m: i16,
    /// True if this corner opens onto an overtaking / DRS-style zone — i.e. where
    /// cars pile in and multi-car incidents start.
    pub overtake_zone: bool,
    /// Corner radius (m) — drives apex speed via v = sqrt(mu * g * r).
    #[serde(default)]
    pub radius_m: f64,
    /// Arc length through the corner (m).
    #[serde(default)]
    pub arc_len_m: f64,
    /// Length of the straight feeding into this corner (m) — where you
    /// accelerate, deploy ERS and reach top speed before braking.
    #[serde(default)]
    pub straight_before_m: f64,
}

impl Corner {
    pub fn new(number: u8, name: &str, kind: CornerKind, runoff: RunOff, base_risk: f64, elevation_m: i16, overtake_zone: bool) -> Self {
        let (radius_m, arc_len_m) = kind.geometry();
        Corner {
            number,
            name: name.to_string(),
            kind,
            runoff,
            base_risk,
            elevation_m,
            overtake_zone,
            radius_m,
            arc_len_m,
            straight_before_m: kind.default_straight(),
        }
    }

    /// Override the preceding straight length (real circuit data).
    pub fn straight(mut self, m: f64) -> Self {
        self.straight_before_m = m;
        self
    }

    /// Override the corner radius (real circuit data).
    pub fn radius(mut self, m: f64) -> Self {
        self.radius_m = m;
        self
    }
}

/// A full circuit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub name: String,
    pub country: String,
    pub length_km: f64,
    pub corners: Vec<Corner>,
    pub sectors: u8,
    /// Overall grip (0.7 wet street .. 1.0 grippy permanent track).
    pub surface_grip: f64,
    /// How crash-prone the layout is overall (street circuits high).
    pub danger: f64,
    pub custom: bool,
}

impl Track {
    /// Estimated tokens to describe the whole track (~4 chars/token).
    pub fn context_tokens(&self) -> usize {
        let json = serde_json::to_string(self).map(|s| s.len()).unwrap_or(0);
        json / 4
    }

    /// Guaranteed to fit a 1M-token window with vast headroom for live state.
    pub fn fits_context(&self) -> bool {
        self.context_tokens() < 1_000_000
    }

    /// A compact, information-dense brief for the LLM engineer: every corner,
    /// its risk, run-off and whether it's an overtake zone. This is what makes a
    /// "rapid change of direction when a car crashes ahead" possible — the model
    /// already holds the entire map.
    pub fn to_context(&self) -> String {
        let mut s = format!(
            "TRACK {} ({}) — {:.3} km, {} sectors, grip {:.2}, danger {:.2}{}\nCORNERS:\n",
            self.name,
            self.country,
            self.length_km,
            self.sectors,
            self.surface_grip,
            self.danger,
            if self.custom { " [CUSTOM]" } else { "" },
        );
        for c in &self.corners {
            s.push_str(&format!(
                "  T{:<2} {:<22} {:?}/{:?} risk={:.2} elev={:+}m{}\n",
                c.number,
                c.name,
                c.kind,
                c.runoff,
                c.base_risk,
                c.elevation_m,
                if c.overtake_zone { " <OVERTAKE>" } else { "" },
            ));
        }
        s
    }

    /// Corners flagged as overtaking zones — where multi-car crashes ignite.
    pub fn overtake_zones(&self) -> Vec<&Corner> {
        self.corners.iter().filter(|c| c.overtake_zone).collect()
    }

    /// The single most dangerous corner (risk × run-off severity).
    pub fn worst_corner(&self) -> Option<&Corner> {
        self.corners
            .iter()
            .max_by(|a, b| {
                (a.base_risk * a.runoff.severity())
                    .partial_cmp(&(b.base_risk * b.runoff.severity()))
                    .unwrap()
            })
    }
}

/// Monaco — the real one. Tight, walled, almost no run-off: a crash circuit.
pub fn monaco() -> Track {
    use CornerKind::*;
    use RunOff::*;
    Track {
        name: "Circuit de Monaco".into(),
        country: "Monaco".into(),
        length_km: 3.337,
        sectors: 3,
        surface_grip: 0.88,
        danger: 0.95,
        custom: false,
        corners: vec![
            Corner::new(1, "Sainte Dévote", Slow, Barrier, 0.18, 5, true).straight(290.0),
            Corner::new(2, "Beau Rivage", Flatout, Wall, 0.04, 30, false).straight(120.0),
            Corner::new(3, "Massenet", Fast, Wall, 0.10, 8, false).straight(150.0),
            Corner::new(4, "Casino", Medium, Wall, 0.09, -3, false).straight(60.0),
            Corner::new(5, "Mirabeau", Slow, Wall, 0.12, -10, true).straight(180.0),
            Corner::new(6, "Grand Hotel Hairpin", Hairpin, Wall, 0.15, -8, false).straight(90.0).radius(11.0),
            Corner::new(7, "Portier", Slow, Wall, 0.11, -5, false).straight(110.0),
            Corner::new(8, "Tunnel", Flatout, Wall, 0.06, 2, false).straight(250.0),
            Corner::new(9, "Nouvelle Chicane", Chicane, Barrier, 0.20, -2, true).straight(580.0),
            Corner::new(10, "Tabac", Fast, Wall, 0.10, 0, false).straight(120.0),
            Corner::new(11, "Piscine", Chicane, Barrier, 0.16, 0, false).straight(90.0),
            Corner::new(12, "La Rascasse", Slow, Wall, 0.13, 0, false).straight(70.0),
            Corner::new(13, "Anthony Noghès", Medium, Barrier, 0.12, 2, true).straight(100.0),
        ],
    }
}

/// A high-speed permanent track with big tarmac run-off — the safe contrast.
pub fn silverstone() -> Track {
    use CornerKind::*;
    use RunOff::*;
    Track {
        name: "Silverstone".into(),
        country: "Great Britain".into(),
        length_km: 5.891,
        sectors: 3,
        surface_grip: 0.97,
        danger: 0.55,
        custom: false,
        corners: vec![
            Corner::new(1, "Abbey", Fast, Gravel, 0.07, 0, false),
            Corner::new(2, "Farm", Fast, Tarmac, 0.05, -2, false),
            Corner::new(3, "Village", Slow, Gravel, 0.08, 0, true),
            Corner::new(4, "The Loop", Hairpin, Tarmac, 0.06, 0, false),
            Corner::new(5, "Aintree", Medium, Tarmac, 0.05, 0, false),
            Corner::new(6, "Brooklands", Medium, Gravel, 0.07, 0, true),
            Corner::new(7, "Luffield", Slow, Tarmac, 0.06, 0, false),
            Corner::new(8, "Copse", Fast, Gravel, 0.10, 0, false),
            Corner::new(9, "Maggotts-Becketts", Flatout, Gravel, 0.12, 0, false),
            Corner::new(10, "Stowe", Fast, Gravel, 0.09, -3, true),
            Corner::new(11, "Vale", Slow, Tarmac, 0.07, 0, true),
            Corner::new(12, "Club", Medium, Tarmac, 0.06, 0, false),
        ],
    }
}

/// **Custom map.** The Flux Ring — the fantasy circuit you drive after winning.
/// Permanent, flowing, forgiving tarmac: built for the drive-around-Flux mode.
pub fn flux_ring() -> Track {
    use CornerKind::*;
    use RunOff::*;
    Track {
        name: "Flux Ring".into(),
        country: "Quillon Graph".into(),
        length_km: 6.022,
        sectors: 4,
        surface_grip: 1.00,
        danger: 0.40,
        custom: true,
        corners: vec![
            Corner::new(1, "Genesis Hairpin", Hairpin, Tarmac, 0.05, 0, true),
            Corner::new(2, "Cranelift Esses", Chicane, Tarmac, 0.06, 4, false),
            Corner::new(3, "Sigil Sweep", Fast, Grass, 0.07, -6, false),
            Corner::new(4, "Cache Carousel", Medium, Tarmac, 0.05, 0, true),
            Corner::new(5, "Gossip Straightline", Flatout, Tarmac, 0.03, 12, false),
            Corner::new(6, "Provenance Parabolica", Fast, Gravel, 0.08, -4, true),
            Corner::new(7, "Swarm Switchback", Slow, Tarmac, 0.05, 0, false),
            Corner::new(8, "Epsilon Crest", Flatout, Grass, 0.06, 20, false),
            Corner::new(9, "Delta Dive", Medium, Tarmac, 0.06, -18, true),
        ],
    }
}

/// Procedurally generate a **custom map** from a seed — endless circuits beyond
/// the real calendar. Deterministic: the same seed yields the same track.
pub fn generate_custom(name: &str, seed: u64, corner_count: u8) -> Track {
    use CornerKind::*;
    use RunOff::*;
    let mut st = seed.max(1);
    let kinds = [Hairpin, Slow, Medium, Fast, Flatout, Chicane];
    let runoffs = [Tarmac, Gravel, Grass, Barrier, Wall];
    let mut corners = Vec::new();
    let mut total_risk = 0.0;
    for i in 0..corner_count.max(4) {
        let kind = kinds[(rand01(&mut st) * kinds.len() as f64) as usize % kinds.len()];
        let runoff = runoffs[(rand01(&mut st) * runoffs.len() as f64) as usize % runoffs.len()];
        let base_risk = 0.04 + rand01(&mut st) * 0.16;
        total_risk += base_risk;
        let elev = ((rand01(&mut st) - 0.5) * 40.0) as i16;
        let overtake = rand01(&mut st) > 0.65;
        corners.push(Corner::new(i + 1, &format!("Turn {}", i + 1), kind, runoff, base_risk, elev, overtake));
    }
    let danger = (total_risk / corners.len() as f64 * 5.0).clamp(0.2, 1.0);
    Track {
        name: name.to_string(),
        country: "Custom".into(),
        length_km: 3.0 + rand01(&mut st) * 4.0,
        sectors: 3,
        surface_grip: 0.85 + rand01(&mut st) * 0.15,
        danger,
        custom: true,
        corners,
    }
}

/// The full catalogue: real circuits + the built-in custom maps.
pub fn catalog() -> Vec<Track> {
    vec![monaco(), silverstone(), flux_ring()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_track_fits_1m_context() {
        for t in catalog() {
            assert!(t.fits_context(), "{} must fit a 1M context ({} tok)", t.name, t.context_tokens());
            // sanity: real tracks are small — a few thousand tokens at most.
            assert!(t.context_tokens() < 20_000);
        }
    }

    #[test]
    fn monaco_is_a_wall_lined_danger_circuit() {
        let m = monaco();
        assert!(m.danger > 0.9);
        let worst = m.worst_corner().unwrap();
        // Monaco's worst corner sits behind a wall or barrier.
        assert!(matches!(worst.runoff, RunOff::Wall | RunOff::Barrier));
        assert!(!m.overtake_zones().is_empty());
    }

    #[test]
    fn context_brief_lists_all_corners() {
        let m = monaco();
        let ctx = m.to_context();
        for c in &m.corners {
            assert!(ctx.contains(&c.name), "brief must mention {}", c.name);
        }
    }

    #[test]
    fn custom_map_is_deterministic_and_bounded() {
        let a = generate_custom("Test Ring", 777, 14);
        let b = generate_custom("Test Ring", 777, 14);
        assert_eq!(a, b);
        assert!(a.custom);
        assert!(a.fits_context());
        assert_eq!(a.corners.len(), 14);
    }

    #[test]
    fn flux_ring_is_a_safe_custom_drive_around_track() {
        let f = flux_ring();
        assert!(f.custom);
        assert!(f.danger < 0.5);
    }
}
