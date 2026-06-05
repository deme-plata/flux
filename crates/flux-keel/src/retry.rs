//! Retry with exponential backoff + jitter (anti-thundering-herd).

/// Backoff schedule.
#[derive(Clone, Copy, Debug)]
pub struct Backoff {
    /// First delay.
    pub base_ms: u64,
    /// Cap.
    pub max_ms: u64,
    /// Growth factor per attempt.
    pub factor: u64,
}

impl Backoff {
    /// Sensible default: 50ms → ×2 → 5s cap.
    pub fn default_fast() -> Self { Backoff { base_ms: 50, max_ms: 5_000, factor: 2 } }
    /// Delay before attempt `attempt` (0-indexed), with `jitter01` in [0,1) the
    /// fraction of the delay to randomly shave (full jitter, decorrelates herds).
    pub fn delay_ms(&self, attempt: u32, jitter01: f64) -> u64 {
        let raw = self.base_ms.saturating_mul(self.factor.saturating_pow(attempt)).min(self.max_ms);
        let j = (raw as f64 * jitter01.clamp(0.0, 1.0)) as u64;
        raw.saturating_sub(j)
    }
}

/// Run `f` up to `max_attempts`, returning Ok on first success or the last Err.
/// `sleep_ms` is injected (real sleep on backend, a no-op/awaitable on frontend)
/// so this stays runtime-agnostic + testable.
pub fn retry<T, E>(
    max_attempts: u32,
    backoff: Backoff,
    mut sleep_ms: impl FnMut(u64),
    mut f: impl FnMut() -> Result<T, E>,
) -> Result<T, E> {
    let mut last = None;
    for attempt in 0..max_attempts.max(1) {
        match f() {
            Ok(v) => return Ok(v),
            Err(e) => {
                last = Some(e);
                if attempt + 1 < max_attempts {
                    sleep_ms(backoff.delay_ms(attempt, 0.5));
                }
            }
        }
    }
    Err(last.expect("at least one attempt"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn backoff_grows_and_caps() {
        let b = Backoff { base_ms: 100, max_ms: 1000, factor: 2 };
        assert_eq!(b.delay_ms(0, 0.0), 100);
        assert_eq!(b.delay_ms(1, 0.0), 200);
        assert_eq!(b.delay_ms(2, 0.0), 400);
        assert_eq!(b.delay_ms(10, 0.0), 1000); // capped
        assert!(b.delay_ms(2, 0.5) < 400);     // jitter shaves
    }
    #[test]
    fn retries_until_success() {
        let mut n = 0;
        let slept = std::cell::RefCell::new(0u64);
        let r: Result<&str, &str> = retry(5, Backoff::default_fast(), |ms| *slept.borrow_mut() += ms, || {
            n += 1; if n >= 3 { Ok("ok") } else { Err("fail") }
        });
        assert_eq!(r, Ok("ok"));
        assert_eq!(n, 3);
        assert!(*slept.borrow() > 0); // backed off between tries
    }
}
