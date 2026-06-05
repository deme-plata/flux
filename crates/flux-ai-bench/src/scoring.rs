//! Per-task scoring primitives.

use serde::{Deserialize, Serialize};

/// A 0–10 score for a single task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Score(pub u8);

impl Score {
    pub const FAIL: Score = Score(0);
    pub const PASS: Score = Score(7);
    pub const PERFECT: Score = Score(10);

    pub fn from_u8(v: u8) -> Self {
        Self(v.min(10))
    }
}

/// Coarse outcome label. Useful for dashboarding without re-deriving from the
/// numeric score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskOutcome {
    Pass,
    PartialCredit,
    Fail,
    Skipped,
    Errored,
}

impl TaskOutcome {
    pub fn from_score(s: Score) -> Self {
        match s.0 {
            10 => TaskOutcome::Pass,
            7..=9 => TaskOutcome::Pass,
            1..=6 => TaskOutcome::PartialCredit,
            0 => TaskOutcome::Fail,
            _ => TaskOutcome::Errored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_clamps_at_10() {
        assert_eq!(Score::from_u8(42).0, 10);
        assert_eq!(Score::from_u8(0).0, 0);
        assert_eq!(Score::from_u8(7).0, 7);
    }

    #[test]
    fn outcome_thresholds() {
        assert_eq!(TaskOutcome::from_score(Score(0)), TaskOutcome::Fail);
        assert_eq!(TaskOutcome::from_score(Score(5)), TaskOutcome::PartialCredit);
        assert_eq!(TaskOutcome::from_score(Score(7)), TaskOutcome::Pass);
        assert_eq!(TaskOutcome::from_score(Score(10)), TaskOutcome::Pass);
    }
}
