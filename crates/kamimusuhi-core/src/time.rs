//! Clock abstractions.
//!
//! Persisted records store UTC wall time (`UtcTimestamp`). Local
//! timeout/freshness control uses a separate monotonic abstraction so a
//! system clock step (NTP correction, VM pause) cannot be mistaken for
//! elapsed duration. `source_time` (when an event happened upstream) and
//! `received_at` (when this runtime observed it) are intentionally kept
//! as separate fields wherever both exist; they are never collapsed into
//! one "timestamp" field.

use serde::{Deserialize, Serialize};
use std::time::Duration;
use time::OffsetDateTime;

/// A UTC wall-clock instant, as stored in canonical records.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UtcTimestamp(#[serde(with = "time::serde::rfc3339")] OffsetDateTime);

impl UtcTimestamp {
    pub fn from_offset_date_time(dt: OffsetDateTime) -> Self {
        UtcTimestamp(dt.to_offset(time::UtcOffset::UTC))
    }

    pub fn as_offset_date_time(&self) -> OffsetDateTime {
        self.0
    }

    /// RFC 3339 text form, the on-disk/JSON representation.
    pub fn to_rfc3339(&self) -> String {
        self.0
            .format(&time::format_description::well_known::Rfc3339)
            .expect("valid RFC3339 formatting")
    }

    pub fn parse_rfc3339(s: &str) -> Result<Self, time::error::Parse> {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map(Self::from_offset_date_time)
    }
}

/// Abstraction over "what time is it, in wall-clock terms". Production
/// uses [`SystemWallClock`]; tests inject a fixed clock so persisted
/// timestamps are reproducible.
pub trait WallClock: Send + Sync {
    fn now_utc(&self) -> UtcTimestamp;
}

#[derive(Debug, Default)]
pub struct SystemWallClock;

impl WallClock for SystemWallClock {
    fn now_utc(&self) -> UtcTimestamp {
        UtcTimestamp::from_offset_date_time(OffsetDateTime::now_utc())
    }
}

/// Abstraction over elapsed-time measurement, independent of wall clock
/// stepping. Used for local timeout/freshness control, never persisted
/// directly as a canonical record field.
pub trait MonotonicClock: Send + Sync {
    fn now_monotonic(&self) -> MonotonicInstant;
}

/// Opaque monotonic instant. Only meaningful relative to another
/// `MonotonicInstant` from the same clock instance.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Debug)]
pub struct MonotonicInstant(pub Duration);

#[derive(Debug, Default)]
pub struct SystemMonotonicClock {
    start: std::sync::OnceLock<std::time::Instant>,
}

impl SystemMonotonicClock {
    pub fn new() -> Self {
        Self::default()
    }
}

impl MonotonicClock for SystemMonotonicClock {
    fn now_monotonic(&self) -> MonotonicInstant {
        let start = *self.start.get_or_init(std::time::Instant::now);
        MonotonicInstant(start.elapsed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_round_trips() {
        let dt = OffsetDateTime::parse(
            "2026-09-08T00:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        let ts = UtcTimestamp::from_offset_date_time(dt);
        let text = ts.to_rfc3339();
        let parsed = UtcTimestamp::parse_rfc3339(&text).unwrap();
        assert_eq!(ts, parsed);
    }

    #[test]
    fn monotonic_clock_is_non_decreasing() {
        let clock = SystemMonotonicClock::new();
        let a = clock.now_monotonic();
        let b = clock.now_monotonic();
        assert!(b >= a);
    }
}
