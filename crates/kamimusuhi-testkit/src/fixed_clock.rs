//! Deterministic clock fixtures for reproducible tests.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

use kamimusuhi_core::time::{MonotonicClock, MonotonicInstant, UtcTimestamp, WallClock};
use std::time::Duration;

/// A wall clock fixed to (or steppable from) a known instant. Every read
/// is deterministic; call [`FixedClock::advance`] to move it forward
/// explicitly rather than relying on wall time passing during a test run.
pub struct FixedClock {
    epoch_seconds: Mutex<i64>,
}

impl FixedClock {
    /// Starts the clock at the given RFC3339 instant.
    pub fn at(rfc3339: &str) -> Self {
        let ts = UtcTimestamp::parse_rfc3339(rfc3339).expect("valid fixture timestamp");
        FixedClock {
            epoch_seconds: Mutex::new(ts.as_offset_date_time().unix_timestamp()),
        }
    }

    pub fn advance(&self, duration: Duration) {
        let mut secs = self.epoch_seconds.lock().unwrap();
        *secs += duration.as_secs() as i64;
    }
}

impl WallClock for FixedClock {
    fn now_utc(&self) -> UtcTimestamp {
        let secs = *self.epoch_seconds.lock().unwrap();
        let dt = time::OffsetDateTime::from_unix_timestamp(secs).expect("in-range fixture time");
        UtcTimestamp::from_offset_date_time(dt)
    }
}

/// A monotonic clock fixture that advances by a fixed step on every read,
/// so ordering assertions in tests are deterministic without sleeping.
pub struct FixedMonotonicClock {
    nanos: AtomicI64,
    step_nanos: i64,
}

impl FixedMonotonicClock {
    pub fn new(step: Duration) -> Self {
        FixedMonotonicClock {
            nanos: AtomicI64::new(0),
            step_nanos: step.as_nanos() as i64,
        }
    }
}

impl Default for FixedMonotonicClock {
    fn default() -> Self {
        Self::new(Duration::from_millis(1))
    }
}

impl MonotonicClock for FixedMonotonicClock {
    fn now_monotonic(&self) -> MonotonicInstant {
        let value = self.nanos.fetch_add(self.step_nanos, Ordering::SeqCst);
        MonotonicInstant(Duration::from_nanos((value + self.step_nanos) as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_clock_is_stable_until_advanced() {
        let clock = FixedClock::at("2026-09-08T00:00:00Z");
        let a = clock.now_utc();
        let b = clock.now_utc();
        assert_eq!(a, b);
        clock.advance(Duration::from_secs(60));
        let c = clock.now_utc();
        assert!(c > a);
    }

    #[test]
    fn fixed_monotonic_clock_strictly_increases() {
        let clock = FixedMonotonicClock::default();
        let a = clock.now_monotonic();
        let b = clock.now_monotonic();
        assert!(b > a);
    }
}
