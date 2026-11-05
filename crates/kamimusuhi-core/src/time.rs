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

    /// Parse RFC 3339 / ISO 8601 text back into a timestamp.
    ///
    /// Accepts the form [`to_rfc3339`](Self::to_rfc3339) emits, the same form
    /// without a fractional part, and `+HH:MM`/`-HH:MM` (or `+HHMM`, `+HH`)
    /// offsets. Anything else returns `None`: the input is producer-supplied
    /// candidate data, so parsing fails soft rather than guessing.
    pub fn from_rfc3339(text: &str) -> Option<Self> {
        fn field(text: &str, from: usize, len: usize) -> Option<i64> {
            let slice = text.get(from..from + len)?;
            if !slice.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            slice.parse().ok()
        }

        let b = text.as_bytes();
        if b.len() < 19
            || b[4] != b'-'
            || b[7] != b'-'
            || !matches!(b[10], b'T' | b't' | b' ')
            || b[13] != b':'
            || b[16] != b':'
        {
            return None;
        }
        let year = field(text, 0, 4)?;
        let month = field(text, 5, 2)?;
        let day = field(text, 8, 2)?;
        let hour = field(text, 11, 2)?;
        let minute = field(text, 14, 2)?;
        let second = field(text, 17, 2)?;
        if !(1..=12).contains(&month)
            || !(1..=31).contains(&day)
            || hour > 23
            || minute > 59
            || second > 60
        {
            return None;
        }

        let mut rest = &text[19..];
        let mut millis = 0i64;
        if let Some(fraction) = rest.strip_prefix('.') {
            let digits = fraction.bytes().take_while(|b| b.is_ascii_digit()).count();
            if digits == 0 {
                return None;
            }
            let taken = digits.min(3);
            millis = fraction[..taken].parse::<i64>().ok()? * 10i64.pow((3 - taken) as u32);
            rest = &fraction[digits..];
        }

        let offset_secs = match rest.as_bytes().first() {
            None | Some(b'Z') | Some(b'z') if rest.len() <= 1 => 0,
            Some(b'+') | Some(b'-') => {
                let sign = if rest.as_bytes()[0] == b'-' { -1 } else { 1 };
                let hm = &rest[1..];
                let (h, m) = match hm.len() {
                    5 if hm.as_bytes()[2] == b':' => (field(hm, 0, 2)?, field(hm, 3, 2)?),
                    4 => (field(hm, 0, 2)?, field(hm, 2, 2)?),
                    2 => (field(hm, 0, 2)?, 0),
                    _ => return None,
                };
                if h > 23 || m > 59 {
                    return None;
                }
                sign * (h * 3600 + m * 60)
            }
            _ => return None,
        };

        let days = days_from_civil(year, month, day);
        let secs = days * 86_400 + hour * 3600 + minute * 60 + second - offset_secs;
        Some(Self::from_unix_millis(
            secs.saturating_mul(1_000).saturating_add(millis),
        ))
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

/// Proleptic Gregorian (year, month, day) to days since 1970-01-01.
/// Howard Hinnant's `days_from_civil` algorithm, the inverse of
/// [`civil_from_days`].
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9).rem_euclid(12);
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
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

/// The two clocks a runtime needs, kept apart on purpose.
///
/// `wall` timestamps durable records, because a record has to be placed in
/// calendar time that survives a restart. `monotonic` measures how long
/// something took, because wall time can jump backwards and a duration
/// measured across an NTP correction is not a duration.
///
/// They are separate fields rather than one clock so that a deterministic
/// fixture can pin the wall clock without also claiming every operation took
/// zero time.
#[derive(Clone)]
pub struct Clocks {
    pub wall: std::sync::Arc<dyn Clock>,
    pub monotonic: std::sync::Arc<dyn MonotonicClock>,
}

impl fmt::Debug for Clocks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Clocks").finish_non_exhaustive()
    }
}

impl Clocks {
    pub fn new(
        wall: std::sync::Arc<dyn Clock>,
        monotonic: std::sync::Arc<dyn MonotonicClock>,
    ) -> Self {
        Self { wall, monotonic }
    }

    /// Both clocks from one real system clock.
    pub fn system() -> Self {
        let clock = std::sync::Arc::new(SystemClock::new());
        Self {
            wall: clock.clone(),
            monotonic: clock,
        }
    }

    pub fn now_utc(&self) -> UtcTimestamp {
        self.wall.now_utc()
    }

    pub fn now_monotonic(&self) -> MonotonicInstant {
        self.monotonic.now_monotonic()
    }

    /// Milliseconds elapsed since `earlier`, measured monotonically.
    pub fn elapsed_ms_since(&self, earlier: MonotonicInstant) -> u64 {
        u64::try_from(self.now_monotonic().duration_since(earlier).as_millis()).unwrap_or(u64::MAX)
    }
}

/// Shared monotonic clocks are monotonic clocks, for the same reason as
/// [`Clock`].
impl<T: MonotonicClock + ?Sized> MonotonicClock for std::sync::Arc<T> {
    fn now_monotonic(&self) -> MonotonicInstant {
        (**self).now_monotonic()
    }
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
    fn rfc3339_parsing_round_trips_and_accepts_offsets() {
        for millis in [0, 1_788_825_600_000, 1_709_210_096_789, -1] {
            let timestamp = UtcTimestamp::from_unix_millis(millis);
            assert_eq!(
                UtcTimestamp::from_rfc3339(&timestamp.to_rfc3339()),
                Some(timestamp)
            );
        }
        // 1_730_000_000 s = 2024-10-27T03:33:20Z; offsets name the same instant.
        let expected = Some(UtcTimestamp::from_unix_millis(1_730_000_000_000));
        assert_eq!(UtcTimestamp::from_rfc3339("2024-10-27T03:33:20Z"), expected);
        assert_eq!(
            UtcTimestamp::from_rfc3339("2024-10-27T04:33:20+01:00"),
            expected
        );
        assert_eq!(
            UtcTimestamp::from_rfc3339("2024-10-27T12:33:20+0900"),
            expected
        );
        // A missing offset is read as UTC; malformed or out-of-range input
        // fails soft.
        assert_eq!(UtcTimestamp::from_rfc3339("2024-10-27T03:33:20"), expected);
        assert_eq!(UtcTimestamp::from_rfc3339("not a date"), None);
        assert_eq!(UtcTimestamp::from_rfc3339("2024-13-01T00:00:00Z"), None);
    }

    #[test]
    fn monotonic_difference_saturates() {
        let a = MonotonicInstant::from_offset(Duration::from_millis(5));
        let b = MonotonicInstant::from_offset(Duration::from_millis(9));
        assert_eq!(b.duration_since(a), Duration::from_millis(4));
        assert_eq!(a.duration_since(b), Duration::ZERO);
    }
}
