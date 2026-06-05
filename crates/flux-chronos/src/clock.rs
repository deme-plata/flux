//! Virtual clock.
//!
//! Time is whatever the universe says it is. Wall-clock has no entry point
//! into this module. Advance by explicit ticks; sleep is just a future-tick
//! schedule entry that gets serviced when the clock reaches that tick.
//!
//! Why micros and not nanos: u64 micros = ~580 years of simulated time, more
//! than enough for any chain scenario. u64 nanos = 584 years too but the
//! math is less ergonomic and we lose nothing by quantizing at microseconds.

use crate::SimDuration;

/// Monotonically-increasing tick id, in microseconds since the universe
/// began. Tick 0 = genesis moment; ticks only ever increase.
pub type TickId = u64;

/// The virtual clock. Owned by [`Universe`](crate::Universe); accessed
/// read-only by every node. The only mutation is via the universe's
/// `advance()` API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VirtualClock {
    /// Current tick. The clock only goes forward.
    now: TickId,
}

impl VirtualClock {
    /// Construct a fresh clock at tick 0.
    pub fn new() -> Self {
        Self { now: 0 }
    }

    /// Current tick.
    pub fn now(&self) -> TickId {
        self.now
    }

    /// Advance the clock by `delta` micros. Returns the new tick.
    ///
    /// `delta == 0` is a valid no-op — useful for "just process whatever
    /// messages are already in flight" without pushing the clock forward.
    pub(crate) fn advance(&mut self, delta: SimDuration) -> TickId {
        self.now = self.now.saturating_add(delta);
        self.now
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{hours, millis, secs};

    #[test]
    fn clock_starts_at_zero() {
        assert_eq!(VirtualClock::new().now(), 0);
    }

    #[test]
    fn advance_moves_forward_by_exact_micros() {
        let mut clock = VirtualClock::new();
        clock.advance(millis(500));
        assert_eq!(clock.now(), 500_000);
        clock.advance(secs(1));
        assert_eq!(clock.now(), 1_500_000);
        clock.advance(hours(1));
        assert_eq!(clock.now(), 1_500_000 + 3_600_000_000);
    }

    #[test]
    fn advance_by_zero_is_a_no_op() {
        let mut clock = VirtualClock::new();
        clock.advance(secs(10));
        let before = clock.now();
        clock.advance(0);
        assert_eq!(clock.now(), before);
    }

    #[test]
    fn advance_saturates_at_u64_max_instead_of_panicking() {
        // Defensive: a malicious scenario could call advance(u64::MAX) twice.
        // We'd rather pin at the ceiling than panic mid-test.
        let mut clock = VirtualClock::new();
        clock.advance(u64::MAX);
        clock.advance(1);
        assert_eq!(clock.now(), u64::MAX);
    }
}
