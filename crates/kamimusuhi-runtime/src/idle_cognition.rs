//! Input-free reflection over new, subject-scoped experience. The checkpoint
//! contains only IDs/digests and scheduling metadata, never conversation text.
//! A persisted admission precedes the API call. An interrupted admission is
//! reported as uncertain and consumed, rather than blindly sending it again.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use kamimusuhi_core::digest::json_digest;
use kamimusuhi_core::ids::IndividualId;
use kamimusuhi_core::routing::PrivacyConstraint;
use serde::{Deserialize, Serialize};

use crate::c0::reflection::{self, ReflectionReport};
use crate::{Runtime, RuntimeError};

pub const STATE_FILE: &str = "idle-cognition.json";
pub const LOCK_FILE: &str = "idle-cognition.lock";
const MAX_STATE_BYTES: u64 = 64 * 1024;
const MAX_SUBJECTS: usize = 32;
const DAY_MS: i64 = 86_400_000;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IdleCognitionPolicy {
    pub idle_after_secs: u64,
    pub min_interval_secs: u64,
    pub retry_after_secs: u64,
    pub max_attempts: u32,
    pub max_runs_per_day: u32,
}

impl Default for IdleCognitionPolicy {
    fn default() -> Self {
        Self {
            idle_after_secs: 60,
            min_interval_secs: 600,
            retry_after_secs: 600,
            max_attempts: 3,
            max_runs_per_day: 24,
        }
    }
}

impl IdleCognitionPolicy {
    pub fn validate(&self) -> Result<(), RuntimeError> {
        if self.idle_after_secs > 86_400
            || !(1..=86_400).contains(&self.min_interval_secs)
            || !(1..=86_400).contains(&self.retry_after_secs)
            || !(1..=10).contains(&self.max_attempts)
            || !(1..=96).contains(&self.max_runs_per_day)
        {
            return Err(invalid("INVALID_IDLE_COGNITION_POLICY"));
        }
        Ok(())
    }
}

#[derive(Debug, Serialize)]
pub struct CogitationReport {
    pub status: &'static str,
    pub reason: &'static str,
    pub subject: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reflection: Option<ReflectionReport>,
}

impl CogitationReport {
    fn new(
        status: &'static str,
        reason: &'static str,
        subject: &str,
        digest: Option<&str>,
    ) -> Self {
        Self {
            status,
            reason,
            subject: subject.to_owned(),
            source_digest: digest.map(str::to_owned),
            reflection: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    version: u32,
    individual_id: IndividualId,
    day: i64,
    runs_today: u32,
    last_attempt_ms: Option<i64>,
    subjects: BTreeMap<String, SubjectCheckpoint>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SubjectCheckpoint {
    source_digest: String,
    consumed_digest: Option<String>,
    attempts: u32,
    in_flight: bool,
    last_attempt_ms: Option<i64>,
    last_outcome: String,
}

fn invalid(code: &str) -> RuntimeError {
    RuntimeError::Usage(code.to_owned())
}

fn valid_subject(subject: &str) -> bool {
    !subject.is_empty()
        && subject.len() <= 64
        && subject
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

fn valid_digest(digest: &str) -> bool {
    digest
        .strip_prefix("sha256:")
        .is_some_and(|value| value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn no_symlink(path: &Path) -> Result<(), RuntimeError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_symlink() || !meta.is_file() => Err(invalid("INVALID_COGNITION_FILE")),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(invalid("COGNITION_STORAGE_UNAVAILABLE")),
    }
}

fn checkpoint(
    dir: &Path,
    individual_id: IndividualId,
    now: i64,
) -> Result<Checkpoint, RuntimeError> {
    let path = dir.join(STATE_FILE);
    no_symlink(&path)?;
    let mut bytes = Vec::new();
    match File::open(&path) {
        Ok(file) => {
            file.take(MAX_STATE_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| invalid("COGNITION_STORAGE_UNAVAILABLE"))?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Checkpoint {
                version: 1,
                individual_id,
                day: now.div_euclid(DAY_MS),
                runs_today: 0,
                last_attempt_ms: None,
                subjects: BTreeMap::new(),
            });
        }
        Err(_) => return Err(invalid("COGNITION_STORAGE_UNAVAILABLE")),
    }
    if bytes.len() as u64 > MAX_STATE_BYTES {
        return Err(invalid("INVALID_COGNITION_CHECKPOINT"));
    }
    let state: Checkpoint =
        serde_json::from_slice(&bytes).map_err(|_| invalid("INVALID_COGNITION_CHECKPOINT"))?;
    if state.version != 1
        || state.individual_id != individual_id
        || state.subjects.len() > MAX_SUBJECTS
        || state.subjects.keys().any(|s| !valid_subject(s))
        || state.subjects.values().any(|slot| {
            !valid_digest(&slot.source_digest)
                || slot
                    .consumed_digest
                    .as_deref()
                    .is_some_and(|digest| !valid_digest(digest))
                || slot.attempts > 10
        })
    {
        return Err(invalid("INVALID_COGNITION_CHECKPOINT"));
    }
    Ok(state)
}

fn save(dir: &Path, state: &Checkpoint) -> Result<(), RuntimeError> {
    let bytes = serde_json::to_vec(state).map_err(|_| invalid("COGNITION_CHECKPOINT_ENCODING"))?;
    if bytes.len() as u64 > MAX_STATE_BYTES {
        return Err(invalid("COGNITION_CHECKPOINT_TOO_LARGE"));
    }
    let tmp = dir.join(format!(
        ".{STATE_FILE}.{}.{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, dir.join(STATE_FILE))?;
        File::open(dir)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(|_: std::io::Error| invalid("COGNITION_STORAGE_UNAVAILABLE"))
}

fn milliseconds(seconds: u64) -> i64 {
    i64::try_from(seconds.saturating_mul(1000)).unwrap_or(i64::MAX)
}

/// At most one reflection. There is no user turn, synthetic utterance or tool
/// execution here. The resident decides when to invoke this bounded command.
pub fn run(
    runtime: &mut Runtime,
    subject: &str,
    privacy: PrivacyConstraint,
) -> Result<CogitationReport, RuntimeError> {
    run_with(runtime, subject, privacy, |runtime, subject, privacy| {
        reflection::run_with_privacy(runtime, subject, privacy)
    })
}

fn run_with(
    runtime: &mut Runtime,
    subject: &str,
    privacy: PrivacyConstraint,
    reflect: impl FnOnce(
        &mut Runtime,
        &str,
        PrivacyConstraint,
    ) -> Result<ReflectionReport, RuntimeError>,
) -> Result<CogitationReport, RuntimeError> {
    let now = runtime.now().unix_millis();
    run_at(runtime, subject, privacy, now, reflect)
}

fn run_at(
    runtime: &mut Runtime,
    subject: &str,
    privacy: PrivacyConstraint,
    now: i64,
    reflect: impl FnOnce(
        &mut Runtime,
        &str,
        PrivacyConstraint,
    ) -> Result<ReflectionReport, RuntimeError>,
) -> Result<CogitationReport, RuntimeError> {
    if !valid_subject(subject) {
        return Err(invalid("INVALID_COGNITION_SUBJECT"));
    }
    let policy = runtime.config().c0.idle.clone();
    policy.validate()?;
    let _identity = runtime.lock_identity()?;
    runtime.ensure_identity()?;
    runtime.config().check_reflector_privacy(privacy)?;
    if runtime.config().c0.auto_activate {
        runtime.config().persona.check_privacy(privacy)?;
    }
    let dir = runtime.paths().dir.clone();
    let lock_path = dir.join(LOCK_FILE);
    no_symlink(&lock_path)?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options
        .open(&lock_path)
        .map_err(|_| invalid("COGNITION_STORAGE_UNAVAILABLE"))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(fs::TryLockError::WouldBlock) => {
            return Ok(CogitationReport::new(
                "busy",
                "COGNITION_BUSY",
                subject,
                None,
            ));
        }
        Err(_) => return Err(invalid("COGNITION_LOCK_UNAVAILABLE")),
    }
    let mut state = checkpoint(&dir, runtime.individual_id(), now)?;
    // An earlier process may have died after the upstream received its request.
    // Without upstream idempotency we cannot safely claim or retry completion.
    if let Some(slot) = state.subjects.get_mut(subject)
        && slot.in_flight
    {
        slot.in_flight = false;
        slot.consumed_digest = Some(slot.source_digest.clone());
        slot.last_outcome = "INTERRUPTED_UNCERTAIN".to_owned();
        let digest = slot.source_digest.clone();
        save(&dir, &state)?;
        return Ok(CogitationReport::new(
            "interrupted",
            "INTERRUPTED_UNCERTAIN",
            subject,
            Some(&digest),
        ));
    }
    let episodes = reflection::source_episodes(runtime, subject)?;
    if episodes.is_empty() {
        return Ok(CogitationReport::new(
            "idle",
            "NO_EXPERIENCE",
            subject,
            None,
        ));
    }
    let latest = episodes
        .iter()
        .map(|e| e.created_at.unix_millis())
        .max()
        .unwrap_or(now);
    let digest = json_digest(
        &serde_json::to_value(&episodes).map_err(|_| invalid("COGNITION_SOURCE_ENCODING"))?,
    );
    if now < latest.saturating_add(milliseconds(policy.idle_after_secs)) {
        return Ok(CogitationReport::new(
            "idle",
            "WAIT_FOR_IDLE",
            subject,
            Some(&digest),
        ));
    }
    if let Some(slot) = state.subjects.get(subject)
        && slot.consumed_digest.as_deref() == Some(digest.as_str())
    {
        if slot.last_outcome != "COMPLETED" {
            return Ok(CogitationReport::new(
                "interrupted",
                "INTERRUPTED_UNCERTAIN",
                subject,
                Some(&digest),
            ));
        }
        return Ok(CogitationReport::new(
            "idle",
            "NO_NEW_EXPERIENCE",
            subject,
            Some(&digest),
        ));
    }
    if state
        .last_attempt_ms
        .is_some_and(|t| now < t.saturating_add(milliseconds(policy.min_interval_secs)))
    {
        return Ok(CogitationReport::new(
            "cooldown",
            "MIN_INTERVAL",
            subject,
            Some(&digest),
        ));
    }
    if now.div_euclid(DAY_MS) > state.day {
        state.day = now.div_euclid(DAY_MS);
        state.runs_today = 0;
    }
    if state.runs_today >= policy.max_runs_per_day {
        return Ok(CogitationReport::new(
            "exhausted",
            "DAILY_BUDGET",
            subject,
            Some(&digest),
        ));
    }
    if !state.subjects.contains_key(subject) && state.subjects.len() >= MAX_SUBJECTS {
        return Err(invalid("COGNITION_SUBJECT_LIMIT"));
    }
    let slot = state.subjects.entry(subject.to_owned()).or_default();
    if slot.source_digest != digest {
        slot.source_digest = digest.clone();
        slot.attempts = 0;
    }
    if slot.attempts >= policy.max_attempts {
        return Ok(CogitationReport::new(
            "exhausted",
            "ATTEMPTS_EXHAUSTED",
            subject,
            Some(&digest),
        ));
    }
    if slot.attempts > 0
        && slot
            .last_attempt_ms
            .is_some_and(|t| now < t.saturating_add(milliseconds(policy.retry_after_secs)))
    {
        return Ok(CogitationReport::new(
            "cooldown",
            "RETRY_BACKOFF",
            subject,
            Some(&digest),
        ));
    }
    slot.attempts += 1;
    slot.last_attempt_ms = Some(now);
    slot.in_flight = true;
    slot.last_outcome = "RUNNING".to_owned();
    state.last_attempt_ms = Some(now);
    state.runs_today += 1;
    save(&dir, &state)?;
    let result = reflect(runtime, subject, privacy);
    let slot = state.subjects.get_mut(subject).expect("admitted subject");
    slot.in_flight = false;
    match result {
        Ok(reflection) => {
            slot.consumed_digest = Some(digest.clone());
            slot.last_outcome = "COMPLETED".to_owned();
            save(&dir, &state)?;
            let mut report =
                CogitationReport::new("completed", "REFLECTED", subject, Some(&digest));
            report.reflection = Some(reflection);
            Ok(report)
        }
        Err(error) => {
            slot.last_outcome = "FAILED".to_owned();
            // A provider error precedes intake, so a bounded retry is safe.
            // Storage/gate failures may have followed a partial commit; do
            // not generate another cycle over that same source blindly.
            if !matches!(error, RuntimeError::Persona(_)) {
                slot.consumed_digest = Some(digest);
                slot.last_outcome = "FAILED_UNCERTAIN".to_owned();
            }
            save(&dir, &state)?;
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ResourceImplementation, RuntimeOptions};
    use kamimusuhi_core::evidence::{
        EvidenceKind, EvidenceSource, EvidenceStore, NewEvidence, RetentionClass,
    };
    use kamimusuhi_core::ids::EvidenceId;
    use kamimusuhi_core::mutation::OriginClass;

    fn prepared() -> (tempfile::TempDir, Runtime) {
        let dir = tempfile::tempdir().unwrap();
        let mut runtime = Runtime::init(
            dir.path(),
            RuntimeOptions::deterministic(0x1D1E),
            ResourceImplementation::FakeA,
        )
        .unwrap();
        let mut config = runtime.config().clone();
        config.c0.idle.idle_after_secs = 0;
        config.c0.idle.min_interval_secs = 1;
        config.c0.idle.retry_after_secs = 10;
        runtime.save_config(config).unwrap();
        let review = runtime.identity_status().unwrap();
        runtime.accept_identity(&review.review_token).unwrap();
        (dir, runtime)
    }
    fn input(runtime: &Runtime, subject: &str, id: u128) {
        runtime
            .store()
            .append(NewEvidence {
                evidence_id: EvidenceId::from_u128(id),
                individual_id: runtime.individual_id(),
                session_id: None,
                turn_id: None,
                kind: EvidenceKind::UserUtterance,
                origin_class: OriginClass::Observed,
                payload: serde_json::json!({"text": format!("experience {id}")}),
                source: EvidenceSource {
                    source_id: Some(format!("text-chat:{subject}")),
                    ..Default::default()
                },
                retention_class: RetentionClass::Standard,
            })
            .unwrap();
    }
    fn success(
        _: &mut Runtime,
        _: &str,
        _: PrivacyConstraint,
    ) -> Result<ReflectionReport, RuntimeError> {
        Ok(ReflectionReport {
            reflection_evidence_id: EvidenceId::from_u128(0xAA),
            reflector_backend: "fixture".to_owned(),
            evaluator_distinct_from_actor: false,
            episodes_presented: 1,
            notes: Vec::new(),
            corrections_linked: 0,
            canonical_drafts_submitted: 0,
            canonical_drafts_activated: 0,
            proposals_created: Vec::new(),
            drafts_rejected: Vec::new(),
            gate_results: Vec::new(),
            activations: Vec::new(),
        })
    }
    fn never(
        _: &mut Runtime,
        _: &str,
        _: PrivacyConstraint,
    ) -> Result<ReflectionReport, RuntimeError> {
        panic!("a skipped tick contacted a reflector")
    }
    fn tick(
        runtime: &mut Runtime,
        subject: &str,
        now: i64,
        reflect: impl FnOnce(
            &mut Runtime,
            &str,
            PrivacyConstraint,
        ) -> Result<ReflectionReport, RuntimeError>,
    ) -> CogitationReport {
        run_at(runtime, subject, PrivacyConstraint::LocalOnly, now, reflect).unwrap()
    }
    #[test]
    fn no_experience_and_recent_activity_do_not_infer() {
        let (_dir, mut runtime) = prepared();
        let now = runtime.now().unix_millis();
        assert_eq!(
            tick(&mut runtime, "alice", now, never).reason,
            "NO_EXPERIENCE"
        );
        input(&runtime, "alice", 1);
        // A source in the future (or a wall-clock rollback) waits safely.
        assert_eq!(
            tick(&mut runtime, "alice", now - 1, never).reason,
            "WAIT_FOR_IDLE"
        );
    }
    #[test]
    fn completed_source_is_consumed_across_restart_and_other_subject_activity() {
        let (dir, mut runtime) = prepared();
        let now = runtime.now().unix_millis();
        input(&runtime, "alice", 1);
        assert_eq!(
            tick(&mut runtime, "alice", now, success).status,
            "completed"
        );
        assert_eq!(
            tick(&mut runtime, "alice", now, never).reason,
            "NO_NEW_EXPERIENCE"
        );
        input(&runtime, "bob", 2);
        assert_eq!(
            tick(&mut runtime, "alice", now + 2000, never).reason,
            "NO_NEW_EXPERIENCE"
        );
        drop(runtime);
        let mut restored =
            Runtime::open(dir.path(), RuntimeOptions::deterministic(0x1D2E)).unwrap();
        assert_eq!(
            tick(&mut restored, "alice", now + 2000, never).reason,
            "NO_NEW_EXPERIENCE"
        );
        input(&restored, "alice", 3);
        assert_eq!(
            tick(&mut restored, "alice", now + 2000, success).status,
            "completed"
        );
    }
    #[test]
    fn attempts_backoff_and_daily_budget_survive_restart_and_clock_rollback() {
        let (dir, mut runtime) = prepared();
        let now = runtime.now().unix_millis();
        input(&runtime, "alice", 1);
        let fail = |_: &mut Runtime, _: &str, _: PrivacyConstraint| {
            Err(RuntimeError::Persona(
                kamimusuhi_core::persona::PersonaError::Transport {
                    backend_id: kamimusuhi_core::ids::PersonaBackendId::from_u128(0xEE),
                    message: "fixture".to_owned(),
                },
            ))
        };
        assert!(
            run_at(
                &mut runtime,
                "alice",
                PrivacyConstraint::LocalOnly,
                now,
                fail
            )
            .is_err()
        );
        assert_eq!(
            tick(&mut runtime, "alice", now + 1000, never).reason,
            "RETRY_BACKOFF"
        );
        assert!(
            run_at(
                &mut runtime,
                "alice",
                PrivacyConstraint::LocalOnly,
                now + 10_000,
                fail
            )
            .is_err()
        );
        assert!(
            run_at(
                &mut runtime,
                "alice",
                PrivacyConstraint::LocalOnly,
                now + 20_000,
                fail
            )
            .is_err()
        );
        assert_eq!(
            tick(&mut runtime, "alice", now + 30_000, never).reason,
            "ATTEMPTS_EXHAUSTED"
        );
        let mut state = checkpoint(dir.path(), runtime.individual_id(), now).unwrap();
        state.runs_today = runtime.config().c0.idle.max_runs_per_day;
        save(dir.path(), &state).unwrap();
        input(&runtime, "bob", 2);
        assert_eq!(
            tick(&mut runtime, "bob", now - 1, never).reason,
            "WAIT_FOR_IDLE"
        );
        assert_eq!(
            tick(&mut runtime, "bob", now + 30_000, never).reason,
            "DAILY_BUDGET"
        );
        drop(runtime);
        let mut restored =
            Runtime::open(dir.path(), RuntimeOptions::deterministic(0x1D3E)).unwrap();
        assert_eq!(
            tick(&mut restored, "bob", now + 30_000, never).reason,
            "DAILY_BUDGET"
        );
        assert_eq!(
            tick(&mut restored, "bob", now + DAY_MS, success).status,
            "completed"
        );
    }
    #[test]
    fn concurrent_tick_and_uncertain_interruption_never_resend_the_source() {
        let (dir, mut runtime) = prepared();
        let now = runtime.now().unix_millis();
        input(&runtime, "alice", 1);
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.path().join(LOCK_FILE))
            .unwrap();
        lock.lock().unwrap();
        assert_eq!(tick(&mut runtime, "alice", now, never).status, "busy");
        drop(lock);
        let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            tick(&mut runtime, "alice", now, |_, _, _| {
                panic!("fixture termination")
            })
        }));
        assert!(crash.is_err());
        drop(runtime);
        let mut restored =
            Runtime::open(dir.path(), RuntimeOptions::deterministic(0x1D4E)).unwrap();
        assert_eq!(
            tick(&mut restored, "alice", now + 2000, never).status,
            "interrupted"
        );
        assert_eq!(
            tick(&mut restored, "alice", now + 2000, never).reason,
            "INTERRUPTED_UNCERTAIN"
        );
        let state = checkpoint(dir.path(), restored.individual_id(), now).unwrap();
        assert_eq!(state.runs_today, 1);
    }
    #[test]
    fn corrupt_foreign_or_symlinked_checkpoints_fail_before_dispatch() {
        let (dir, mut runtime) = prepared();
        let now = runtime.now().unix_millis();
        input(&runtime, "alice", 1);
        fs::write(dir.path().join(STATE_FILE), "not JSON").unwrap();
        assert!(
            run_at(
                &mut runtime,
                "alice",
                PrivacyConstraint::LocalOnly,
                now,
                never
            )
            .is_err()
        );
        fs::remove_file(dir.path().join(STATE_FILE)).unwrap();
        let mut foreign = checkpoint(dir.path(), runtime.individual_id(), now).unwrap();
        foreign.individual_id = IndividualId::from_u128(0xBAD);
        save(dir.path(), &foreign).unwrap();
        assert!(
            run_at(
                &mut runtime,
                "alice",
                PrivacyConstraint::LocalOnly,
                now,
                never
            )
            .is_err()
        );
        #[cfg(unix)]
        {
            fs::remove_file(dir.path().join(STATE_FILE)).unwrap();
            std::os::unix::fs::symlink("missing-target", dir.path().join(STATE_FILE)).unwrap();
            assert!(
                run_at(
                    &mut runtime,
                    "alice",
                    PrivacyConstraint::LocalOnly,
                    now,
                    never
                )
                .is_err()
            );
        }
    }

    #[test]
    fn api_failure_after_intake_is_not_dispatched_a_second_time() {
        let (_dir, mut runtime) = prepared();
        let now = runtime.now().unix_millis();
        input(&runtime, "alice", 1);
        let failure = run_at(
            &mut runtime,
            "alice",
            PrivacyConstraint::LocalOnly,
            now,
            |_, _, _| {
                Err(RuntimeError::ReflectionAfterIntake(Box::new(
                    RuntimeError::Persona(kamimusuhi_core::persona::PersonaError::Transport {
                        backend_id: kamimusuhi_core::ids::PersonaBackendId::from_u128(0xEE),
                        message: "post-intake replay outage".to_owned(),
                    }),
                )))
            },
        );
        assert!(failure.is_err());
        assert_eq!(
            tick(&mut runtime, "alice", now + 20_000, never).reason,
            "INTERRUPTED_UNCERTAIN"
        );
    }
}
