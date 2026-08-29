//! The grand auditorium — where big thinkers come by, giving wisdom and food
//! for thought. Three contiguous floors (6–8), 1,200 seats.
//!
//! The scheduler's one hard promise: no two talks overlap, and no talk is
//! booked beyond the room. The `wisdom_index` is the room's output measure —
//! how much thinking actually happened, not how many events were announced.

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const GRAND_CAPACITY: u32 = 1_200;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Talk {
    pub id: String,
    pub speaker: String,
    pub title: String,
    pub start_tick: u64,
    pub duration_ticks: u64,
    pub expected_attendance: u32,
}

impl Talk {
    fn end_tick(&self) -> u64 {
        self.start_tick + self.duration_ticks
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum ScheduleError {
    #[error("talk {a} overlaps talk {b}")]
    Overlap { a: String, b: String },
    #[error("expected attendance {0} exceeds capacity {GRAND_CAPACITY}")]
    OverCapacity(u32),
    #[error("zero-duration talk")]
    ZeroDuration,
}

pub struct Auditorium {
    pub capacity: u32,
    schedule: Vec<Talk>,
    /// (talk id, actual attendance) — recorded after each session.
    held: Vec<(String, u32)>,
}

impl Auditorium {
    pub fn grand() -> Self {
        Self { capacity: GRAND_CAPACITY, schedule: Vec::new(), held: Vec::new() }
    }

    pub fn schedule(&mut self, talk: Talk) -> Result<(), ScheduleError> {
        if talk.duration_ticks == 0 {
            return Err(ScheduleError::ZeroDuration);
        }
        if talk.expected_attendance > self.capacity {
            return Err(ScheduleError::OverCapacity(talk.expected_attendance));
        }
        if let Some(existing) = self
            .schedule
            .iter()
            .find(|t| talk.start_tick < t.end_tick() && t.start_tick < talk.end_tick())
        {
            return Err(ScheduleError::Overlap { a: talk.id, b: existing.id.clone() });
        }
        self.schedule.push(talk);
        Ok(())
    }

    /// Any talk whose window has fully passed is held; attendance recorded.
    pub fn hold_due_talks(&mut self, now: u64, attendance_fn: impl Fn(&Talk) -> u32) {
        let due: Vec<Talk> = self
            .schedule
            .iter()
            .filter(|t| t.end_tick() <= now)
            .cloned()
            .collect();
        self.schedule.retain(|t| t.end_tick() > now);
        for talk in due {
            let actual = attendance_fn(&talk).min(self.capacity);
            self.held.push((talk.id, actual));
        }
    }

    pub fn talks_held(&self) -> usize {
        self.held.len()
    }

    /// Mean fill ratio of held talks (0–1). An empty room teaches nobody;
    /// a full one is food for thought by the plateful.
    pub fn wisdom_index(&self) -> f64 {
        if self.held.is_empty() {
            return 0.0;
        }
        let sum: f64 = self.held.iter().map(|(_, a)| *a as f64 / self.capacity as f64).sum();
        sum / self.held.len() as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn talk(id: &str, start: u64, dur: u64) -> Talk {
        Talk {
            id: id.into(),
            speaker: "a big thinker".into(),
            title: "Food for Thought".into(),
            start_tick: start,
            duration_ticks: dur,
            expected_attendance: 800,
        }
    }

    #[test]
    fn overlap_is_rejected() {
        let mut a = Auditorium::grand();
        a.schedule(talk("t1", 100, 60)).unwrap();
        let err = a.schedule(talk("t2", 130, 60)).unwrap_err();
        assert!(matches!(err, ScheduleError::Overlap { .. }));
        // Back-to-back is fine: [100,160) then [160,220).
        a.schedule(talk("t3", 160, 60)).unwrap();
    }

    #[test]
    fn over_capacity_is_rejected() {
        let mut a = Auditorium::grand();
        let mut t = talk("big", 0, 60);
        t.expected_attendance = 5_000;
        assert_eq!(a.schedule(t), Err(ScheduleError::OverCapacity(5_000)));
    }

    #[test]
    fn wisdom_index_reflects_fill() {
        let mut a = Auditorium::grand();
        a.schedule(talk("t1", 0, 60)).unwrap();
        a.schedule(talk("t2", 60, 60)).unwrap();
        a.hold_due_talks(120, |t| if t.id == "t1" { 1_200 } else { 600 });
        assert_eq!(a.talks_held(), 2);
        // (1.0 + 0.5) / 2 = 0.75
        assert!((a.wisdom_index() - 0.75).abs() < 1e-9);
    }
}
