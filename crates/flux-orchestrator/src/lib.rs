//! flux-orchestrator — **conduct an AI-agent ensemble like a symphony.**
//!
//! This is the working realization of the "direction" idea (a conductor for the
//! agentic build that was intended but never came to work).
//!
//! - **Instruments** are agents/models (Claude Code, Codex, Qwen, Grok, Gemini
//!   CLI, Cursor), grouped into [`Section`]s.
//! - The **flux MCP combo** (`flux_combo` = build+test+predict) plays each part;
//!   one run becomes a [`PlayResult`] (did the instrument hit its notes, how
//!   fast, and what artifact did it produce).
//! - The orchestrator **measures whether the music is coherent** —
//!   *in tune* (everyone passed), *in time* (within tempo), *in unison*
//!   (instruments agree on the same artifact hash) — and folds that into a
//!   single [`Coherence`] verdict with a `harmony` score.
//! - A [`Conductor`] (the user **or** the AI) drives the whole thing with one
//!   call, [`Orchestra::conduct`], which also returns the next [`Direction`]s —
//!   solo the weak instrument, rest a broken one, retune one that's off — so the
//!   AI can "styre slagets gang" by a single button press.
//!
//! Pure logic, blake3 + serde only — no consensus/balance/crypto.

#![warn(missing_docs)]

use serde::{Deserialize, Serialize};

/// 32-byte BLAKE3 digest (an artifact / "note" fingerprint).
pub type Hash = [u8; 32];

/// Orchestra sections — a flavorful grouping of agent families.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Section {
    /// Lead, melodic agents (e.g. Claude Code).
    Strings,
    /// Bold, structural agents (e.g. Codex).
    Brass,
    /// Nimble, exploratory agents (e.g. Qwen, Gemini CLI).
    Woodwind,
    /// Timekeepers / harness agents (e.g. Cursor, Grok).
    Percussion,
}

/// One instrument in the orchestra = one agent and its capability ("voice").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Instrument {
    /// Agent name (e.g. `"Claude Code"`).
    pub name: String,
    /// Which section it plays in.
    pub section: Section,
    /// Capability 0.0..=1.0 — how strong this voice is.
    pub voice: f64,
}

impl Instrument {
    /// Build an instrument.
    pub fn new(name: &str, section: Section, voice: f64) -> Self {
        Self { name: name.to_string(), section, voice: voice.clamp(0.0, 1.0) }
    }
}

/// The result of one instrument **playing its part** — fed by a real flux MCP
/// combo run (`flux_combo` on the crate/task the agent was handed).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayResult {
    /// Which instrument played.
    pub name: String,
    /// Did it hit its notes? (combo green: build ✓ + tests pass)
    pub passed: bool,
    /// How long the part took, in ms (for the *in-time* check).
    pub ms: u64,
    /// BLAKE3 of the artifact it produced (for the *in-unison* check — agents
    /// playing the same part should converge on the same note).
    pub artifact: Hash,
}

impl PlayResult {
    /// Convenience: a result whose artifact is `H(tag)`.
    pub fn of(name: &str, passed: bool, ms: u64, tag: &str) -> Self {
        Self { name: name.to_string(), passed, ms, artifact: *blake3::hash(tag.as_bytes()).as_bytes() }
    }
}

/// The beat the conductor keeps: parts should land near `target_ms`, within
/// `tolerance_ms`, and not drift apart by more than `2 * tolerance_ms`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tempo {
    /// Target part duration (the downbeat) in ms.
    pub target_ms: u64,
    /// Allowed slack around the target, in ms.
    pub tolerance_ms: u64,
}

impl Default for Tempo {
    fn default() -> Self {
        Self { target_ms: 15_000, tolerance_ms: 8_000 }
    }
}

/// Who holds the baton.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Conductor {
    /// A human pressed the button.
    User,
    /// The AI is conducting autonomously.
    Ai,
}

/// A conductor's instruction for the next bar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    /// Everyone plays together.
    Tutti,
    /// Spotlight one instrument (the strongest, when the ensemble is muddy).
    Solo(String),
    /// Mute an instrument that broke the harmony (its part failed).
    Rest(String),
    /// Re-tune an instrument that's playing a different note (out of unison).
    Retune(String),
}

/// The measured verdict — **is the music coherent?**
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Coherence {
    /// 0.0..=1.0 overall — `0.5*in_tune + 0.3*unison + 0.2*in_time`.
    pub harmony: f64,
    /// Everyone hit their notes (all parts passed).
    pub in_tune: bool,
    /// All parts landed within tempo (and didn't drift apart).
    pub in_time: bool,
    /// The passing instruments agree on the same artifact (largest cluster = all).
    pub in_unison: bool,
    /// The headline: coherent ⇔ `harmony >= 0.8 && in_tune`.
    pub coherent: bool,
    /// How many instruments passed.
    pub passed: usize,
    /// How many played.
    pub total: usize,
    /// The conductor's next instructions (what to do to fix/keep the music).
    pub directions: Vec<Direction>,
    /// Human-readable performance notes.
    pub notes: Vec<String>,
}

/// The ensemble.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Orchestra {
    /// The instruments, in seating order.
    pub instruments: Vec<Instrument>,
}

impl Orchestra {
    /// Empty orchestra.
    pub fn new() -> Self {
        Self { instruments: Vec::new() }
    }

    /// Seat an instrument.
    pub fn seat(mut self, i: Instrument) -> Self {
        self.instruments.push(i);
        self
    }

    /// The canonical Flux agent ensemble.
    pub fn flux_default() -> Self {
        Orchestra::new()
            .seat(Instrument::new("Claude Code", Section::Strings, 0.95))
            .seat(Instrument::new("Codex", Section::Brass, 0.88))
            .seat(Instrument::new("Qwen", Section::Woodwind, 0.80))
            .seat(Instrument::new("Gemini CLI", Section::Woodwind, 0.82))
            .seat(Instrument::new("Grok", Section::Percussion, 0.78))
            .seat(Instrument::new("Cursor", Section::Percussion, 0.84))
    }

    /// **Solo** check: did this one instrument carry its part alone?
    pub fn solo(&self, results: &[PlayResult], name: &str) -> bool {
        results.iter().find(|r| r.name == name).map(|r| r.passed).unwrap_or(false)
    }

    /// The strongest seated instrument (highest voice) — the natural soloist.
    pub fn principal(&self) -> Option<&Instrument> {
        self.instruments.iter().max_by(|a, b| a.voice.partial_cmp(&b.voice).unwrap_or(std::cmp::Ordering::Equal))
    }

    /// **Conduct one performance** — the one-button call. Folds the parts into a
    /// [`Coherence`] verdict and (if the AI holds the baton) emits the next
    /// [`Direction`]s to keep the music coherent.
    pub fn conduct(&self, results: &[PlayResult], tempo: Tempo, who: Conductor) -> Coherence {
        let total = results.len();
        let passed = results.iter().filter(|r| r.passed).count();
        let mut notes = Vec::new();

        // in tune: everyone hit their notes
        let in_tune = total > 0 && passed == total;

        // in time: every part within [—, target+tol] and spread <= 2*tol
        let (in_time, max_ms, spread) = if results.is_empty() {
            (false, 0, 0)
        } else {
            let max = results.iter().map(|r| r.ms).max().unwrap();
            let min = results.iter().map(|r| r.ms).min().unwrap();
            let spread = max - min;
            let ok = max <= tempo.target_ms.saturating_add(tempo.tolerance_ms)
                && spread <= tempo.tolerance_ms.saturating_mul(2);
            (ok, max, spread)
        };

        // in unison: the passing instruments agree on one artifact (largest cluster)
        let mut clusters: std::collections::BTreeMap<Hash, usize> = std::collections::BTreeMap::new();
        for r in results.iter().filter(|r| r.passed) {
            *clusters.entry(r.artifact).or_insert(0) += 1;
        }
        let biggest = clusters.values().copied().max().unwrap_or(0);
        let unison_score = if passed == 0 { 0.0 } else { biggest as f64 / passed as f64 };
        let in_unison = passed > 0 && biggest == passed;

        let tune_score = if total == 0 { 0.0 } else { passed as f64 / total as f64 };
        let time_score = if in_time { 1.0 } else { 0.0 };
        let harmony = 0.5 * tune_score + 0.3 * unison_score + 0.2 * time_score;
        // truly coherent ⇒ everyone in tune AND agreeing (unison) AND on tempo.
        let coherent = harmony >= 0.8 && in_tune && in_unison;

        if !in_tune {
            notes.push(format!("{}/{} instruments off — not in tune", total - passed, total));
        }
        if !in_unison && passed > 1 {
            notes.push(format!("{} distinct artifacts among {} players — not in unison", clusters.len(), passed));
        }
        if !in_time {
            notes.push(format!("slowest part {}ms (spread {}ms) — out of tempo", max_ms, spread));
        }
        if coherent {
            notes.push("the music is coherent ✓".into());
        }

        // The AI conductor's next directions (the baton).
        let mut directions = Vec::new();
        if matches!(who, Conductor::Ai) {
            for r in results.iter().filter(|r| !r.passed) {
                directions.push(Direction::Rest(r.name.clone())); // mute broken parts
            }
            if !in_unison && passed > 1 {
                // retune the minority voices to the majority note
                let majority = clusters.iter().max_by_key(|(_, &c)| c).map(|(h, _)| *h);
                for r in results.iter().filter(|r| r.passed) {
                    if Some(r.artifact) != majority {
                        directions.push(Direction::Retune(r.name.clone()));
                    }
                }
            }
            if directions.is_empty() {
                directions.push(if coherent { Direction::Tutti } else {
                    self.principal().map(|p| Direction::Solo(p.name.clone())).unwrap_or(Direction::Tutti)
                });
            }
        }

        Coherence { harmony, in_tune, in_time, in_unison, coherent, passed, total, directions, notes }
    }
}

/// Harmony at which a bar counts as a **climax** — the music swells to a peak
/// of full, in-unison, in-time coherence.
pub const CLIMAX_HARMONY: f64 = 0.95;

/// One bar of music = one ensemble round (each instrument's part this bar).
pub type Bar = Vec<PlayResult>;

/// A track / number — an ordered sequence of [`Bar`]s (a whole piece).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Track {
    /// The track's name/number.
    pub name: String,
    /// Its bars, in order.
    pub bars: Vec<Bar>,
}

impl Track {
    /// Start an empty track.
    pub fn new(name: &str) -> Self {
        Self { name: name.to_string(), bars: Vec::new() }
    }
    /// Append a bar.
    pub fn bar(mut self, b: Bar) -> Self {
        self.bars.push(b);
        self
    }
}

/// The result of performing a whole [`Track`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Performance {
    /// Track name.
    pub track: String,
    /// Per-bar coherence verdict.
    pub bars: Vec<Coherence>,
    /// Indices of bars that reached **climax**.
    pub climaxes: Vec<usize>,
    /// Highest harmony reached anywhere in the track.
    pub peak_harmony: f64,
    /// Fraction of bars that were coherent.
    pub coherent_ratio: f64,
    /// The requirement: **climax reached many times** during the track
    /// (≥ 2 climaxes AND ≥ 30% of the bars).
    pub many_climaxes: bool,
    /// Bars that were an **aha moment** — harmony jumped up sharply vs the
    /// previous bar, or the track hit a brand-new peak (a breakthrough).
    pub aha_moments: Vec<usize>,
    /// **Progress** 0.0..=1.0 — net forward motion across the track
    /// (how much higher it ends/peaks vs where it began, blended with the
    /// share of bars that improved on the one before).
    pub progress: f64,
}

/// Harmony rise (vs the previous bar) that counts as an **aha moment**.
pub const AHA_DELTA: f64 = 0.15;

impl Orchestra {
    /// **Perform a whole track** — conduct every bar, then check the shape:
    /// did the music reach climax *many times* (Viktor's rule), not just once?
    pub fn perform(&self, track: &Track, tempo: Tempo, who: Conductor) -> Performance {
        let bars: Vec<Coherence> = track.bars.iter().map(|b| self.conduct(b, tempo, who)).collect();
        let climaxes: Vec<usize> = bars
            .iter()
            .enumerate()
            .filter(|(_, c)| c.coherent && c.harmony >= CLIMAX_HARMONY)
            .map(|(i, _)| i)
            .collect();
        let peak_harmony = bars.iter().map(|c| c.harmony).fold(0.0_f64, f64::max);
        let coherent_ratio = if bars.is_empty() {
            0.0
        } else {
            bars.iter().filter(|c| c.coherent).count() as f64 / bars.len() as f64
        };
        let many_climaxes = climaxes.len() >= 2 && climaxes.len() * 100 >= bars.len() * 30;

        // aha moments + progress: walk the harmony curve bar-by-bar.
        let mut aha_moments = Vec::new();
        let mut running_peak = 0.0_f64;
        let mut rises = 0usize;
        for (i, c) in bars.iter().enumerate() {
            let prev = if i == 0 { 0.0 } else { bars[i - 1].harmony };
            if i > 0 && c.harmony > prev {
                rises += 1;
            }
            let jumped = i > 0 && (c.harmony - prev) >= AHA_DELTA;
            let new_peak = c.harmony > running_peak + 1e-9 && i > 0;
            if jumped || new_peak {
                aha_moments.push(i);
            }
            running_peak = running_peak.max(c.harmony);
        }
        let net = if bars.len() < 2 {
            bars.first().map(|c| c.harmony).unwrap_or(0.0)
        } else {
            let first = bars.first().unwrap().harmony;
            (peak_harmony - first).max(bars.last().unwrap().harmony - first)
        };
        let rising_share = if bars.len() < 2 { 0.0 } else { rises as f64 / (bars.len() - 1) as f64 };
        let progress = (0.6 * net.clamp(0.0, 1.0) + 0.4 * rising_share).clamp(0.0, 1.0);

        Performance {
            track: track.name.clone(),
            bars,
            climaxes,
            peak_harmony,
            coherent_ratio,
            many_climaxes,
            aha_moments,
            progress,
        }
    }
}

fn hex8(h: &Hash) -> String {
    h[..4].iter().map(|b| format!("{:02x}", b)).collect()
}

/// Render a performance as JSON for a "Conduct" button on the desktop / qwen app.
pub fn concert_status_json(orch: &Orchestra, results: &[PlayResult], c: &Coherence) -> String {
    let players: Vec<String> = results
        .iter()
        .map(|r| {
            format!(
                r#"{{"name":"{}","passed":{},"ms":{},"note":"{}"}}"#,
                r.name, r.passed, r.ms, hex8(&r.artifact)
            )
        })
        .collect();
    let dirs: Vec<String> = c
        .directions
        .iter()
        .map(|d| match d {
            Direction::Tutti => "\"tutti\"".to_string(),
            Direction::Solo(n) => format!("\"solo:{}\"", n),
            Direction::Rest(n) => format!("\"rest:{}\"", n),
            Direction::Retune(n) => format!("\"retune:{}\"", n),
        })
        .collect();
    format!(
        r#"{{"coherent":{},"harmony":{:.3},"in_tune":{},"in_time":{},"in_unison":{},"passed":{},"total":{},"seats":{},"players":[{}],"directions":[{}]}}"#,
        c.coherent,
        c.harmony,
        c.in_tune,
        c.in_time,
        c.in_unison,
        c.passed,
        c.total,
        orch.instruments.len(),
        players.join(","),
        dirs.join(",")
    )
}

/// Render a whole [`Performance`] as JSON for the desktop / qwen "Conduct"
/// button — the harmony curve, the climaxes, the aha-moments, and progress.
pub fn performance_status_json(p: &Performance) -> String {
    let curve: Vec<String> = p.bars.iter().map(|c| format!("{:.3}", c.harmony)).collect();
    let cohere: Vec<String> = p.bars.iter().map(|c| c.coherent.to_string()).collect();
    let clx: Vec<String> = p.climaxes.iter().map(|i| i.to_string()).collect();
    let aha: Vec<String> = p.aha_moments.iter().map(|i| i.to_string()).collect();
    format!(
        r#"{{"track":"{}","bars":{},"peak_harmony":{:.3},"coherent_ratio":{:.3},"climaxes":[{}],"many_climaxes":{},"aha_moments":[{}],"progress":{:.3},"harmony_curve":[{}],"coherent":[{}]}}"#,
        p.track,
        p.bars.len(),
        p.peak_harmony,
        p.coherent_ratio,
        clx.join(","),
        p.many_climaxes,
        aha.join(","),
        p.progress,
        curve.join(","),
        cohere.join(",")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coherent_when_all_pass_in_unison_in_time() {
        let o = Orchestra::flux_default();
        // everyone passes, same artifact, tight timing
        let r: Vec<PlayResult> = ["Claude Code", "Codex", "Qwen", "Gemini CLI", "Grok", "Cursor"]
            .iter()
            .map(|n| PlayResult::of(n, true, 12_000, "same-note"))
            .collect();
        let c = o.conduct(&r, Tempo::default(), Conductor::Ai);
        assert!(c.in_tune && c.in_unison && c.in_time);
        assert!(c.coherent);
        assert!((c.harmony - 1.0).abs() < 1e-9);
        assert_eq!(c.directions, vec![Direction::Tutti]);
    }

    #[test]
    fn one_broken_part_breaks_coherence_and_gets_rested() {
        let o = Orchestra::flux_default();
        let mut r: Vec<PlayResult> =
            ["Claude Code", "Codex", "Qwen"].iter().map(|n| PlayResult::of(n, true, 12_000, "x")).collect();
        r.push(PlayResult::of("Grok", false, 9_000, "x")); // Grok dropped its part
        let c = o.conduct(&r, Tempo::default(), Conductor::Ai);
        assert!(!c.coherent);
        assert!(!c.in_tune);
        assert!(c.directions.contains(&Direction::Rest("Grok".to_string())));
    }

    #[test]
    fn disagreement_is_not_in_unison_and_triggers_retune() {
        let o = Orchestra::flux_default();
        let r = vec![
            PlayResult::of("Claude Code", true, 12_000, "note-A"),
            PlayResult::of("Codex", true, 12_500, "note-A"),
            PlayResult::of("Qwen", true, 13_000, "note-B"), // off pitch
        ];
        let c = o.conduct(&r, Tempo::default(), Conductor::Ai);
        assert!(c.in_tune); // all passed
        assert!(!c.in_unison); // but disagree
        assert!(!c.coherent);
        assert!(c.directions.contains(&Direction::Retune("Qwen".to_string())));
    }

    #[test]
    fn out_of_tempo_lowers_harmony() {
        let o = Orchestra::flux_default();
        let r = vec![
            PlayResult::of("Claude Code", true, 1_000, "x"),
            PlayResult::of("Codex", true, 90_000, "x"), // way late
        ];
        let c = o.conduct(&r, Tempo::default(), Conductor::Ai);
        assert!(!c.in_time);
        assert!(c.harmony < 1.0);
    }

    #[test]
    fn user_conductor_emits_no_auto_directions() {
        let o = Orchestra::flux_default();
        let r = vec![PlayResult::of("Qwen", false, 5_000, "x")];
        let c = o.conduct(&r, Tempo::default(), Conductor::User);
        assert!(c.directions.is_empty()); // the human holds the baton
    }

    #[test]
    fn principal_is_strongest_voice() {
        let o = Orchestra::flux_default();
        assert_eq!(o.principal().unwrap().name, "Claude Code");
    }

    #[test]
    fn track_reaches_climax_many_times_with_aha_and_progress() {
        let o = Orchestra::flux_default();
        let players = ["Claude Code", "Codex", "Qwen"];
        let climax_bar = || -> Bar { players.iter().map(|n| PlayResult::of(n, true, 12_000, "unison")).collect() };
        let muddy_bar = || -> Bar {
            vec![
                PlayResult::of("Claude Code", true, 12_000, "a"),
                PlayResult::of("Codex", false, 40_000, "b"), // broken + late
                PlayResult::of("Qwen", true, 13_000, "c"),   // off pitch
            ]
        };
        // rises and falls, peaking (climax) several times — a real "number"
        let track = Track::new("opus-1")
            .bar(muddy_bar())
            .bar(climax_bar())
            .bar(muddy_bar())
            .bar(climax_bar())
            .bar(climax_bar());
        let perf = o.perform(&track, Tempo::default(), Conductor::Ai);
        assert!(perf.climaxes.len() >= 2, "climax must be reached many times");
        assert!(perf.many_climaxes, "track should qualify as many-climaxes");
        assert!(!perf.aha_moments.is_empty(), "muddy→climax transitions are aha moments");
        assert!(perf.progress > 0.0, "the number should show progress");
        assert!((perf.peak_harmony - 1.0).abs() < 1e-9);
        let j = performance_status_json(&perf);
        assert!(j.contains("\"many_climaxes\":true"));
        assert!(j.contains("\"harmony_curve\""));
    }

    #[test]
    fn status_json_is_wellformed() {
        let o = Orchestra::flux_default();
        let r = vec![PlayResult::of("Qwen", true, 12_000, "x")];
        let c = o.conduct(&r, Tempo::default(), Conductor::Ai);
        let j = concert_status_json(&o, &r, &c);
        assert!(j.contains("\"coherent\":"));
        assert!(j.contains("\"harmony\":"));
        assert!(j.starts_with('{') && j.ends_with('}'));
    }
}
