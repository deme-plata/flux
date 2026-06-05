//! gore.rs — blood & gore overlay for the Ashlands. A dark-fantasy ARPG should *bleed*.
//!
//! A [`BloodField`] is a decaying particle layer: hits **spill** droplets (count ∝ damage), kills
//! **gib** into a heavy splatter + a visceral one-liner. It's std-only and **deterministic** (an
//! internal splitmix64 PRNG seeded by a tick counter), so tests are reproducible and the look is the
//! same on every machine. Renderers overlay the blood cells in red ANSI under the entities.
//!
//! Used by `bossfight.rs` (and wireable into `rt.rs`) — kept standalone so any arena can bleed.

use crate::V3;

// ── ANSI (renderers use these to paint the overlay) ─────────────────
pub const RED: &str = "\x1b[38;5;196m";
pub const DARK_RED: &str = "\x1b[38;5;88m";
pub const CRIMSON: &str = "\x1b[38;5;124m";
pub const RESET: &str = "\x1b[0m";

/// Light droplet glyphs (fresh spray) and heavy ones (pooling viscera).
const DROPLETS: [char; 7] = ['`', '.', ',', '\'', ':', ';', '"'];
const HEAVY: [char; 4] = ['*', 'x', '%', '#'];

/// Gore one-liners, surfaced on a kill (dismemberment flavour for the feed).
pub const GORE_LINES: [&str; 10] = [
    "viscera paints the ash black-red.",
    "a limb cartwheels into the dark.",
    "it comes apart in a wet thunderclap.",
    "the spine gives with a green-stick crack.",
    "hot ichor sluices across the stone.",
    "ribs splay like a broken cage.",
    "the skull folds; the ash drinks deep.",
    "a fan of gore arcs and patters down.",
    "sinew snaps, and the thing unspools.",
    "it bursts — a red bloom on grey ground.",
];

/// One blood particle: where it landed, its glyph, how heavy, and how long it lingers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Splat {
    pub pos: V3,
    pub ch: char,
    pub ttl: u8,
    pub heavy: bool,
}

/// The decaying blood layer.
#[derive(Debug, Clone)]
pub struct BloodField {
    splats: Vec<Splat>,
    rng: u64,
    spilled: u64, // running total of droplets ever spawned (telemetry / determinism seed)
}

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E3779B97F4A7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

impl BloodField {
    pub fn new() -> BloodField {
        BloodField { splats: Vec::new(), rng: 0xA5A5_5A5A_DEAD_BEEF, spilled: 0 }
    }

    fn next(&mut self) -> u64 {
        self.rng = splitmix(self.rng);
        self.rng
    }

    /// Age the field one tick; old spray dries and disappears. Heavy pools linger longer.
    pub fn tick(&mut self) {
        for s in &mut self.splats {
            if s.ttl > 0 {
                s.ttl -= 1;
            }
        }
        self.splats.retain(|s| s.ttl > 0);
        // cap the field so a long fight never unbounds memory
        if self.splats.len() > 256 {
            let drop = self.splats.len() - 256;
            self.splats.drain(0..drop);
        }
    }

    /// Spill blood near `at`, scaled by `dmg`. Droplets scatter to neighbouring tiles.
    pub fn spill(&mut self, at: V3, dmg: i32) {
        let drops = (dmg / 5).clamp(1, 6) as usize;
        for _ in 0..drops {
            let r = self.next();
            let dx = (r % 3) as i32 - 1;
            let dy = ((r >> 2) % 3) as i32 - 1;
            let ch = DROPLETS[(r >> 4) as usize % DROPLETS.len()];
            let ttl = 10 + (r >> 8) as u8 % 14;
            self.splats.push(Splat { pos: V3::new(at.x + dx, at.y + dy, at.z), ch, ttl, heavy: false });
            self.spilled += 1;
        }
    }

    /// A kill: dump a heavy splatter at `at` and return a gore line for the feed.
    pub fn gib(&mut self, at: V3) -> &'static str {
        for dx in -1..=1 {
            for dy in -1..=1 {
                let r = self.next();
                let ch = HEAVY[(r >> 3) as usize % HEAVY.len()];
                let ttl = 26 + (r >> 7) as u8 % 20;
                self.splats.push(Splat { pos: V3::new(at.x + dx, at.y + dy, at.z), ch, ttl, heavy: true });
                self.spilled += 1;
            }
        }
        let pick = self.next() as usize % GORE_LINES.len();
        GORE_LINES[pick]
    }

    pub fn cells(&self) -> &[Splat] {
        &self.splats
    }
    pub fn is_empty(&self) -> bool {
        self.splats.is_empty()
    }
    pub fn total_spilled(&self) -> u64 {
        self.spilled
    }
}

impl Default for BloodField {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spill_scales_with_damage_and_decays() {
        let mut b = BloodField::new();
        b.spill(V3::new(4, 4, 0), 30); // big hit → up to 6 droplets
        assert!(!b.is_empty());
        let n0 = b.cells().len();
        assert!(n0 >= 1 && n0 <= 6);
        // they dry out eventually
        for _ in 0..40 {
            b.tick();
        }
        assert!(b.is_empty(), "spray dried up");
    }

    #[test]
    fn gib_makes_a_heavy_splatter_and_returns_a_line() {
        let mut b = BloodField::new();
        let line = b.gib(V3::new(3, 3, 0));
        assert!(GORE_LINES.contains(&line));
        assert!(b.cells().iter().any(|s| s.heavy), "gib left heavy viscera");
        assert!(b.cells().len() >= 9, "3x3 splatter");
    }

    #[test]
    fn deterministic_replay() {
        let mut a = BloodField::new();
        let mut b = BloodField::new();
        for _ in 0..5 {
            a.spill(V3::new(2, 2, 0), 12);
            b.spill(V3::new(2, 2, 0), 12);
        }
        assert_eq!(a.cells(), b.cells(), "same seed → identical gore");
    }

    #[test]
    fn field_is_capped() {
        let mut b = BloodField::new();
        for _ in 0..400 {
            b.gib(V3::new(4, 4, 0));
        }
        b.tick();
        assert!(b.cells().len() <= 256, "blood field stays bounded");
    }
}
