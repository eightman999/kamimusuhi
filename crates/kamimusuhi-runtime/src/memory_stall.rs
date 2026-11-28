//! Memory stall signal: conversation keeps accumulating while canonical
//! memory does not grow.
//!
//! Live turns seldom write canonical memory directly; it grows when the
//! reflection cycle proposes something and the Mutation Policy and the
//! Continuity Kernel accept it. A broken reflector, a reflector that is not
//! configured to run, or proposals that are refused every time therefore all
//! look the same from the outside: the conversation carries on and memory
//! silently stops growing. This module makes that state visible.
//!
//! It is derived and read-only. The counts come from
//! [`SqliteStore::memory_activity`](kamimusuhi_store_sqlite::SqliteStore::memory_activity),
//! which only runs `SELECT`s; nothing here writes evidence, state, commits,
//! the derived lane or the trace. It detects and reports — it never retries a
//! reflection, repairs anything, or changes configuration.

use kamimusuhi_core::ids::IndividualId;
use kamimusuhi_core::time::UtcTimestamp;
use kamimusuhi_store_sqlite::MemoryActivity;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::error::RuntimeError;
use crate::runtime::Runtime;

/// Default: user utterances since the last memory before a stall is called.
pub const DEFAULT_MIN_USER_UTTERANCES: u64 = 40;
/// Default: seconds since the last memory before a stall is called (48h).
pub const DEFAULT_MIN_QUIET_SECS: u64 = 48 * 60 * 60;

const MAX_MIN_USER_UTTERANCES: u64 = 1_000_000;
const MAX_MIN_QUIET_SECS: u64 = 366 * 24 * 60 * 60;

/// When to call a stall. Both conditions must hold, which keeps false
/// alarms rare: a burst of chat in one evening is not a stall, and neither
/// is a long quiet week with a handful of messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryStallThresholds {
    /// At least this many user utterances since memory last grew (>= 1, so
    /// an individual nobody talks to is never stalled).
    pub min_user_utterances: u64,
    /// At least this many seconds since memory last grew.
    pub min_quiet_secs: u64,
}

impl Default for MemoryStallThresholds {
    fn default() -> Self {
        Self {
            min_user_utterances: DEFAULT_MIN_USER_UTTERANCES,
            min_quiet_secs: DEFAULT_MIN_QUIET_SECS,
        }
    }
}

impl MemoryStallThresholds {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=MAX_MIN_USER_UTTERANCES).contains(&self.min_user_utterances) {
            return Err(format!(
                "min_user_utterances must be 1..={MAX_MIN_USER_UTTERANCES}"
            ));
        }
        if self.min_quiet_secs > MAX_MIN_QUIET_SECS {
            return Err(format!(
                "min_quiet_secs must be at most {MAX_MIN_QUIET_SECS}"
            ));
        }
        Ok(())
    }
}

/// `ok` or `stalled`. Nothing in between: the reason code carries the detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStallState {
    Ok,
    Stalled,
}

/// Stable reason codes. The `STALLED_*` codes are a diagnostic hint read
/// from the counts, not a proof of cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MemoryStallReason {
    /// Nobody has spoken since memory last grew.
    NoUserUtterancesSinceMemory,
    /// Conversation has accumulated, but not past both thresholds.
    BelowThreshold,
    /// No reflection cycle ran and nothing was proposed.
    StalledNoReflection,
    /// Reflection ran but proposed no canonical memory.
    StalledNoMemoryProposals,
    /// Memory was proposed but refused (policy reject/quarantine, or the
    /// reflection's own draft check).
    StalledProposalsRefused,
    /// Proposals exist, none refused, yet memory did not grow.
    StalledUnexplained,
}

/// Where the counts are measured from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStallBaseline {
    /// The newest state record.
    LastMemory,
    /// The individual has no state record; counted from its creation.
    IndividualCreated,
}

/// The signal. Counts and timestamps only — no conversation or memory
/// content ever appears here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryStallReport {
    pub state: MemoryStallState,
    pub reason: MemoryStallReason,
    pub individual_id: IndividualId,
    pub generated_at: String,
    pub thresholds: MemoryStallThresholds,
    pub baseline: MemoryStallBaseline,
    pub baseline_at: String,
    pub last_memory_at: Option<String>,
    pub state_records_total: u64,
    /// Seconds from the baseline to now (0 if the clock is behind it).
    pub quiet_secs: u64,
    /// Standard-retention user utterances since the baseline — the only
    /// utterances the verdict counts.
    pub user_utterances_since_memory: u64,
    /// Newest standard-retention user utterance.
    pub last_user_utterance_at: Option<String>,
    /// Ephemeral-retention user utterances since the baseline (e.g.
    /// non-operator speech in a public channel). Reference only: such speech
    /// is not meant to become memory, so it never counts toward a stall.
    pub ephemeral_user_utterances_since_memory: u64,
    pub reflections_since_memory: u64,
    pub last_reflection_at: Option<String>,
    pub reflection_drafts_rejected: u64,
    pub memory_proposals: u64,
    /// `accept` / `reject` / `quarantine` → count, for proposals recorded
    /// since the baseline.
    pub memory_decisions: BTreeMap<String, u64>,
    /// Operative-lane (C0) proposals since the baseline, by status. They do
    /// not grow canonical memory; reported so a reflector that only tunes
    /// parameters is distinguishable from one that does nothing.
    pub c0_proposals: BTreeMap<String, u64>,
}

fn decided(decisions: &BTreeMap<String, u64>, key: &str) -> u64 {
    decisions.get(key).copied().unwrap_or(0)
}

/// Pure judgement over collected counts. `now` is passed in so the boundary
/// is testable without a clock.
pub fn assess(
    individual_id: IndividualId,
    activity: &MemoryActivity,
    now: UtcTimestamp,
    thresholds: MemoryStallThresholds,
) -> MemoryStallReport {
    let quiet_ms = now
        .unix_millis()
        .saturating_sub(activity.baseline_at.unix_millis());
    let quiet_secs = u64::try_from(quiet_ms / 1000).unwrap_or(0);
    let utterances = activity.user_utterances_since;
    let refused = decided(&activity.memory_decisions_since, "reject")
        + decided(&activity.memory_decisions_since, "quarantine")
        + activity.reflection_drafts_rejected_since;

    let (state, reason) = if utterances == 0 {
        (
            MemoryStallState::Ok,
            MemoryStallReason::NoUserUtterancesSinceMemory,
        )
    } else if utterances < thresholds.min_user_utterances || quiet_secs < thresholds.min_quiet_secs
    {
        (MemoryStallState::Ok, MemoryStallReason::BelowThreshold)
    } else if activity.reflections_since == 0 && activity.memory_proposals_since == 0 {
        (
            MemoryStallState::Stalled,
            MemoryStallReason::StalledNoReflection,
        )
    } else if refused > 0 {
        (
            MemoryStallState::Stalled,
            MemoryStallReason::StalledProposalsRefused,
        )
    } else if activity.memory_proposals_since == 0 {
        (
            MemoryStallState::Stalled,
            MemoryStallReason::StalledNoMemoryProposals,
        )
    } else {
        (
            MemoryStallState::Stalled,
            MemoryStallReason::StalledUnexplained,
        )
    };

    MemoryStallReport {
        state,
        reason,
        individual_id,
        generated_at: now.to_rfc3339(),
        thresholds,
        baseline: if activity.last_state_record_at.is_some() {
            MemoryStallBaseline::LastMemory
        } else {
            MemoryStallBaseline::IndividualCreated
        },
        baseline_at: activity.baseline_at.to_rfc3339(),
        last_memory_at: activity.last_state_record_at.map(UtcTimestamp::to_rfc3339),
        state_records_total: activity.state_records_total,
        quiet_secs,
        user_utterances_since_memory: utterances,
        last_user_utterance_at: activity
            .last_user_utterance_at
            .map(UtcTimestamp::to_rfc3339),
        ephemeral_user_utterances_since_memory: activity.ephemeral_user_utterances_since,
        reflections_since_memory: activity.reflections_since,
        last_reflection_at: activity.last_reflection_at.map(UtcTimestamp::to_rfc3339),
        reflection_drafts_rejected: activity.reflection_drafts_rejected_since,
        memory_proposals: activity.memory_proposals_since,
        memory_decisions: activity.memory_decisions_since.clone(),
        c0_proposals: activity.c0_proposals_since.clone(),
    }
}

/// [`report_at`] with the runtime's own clock.
pub fn report(
    runtime: &Runtime,
    thresholds: MemoryStallThresholds,
) -> Result<MemoryStallReport, RuntimeError> {
    report_at(runtime, runtime.now(), thresholds)
}

/// Collect the counts for the runtime's individual and judge them at `now`.
/// `SELECT`s only.
pub fn report_at(
    runtime: &Runtime,
    now: UtcTimestamp,
    thresholds: MemoryStallThresholds,
) -> Result<MemoryStallReport, RuntimeError> {
    thresholds.validate().map_err(RuntimeError::Usage)?;
    let individual = runtime.individual_id();
    let activity = runtime
        .store()
        .memory_activity(individual)?
        .ok_or_else(|| {
            RuntimeError::Continuity(kamimusuhi_core::continuity::ContinuityError::Backend {
                message: "individual row missing from the canonical store".to_owned(),
            })
        })?;
    Ok(assess(individual, &activity, now, thresholds))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR_MS: i64 = 60 * 60 * 1000;

    fn activity(utterances: u64, quiet_ms: i64) -> (MemoryActivity, UtcTimestamp) {
        let baseline = UtcTimestamp::from_unix_millis(1_800_000_000_000);
        (
            MemoryActivity {
                individual_created_at: UtcTimestamp::from_unix_millis(1_700_000_000_000),
                state_records_total: 3,
                last_state_record_at: Some(baseline),
                baseline_at: baseline,
                user_utterances_since: utterances,
                last_user_utterance_at: None,
                ephemeral_user_utterances_since: 0,
                reflections_since: 0,
                last_reflection_at: None,
                reflection_drafts_rejected_since: 0,
                memory_proposals_since: 0,
                memory_decisions_since: BTreeMap::new(),
                c0_proposals_since: BTreeMap::new(),
            },
            UtcTimestamp::from_unix_millis(baseline.unix_millis() + quiet_ms),
        )
    }

    fn judge(activity: &MemoryActivity, now: UtcTimestamp) -> MemoryStallReport {
        assess(
            IndividualId::from_u128(1),
            activity,
            now,
            MemoryStallThresholds::default(),
        )
    }

    #[test]
    fn defaults_are_conservative_and_valid() {
        let t = MemoryStallThresholds::default();
        assert_eq!(t.min_user_utterances, 40);
        assert_eq!(t.min_quiet_secs, 172_800);
        t.validate().unwrap();
        for bad in [
            MemoryStallThresholds {
                min_user_utterances: 0,
                ..t
            },
            MemoryStallThresholds {
                min_quiet_secs: MAX_MIN_QUIET_SECS + 1,
                ..t
            },
        ] {
            assert!(bad.validate().is_err());
        }
    }

    #[test]
    fn no_utterances_is_never_a_stall_however_long_the_silence() {
        let (a, now) = activity(0, 365 * 24 * HOUR_MS);
        let r = judge(&a, now);
        assert_eq!(r.state, MemoryStallState::Ok);
        assert_eq!(r.reason, MemoryStallReason::NoUserUtterancesSinceMemory);
    }

    #[test]
    fn both_thresholds_are_inclusive_and_both_are_required() {
        // Exactly at both thresholds: stalled.
        let (a, now) = activity(40, 48 * HOUR_MS);
        let r = judge(&a, now);
        assert_eq!(r.state, MemoryStallState::Stalled);
        assert_eq!(r.quiet_secs, 172_800);
        assert_eq!(r.reason, MemoryStallReason::StalledNoReflection);
        // One utterance short.
        let (a, now) = activity(39, 48 * HOUR_MS);
        assert_eq!(judge(&a, now).reason, MemoryStallReason::BelowThreshold);
        // One millisecond short of the quiet period (rounds down a second).
        let (a, now) = activity(40, 48 * HOUR_MS - 1);
        assert_eq!(judge(&a, now).state, MemoryStallState::Ok);
        // Many utterances, but recent memory: not a stall.
        let (a, now) = activity(10_000, HOUR_MS);
        assert_eq!(judge(&a, now).state, MemoryStallState::Ok);
    }

    #[test]
    fn ephemeral_utterances_are_reported_but_never_counted() {
        // Only public-channel chatter, far past both thresholds: not a stall.
        let (mut a, now) = activity(0, 30 * 24 * HOUR_MS);
        a.ephemeral_user_utterances_since = 5_000;
        let r = judge(&a, now);
        assert_eq!(r.state, MemoryStallState::Ok);
        assert_eq!(r.reason, MemoryStallReason::NoUserUtterancesSinceMemory);
        assert_eq!(r.ephemeral_user_utterances_since_memory, 5_000);
        // Standard utterances alone decide, however many ephemeral ones exist.
        a.user_utterances_since = 39;
        assert_eq!(judge(&a, now).reason, MemoryStallReason::BelowThreshold);
        a.user_utterances_since = 40;
        assert_eq!(judge(&a, now).state, MemoryStallState::Stalled);
    }

    #[test]
    fn clock_behind_the_baseline_reads_as_zero_quiet() {
        let (a, now) = activity(100, -HOUR_MS);
        let r = judge(&a, now);
        assert_eq!(r.quiet_secs, 0);
        assert_eq!(r.state, MemoryStallState::Ok);
    }

    #[test]
    fn stall_reason_reads_the_reflection_and_proposal_counts() {
        let (mut a, now) = activity(50, 72 * HOUR_MS);
        assert_eq!(
            judge(&a, now).reason,
            MemoryStallReason::StalledNoReflection
        );
        a.reflections_since = 4;
        assert_eq!(
            judge(&a, now).reason,
            MemoryStallReason::StalledNoMemoryProposals
        );
        a.reflection_drafts_rejected_since = 2;
        assert_eq!(
            judge(&a, now).reason,
            MemoryStallReason::StalledProposalsRefused
        );
        a.reflection_drafts_rejected_since = 0;
        a.memory_proposals_since = 3;
        assert_eq!(judge(&a, now).reason, MemoryStallReason::StalledUnexplained);
        a.memory_decisions_since.insert("reject".to_owned(), 3);
        assert_eq!(
            judge(&a, now).reason,
            MemoryStallReason::StalledProposalsRefused
        );
        // Proposals refused without any reflection (e.g. live-turn drafts).
        a.reflections_since = 0;
        assert_eq!(
            judge(&a, now).reason,
            MemoryStallReason::StalledProposalsRefused
        );
    }

    #[test]
    fn report_serializes_stable_codes() {
        let (a, now) = activity(40, 48 * HOUR_MS);
        let value = serde_json::to_value(judge(&a, now)).unwrap();
        assert_eq!(value["state"], "stalled");
        assert_eq!(value["reason"], "STALLED_NO_REFLECTION");
        assert_eq!(value["baseline"], "last_memory");
        assert_eq!(value["thresholds"]["min_user_utterances"], 40);
    }
}
