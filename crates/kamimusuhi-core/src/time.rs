//! Clock abstractions.
//!
//! Durable records store UTC wall time ([`UtcTimestamp`]). Timeout and
//! freshness control uses a monotonic clock ([`MonotonicClock`]) so that wall
//! clock rollback cannot extend deadlines. Tests inject fixed implementations
//! from the testkit; production uses [`SystemClock`].
//!
//! `source_time` (when a sensor observed something) and `received_at` (when
//! the runtime accepted the event) are distinct fields in event contracts and
//! must not be collapsed into one value.

use std::fmt;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// UTC wall-clock time with millisecond precision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UtcTimestamp {
    unix_millis: i64,
}

impl UtcTimestamp {
    pub const fn from_unix_millis(unix_millis: i64) -> Self {
        Self { unix_millis }
    }

    pub const fn unix_millis(self) -> i64 {
        self.unix_millis
    }

    /// RFC 3339 / ISO 8601 text, e.g. `2026-09-08T00:00:00.000Z`.
    pub fn to_rfc3339(self) -> String {
        let millis = self.unix_millis.rem_euclid(1000);
        let secs = self.unix_millis.div_euclid(1000);
        let days = secs.div_euclid(86_400);
        let sod = secs.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        format!(
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
            sod / 3600,
            (sod % 3600) / 60,
            sod % 60
        )
    }
}

impl fmt::Display for UtcTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_rfc3339())
    }
}

/// Days since 1970-01-01 to proleptic Gregorian (year, month, day).
/// Howard Hinnant's `civil_from_days` algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Source of UTC wall time for durable records.
pub trait Clock: Send + Sync {
    fn now_utc(&self) -> UtcTimestamp;
}

/// Shared clocks are clocks. One `Arc<dyn Clock>` can then be handed to the
/// store and to the kernel instead of constructing two that could disagree.
impl<T: Clock + ?Sized> Clock for std::sync::Arc<T> {
    fn now_utc(&self) -> UtcTimestamp {
        (**self).now_utc()
    }
}

/// Monotonic instant relative to a process-local anchor. Only differences
/// between instants from the same clock are meaningful.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MonotonicInstant(Duration);

impl MonotonicInstant {
    pub const fn from_offset(offset: Duration) -> Self {
        Self(offset)
    }

    pub fn duration_since(self, earlier: Self) -> Duration {
        self.0.saturating_sub(earlier.0)
    }
}

/// Source of monotonic time for timeouts and freshness windows.
pub trait MonotonicClock: Send + Sync {
    fn now_monotonic(&self) -> MonotonicInstant;
}

/// Real system clocks.
#[derive(Debug, Clone)]
pub struct SystemClock {
    anchor: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            anchor: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now_utc(&self) -> UtcTimestamp {
        let millis = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
            Err(e) => -i64::try_from(e.duration().as_millis()).unwrap_or(i64::MAX),
        };
        UtcTimestamp::from_unix_millis(millis)
    }
}

impl MonotonicClock for SystemClock {
    fn now_monotonic(&self) -> MonotonicInstant {
        MonotonicInstant(self.anchor.elapsed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_epoch_and_known_dates() {
        assert_eq!(
            UtcTimestamp::from_unix_millis(0).to_rfc3339(),
            "1970-01-01T00:00:00.000Z"
        );
        // 2026-09-08T00:00:00Z
        assert_eq!(
            UtcTimestamp::from_unix_millis(1_788_825_600_000).to_rfc3339(),
            "2026-09-08T00:00:00.000Z"
        );
        // Leap day with sub-second component: 2024-02-29T12:34:56.789Z
        assert_eq!(
            UtcTimestamp::from_unix_millis(1_709_210_096_789).to_rfc3339(),
            "2024-02-29T12:34:56.789Z"
        );
        assert_eq!(
            UtcTimestamp::from_unix_millis(-1).to_rfc3339(),
            "1969-12-31T23:59:59.999Z"
        );
    }

    #[test]
    fn monotonic_difference_saturates() {
        let a = MonotonicInstant::from_offset(Duration::from_millis(5));
        let b = MonotonicInstant::from_offset(Duration::from_millis(9));
        assert_eq!(b.duration_since(a), Duration::from_millis(4));
        assert_eq!(a.duration_since(b), Duration::ZERO);
    }
}
