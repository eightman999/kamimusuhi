"""Evaluation job queue policy.

State machine: QUEUED -> RUNNING -> SUCCEEDED|FAILED;
RUNNING -> UNKNOWN (stale worker / coordinator restart);
UNKNOWN -> QUEUED (requeue policy, attempt+1, max_attempts -> FAILED).
Pause only stops handing out claims. Never infer RUNNING->SUCCEEDED.
"""
from __future__ import annotations

from ..storage import models as M


def enqueue(db, experiment_id, genome_id, environment_id, seed, tier,
            backend, duration_ms, requested_traces, priority=0) -> str:
    return db.enqueue_job(experiment_id, genome_id, environment_id, seed,
                          tier, backend, duration_ms, requested_traces,
                          priority)


def claim(db, experiment_id, worker_id, paused: bool) -> dict | None:
    if paused:
        return None
    return db.claim_job(experiment_id, worker_id)


def finish(db, experiment_id, job, status, worker_id, error=None):
    db.finish_job(job["job_id"], status, worker_id, error)
    db.worker_finished_job(experiment_id, worker_id,
                           ok=(status == M.JOB_SUCCEEDED))


def mark_unknown(db, experiment_id, job_ids=None) -> list[str]:
    return db.mark_running_unknown(experiment_id, job_ids)


def requeue(db, experiment_id, config) -> list[tuple[str, str]]:
    if not config.get("jobs", {}).get("requeue_unknown", True):
        return []
    max_attempts = int(config.get("jobs", {}).get("max_attempts", 3))
    return db.requeue_unknown(experiment_id, max_attempts)
