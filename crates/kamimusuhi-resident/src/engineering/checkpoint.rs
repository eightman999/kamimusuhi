//! Structural meaning of the durable engineering cursor. This check performs
//! no git, model or command work, and runs before any recovery writes.

use std::collections::HashSet;
use std::path::{Component, Path};

use super::{Phase, Snapshot};

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

pub(super) fn validate(snapshot: &Snapshot, candidate_root: &Path) -> Result<(), String> {
    if snapshot.schema_version != 1 || !candidate_root.is_absolute() {
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
        if let Some(pending) = &job.pending
            && (!matches!(job.phase, Phase::Active | Phase::Paused)
                || metrics.actions.checked_add(1) != Some(pending.sequence)
                || metrics.decisions < pending.sequence)
        {
            return Err("engineering checkpoint pending cursor is inconsistent".into());
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
