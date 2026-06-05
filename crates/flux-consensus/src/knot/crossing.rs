//! Knot crossings — positive/negative diagrams over input/output strands.
//!
//! Lifted from `QTFT/blockchain/knot.rs` (2026-05-29). No semantic changes;
//! `weight: f64` keeps its meaning (transaction amount in monetary units;
//! generic in this Flux-consensus port).

use serde::{Deserialize, Serialize};

/// Knot crossing sign — positive (right-hand rule) or negative (left-hand rule).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CrossingType {
    /// Positive crossing.
    Positive,
    /// Negative crossing.
    Negative,
}

impl CrossingType {
    /// `+1` for positive, `-1` for negative — used in writhe / linking-number sums.
    pub const fn sign(&self) -> i8 {
        match self {
            CrossingType::Positive => 1,
            CrossingType::Negative => -1,
        }
    }
}

/// A single crossing point in a knot diagram.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Crossing {
    /// Index of the over-crossing strand in the parent diagram.
    pub over_strand: usize,
    /// Index of the under-crossing strand.
    pub under_strand: usize,
    /// Positive or negative crossing.
    pub crossing_type: CrossingType,
    /// Weight at this crossing — semantically arbitrary; callers map their own
    /// quantity (transaction amount, fee, edge weight, etc).
    pub weight: f64,
}

/// A strand in a knot diagram — a directed segment in 3D with two endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnotStrand {
    /// Strand identifier — index into the parent diagram's strand list.
    pub id: usize,
    /// Start position in 3D ambient space.
    pub start: [f64; 3],
    /// End position in 3D ambient space.
    pub end: [f64; 3],
    /// Whether this strand carries an input (true) or output (false).
    pub is_input: bool,
    /// Weight along the strand — semantically arbitrary (see `Crossing::weight`).
    pub weight: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs_are_plus_minus_one() {
        assert_eq!(CrossingType::Positive.sign(), 1);
        assert_eq!(CrossingType::Negative.sign(), -1);
    }

    #[test]
    fn crossings_round_trip_through_json() {
        let c = Crossing {
            over_strand: 0,
            under_strand: 1,
            crossing_type: CrossingType::Positive,
            weight: 100.0,
        };
        let j = serde_json::to_string(&c).unwrap();
        let back: Crossing = serde_json::from_str(&j).unwrap();
        assert_eq!(back.over_strand, c.over_strand);
        assert_eq!(back.crossing_type, c.crossing_type);
    }
}
