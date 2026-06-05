//! Circuit breaker — stop hammering a failing dependency; auto-recover.

/// Breaker state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CircuitState {
    /// Calls allowed.
    Closed,
    /// Calls blocked (the dependency is failing).
    Open,
    /// One probe allowed to test recovery.
    HalfOpen,
}

/// A circuit breaker. Trips Open after `threshold` consecutive failures; after
/// `recovery_ms` it goes HalfOpen (one probe); a probe success closes it, a
/// probe failure re-opens it. Time is injected via `now_ms`.
#[derive(Clone, Debug)]
pub struct CircuitBreaker {
    state: CircuitState,
    failures: u32,
    threshold: u32,
    recovery_ms: u64,
    opened_at: u64,
}

impl CircuitBreaker {
    /// New breaker: trip after `threshold` failures, probe after `recovery_ms`.
    pub fn new(threshold: u32, recovery_ms: u64) -> Self {
        CircuitBreaker { state: CircuitState::Closed, failures: 0, threshold: threshold.max(1), recovery_ms, opened_at: 0 }
    }
    /// May a call proceed now? (Open transitions to HalfOpen once recovery_ms passes.)
    pub fn allow(&mut self, now_ms: u64) -> bool {
        if self.state == CircuitState::Open && now_ms.saturating_sub(self.opened_at) >= self.recovery_ms {
            self.state = CircuitState::HalfOpen;
        }
        self.state != CircuitState::Open
    }
    /// Record a success.
    pub fn on_success(&mut self) {
        self.failures = 0;
        self.state = CircuitState::Closed;
    }
    /// Record a failure at `now_ms`.
    pub fn on_failure(&mut self, now_ms: u64) {
        if self.state == CircuitState::HalfOpen {
            self.trip(now_ms);
            return;
        }
        self.failures += 1;
        if self.failures >= self.threshold {
            self.trip(now_ms);
        }
    }
    fn trip(&mut self, now_ms: u64) {
        self.state = CircuitState::Open;
        self.opened_at = now_ms;
        self.failures = self.threshold;
    }
    /// Current state.
    pub fn state(&self) -> CircuitState { self.state }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trips_open_then_recovers() {
        let mut cb = CircuitBreaker::new(3, 1000);
        assert!(cb.allow(0));
        cb.on_failure(0); cb.on_failure(0); cb.on_failure(0);   // 3 fails → Open
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.allow(500));                                 // still open before recovery
        assert!(cb.allow(1000));                                 // recovery_ms passed → HalfOpen probe allowed
        assert_eq!(cb.state(), CircuitState::HalfOpen);
        cb.on_success();                                         // probe ok → Closed
        assert_eq!(cb.state(), CircuitState::Closed);
    }
    #[test]
    fn halfopen_failure_reopens() {
        let mut cb = CircuitBreaker::new(1, 100);
        cb.on_failure(0); assert_eq!(cb.state(), CircuitState::Open);
        assert!(cb.allow(100)); assert_eq!(cb.state(), CircuitState::HalfOpen);
        cb.on_failure(100); assert_eq!(cb.state(), CircuitState::Open); // probe failed → reopen
    }
}
