use std::sync::Mutex;
use std::time::Duration;

use kamimusuhi_core::time::{Clock, MonotonicClock, MonotonicInstant, UtcTimestamp};

/// Manually advanced clock. Wall and monotonic time advance together unless
/// a test deliberately skews them.
#[derive(Debug)]
pub struct FixedClock {
    state: Mutex<State>,
}

#[derive(Debug, Clone, Copy)]
struct State {
    wall: UtcTimestamp,
    monotonic: Duration,
}

impl FixedClock {
    /// 2026-09-08T00:00:00Z, the planning baseline date.
    pub const BASELINE_UNIX_MILLIS: i64 = 1_788_825_600_000;

    pub fn at(wall: UtcTimestamp) -> Self {
        Self {
            state: Mutex::new(State {
                wall,
                monotonic: Duration::ZERO,
            }),
        }
    }

    pub fn baseline() -> Self {
        Self::at(UtcTimestamp::from_unix_millis(Self::BASELINE_UNIX_MILLIS))
    }

    /// Advance both clocks.
    pub fn advance(&self, by: Duration) {
        let mut s = self.state.lock().expect("fixed clock poisoned");
        s.wall = UtcTimestamp::from_unix_millis(
            s.wall.unix_millis() + i64::try_from(by.as_millis()).unwrap_or(i64::MAX),
        );
        s.monotonic += by;
    }

    /// Move only the wall clock (e.g. to simulate NTP rollback).
    pub fn set_wall(&self, wall: UtcTimestamp) {
        self.state.lock().expect("fixed clock poisoned").wall = wall;
    }
}

impl Default for FixedClock {
    fn default() -> Self {
        Self::baseline()
    }
}

impl Clock for FixedClock {
    fn now_utc(&self) -> UtcTimestamp {
        self.state.lock().expect("fixed clock poisoned").wall
    }
}

impl MonotonicClock for FixedClock {
    fn now_monotonic(&self) -> MonotonicInstant {
        MonotonicInstant::from_offset(self.state.lock().expect("fixed clock poisoned").monotonic)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wall_rollback_does_not_move_monotonic_clock() {
        let clock = FixedClock::baseline();
        let m0 = clock.now_monotonic();
        clock.advance(Duration::from_secs(10));
        clock.set_wall(UtcTimestamp::from_unix_millis(0));
        assert_eq!(clock.now_utc().unix_millis(), 0);
        assert_eq!(
            clock.now_monotonic().duration_since(m0),
            Duration::from_secs(10)
        );
    }
}
