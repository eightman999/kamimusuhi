//! Structural meaning of the durable engineering cursor. This check performs
//! no git, model or command work, and runs before any recovery writes.

use std::collections::HashSet;
use std::path::{Component, Path};

use super::{Job, Phase, Snapshot};

pub(super) const CURRENT_SCHEMA_VERSION: u32 = 2;

fn identifier(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 80
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

fn commit_id(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|b| b.is_ascii_hexdigit())
}

fn text(text: &str, max: usize) -> bool {
    !text.trim().is_empty() && text.len() <= max && !text.contains('\0')
}

fn highest_sequence(job: &Job) -> Result<u64, String> {
    let mut highest = job.metrics.decisions;
    if let Some(pending) = &job.pending {
        highest = highest.max(pending.sequence);
    }
    for record in &job.observations {
        highest = highest.max(record.sequence);
        // In v1, the outer sequence of a recovery observation was the
        // completed-action count, not the interrupted claim's sequence.
        // Inspect only this resident-owned format, never arbitrary output.
        if record.detail["kind"] == "interrupted_action_outcome_unknown" {
            let sequence = record.detail["sequence"]
                .as_u64()
                .filter(|sequence| *sequence > 0)
                .ok_or("engineering checkpoint interruption sequence is invalid")?;
            highest = highest.max(sequence);
        }
    }
    Ok(highest)
}

/// Upgrade a snapshot whose complete v1 meaning has already been validated.
/// The caller recovers legacy pending actions before validating the result as
/// v2: a v1 pending sequence can precede the derived high-water mark.
pub(super) fn migrate(snapshot: &mut Snapshot) -> Result<bool, String> {
    match snapshot.schema_version {
        CURRENT_SCHEMA_VERSION => return Ok(false),
        1 => {}
        _ => return Err("unsupported engineering checkpoint schema".into()),
    }
    // Compute every cursor before changing anything, including the schema.
    // A late overflow or invalid recovery record leaves the input intact.
    let cursors: Vec<u64> = snapshot
        .jobs
        .iter()
        .map(|job| {
            highest_sequence(job)?
                .checked_add(1)
                .ok_or_else(|| "engineering action sequence space exhausted".to_owned())
        })
        .collect::<Result<_, _>>()?;
    for (job, cursor) in snapshot.jobs.iter_mut().zip(cursors) {
        job.next_action_sequence = cursor;
    }
    snapshot.schema_version = CURRENT_SCHEMA_VERSION;
    Ok(true)
}

pub(super) fn validate(snapshot: &Snapshot, candidate_root: &Path) -> Result<(), String> {
    if !matches!(snapshot.schema_version, 1 | CURRENT_SCHEMA_VERSION)
        || !candidate_root.is_absolute()
    {
        return Err("unsupported engineering checkpoint schema or root".into());
    }
    let mut ids = HashSet::new();
    let mut request_ids = HashSet::new();
    for job in &snapshot.jobs {
        if !identifier(&job.id)
            || !identifier(&job.workspace)
            || !text(&job.objective, 16_384)
            || !ids.insert(job.id.as_str())
        {
            return Err("engineering checkpoint contains invalid or duplicate job identity".into());
        }
        if let Some(request_id) = &job.request_id
            && (!text(request_id, 128) || !request_ids.insert(request_id.as_str()))
        {
            return Err(
                "engineering checkpoint contains invalid or duplicate request identity".into(),
            );
        }
        let expected_path = candidate_root.join(&job.workspace).join(&job.id);
        if job.candidate.path != expected_path
            || !job.candidate.path.is_absolute()
            || job
                .candidate
                .path
                .components()
                .any(|c| matches!(c, Component::ParentDir))
            || job.candidate.branch != format!("kamimusuhi/engineering/{}", job.id)
            || !commit_id(&job.candidate.base_commit)
        {
            return Err("engineering checkpoint candidate identity is inconsistent".into());
        }
        let metrics = &job.metrics;
        if metrics.verification_failures > metrics.verification_runs
            || metrics.verification_runs > metrics.actions
            || metrics.actions > metrics.decisions
            || metrics.repairs > metrics.actions
        {
            return Err("engineering checkpoint counters are inconsistent".into());
        }
        if let Some(episode) = &job.model_recovery
            && (episode.attempts == 0
                || episode.attempts > metrics.model_failures
                || episode.first_error.len() > super::MODEL_ERROR_BYTES
                || episode.latest_error.len() > super::MODEL_ERROR_BYTES)
        {
            return Err("engineering checkpoint model recovery episode is inconsistent".into());
        }
        // Wall-clock adjustments do not invalidate a durable episode. Error
        // strings may be empty or contain controls; serde escapes them and
        // their byte bound, not human prose validation, governs persistence.
        let highest = highest_sequence(job)?;
        if snapshot.schema_version == CURRENT_SCHEMA_VERSION
            && (job.next_action_sequence == 0 || job.next_action_sequence <= highest)
        {
            return Err("engineering checkpoint action cursor is inconsistent".into());
        }
        if let Some(pending) = &job.pending {
            let valid_cursor = match snapshot.schema_version {
                1 => {
                    metrics.actions.checked_add(1) == Some(pending.sequence)
                        && metrics.decisions >= pending.sequence
                }
                CURRENT_SCHEMA_VERSION => {
                    pending.sequence > 0
                        && pending.sequence.checked_add(1) == Some(job.next_action_sequence)
                        && metrics
                            .actions
                            .checked_add(1)
                            .is_some_and(|claimed| metrics.decisions >= claimed)
                }
                _ => unreachable!("schema checked above"),
            };
            if !matches!(job.phase, Phase::Active | Phase::Paused) || !valid_cursor {
                return Err("engineering checkpoint pending cursor is inconsistent".into());
            }
        }
        if job.validation.as_ref().is_some_and(|v| !commit_id(&v.head)) {
            return Err("engineering checkpoint validation revision is invalid".into());
        }
        if job.phase == Phase::Ready {
            let proven = job.validation.as_ref().is_some_and(|v| {
                v.passed
                    && !v.checks.is_empty()
                    && v.checks.iter().all(|check| {
                        check["passed"] == true && check["exit_code"].as_i64() == Some(0)
                    })
            });
            if !proven
                || job.pending.is_some()
                || metrics.verification_runs == 0
                || job.repair_pending
            {
                return Err(
                    "engineering checkpoint Ready state has no valid verification receipt".into(),
                );
            }
        }
        if job.checkpoint.summary.len() > 4096
            || job.checkpoint.next_step.len() > 2048
            || job.observations.len() > super::KEEP_OBSERVATIONS
        {
            return Err("engineering checkpoint working memory exceeds its bounds".into());
        }
    }
    Ok(())
}
