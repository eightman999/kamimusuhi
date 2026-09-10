"""Experiment lifecycle: pause / resume / checkpoint / stop / resume-from-checkpoint.

Checkpoint = WAL truncate + manifest JSON at
``<runs>/<exp>/checkpoints/<ts>-<reason>.json`` + a ``checkpoints`` row.
Resume = open the existing DB, RUNNING -> UNKNOWN (then requeue per
policy), reload rng state, workers re-register.
"""
from __future__ import annotations

import json
import time
from pathlib import Path

from ..storage import models as M


def checkpoint(service, reason: str = "manual") -> dict:
    db = service.db
    db.wal_checkpoint_truncate()
    ts = int(time.time())
    ckpt_dir = Path(service.run_dir) / "checkpoints"
    ckpt_dir.mkdir(parents=True, exist_ok=True)
    manifest = {
        "experiment_id": service.experiment_id,
        "created_at": db._now(),
        "reason": reason,
        "git_commit": service.git_commit,
        "config_hash": service.config_hash,
        "counters": service.counters(),
        "rng_state": service.dump_rng_state(),
        "queue_counts": db.job_counts(service.experiment_id),
        "worker_ids": [w["worker_id"] for w in
                       db.list_workers(service.experiment_id)],
        "db_sha256": db.db_sha256(),
        "last_event_id": db.last_event_id(service.experiment_id),
    }
    path = ckpt_dir / f"{ts}-{reason}.json"
    path.write_text(json.dumps(manifest, indent=2))
    db.insert_checkpoint(service.experiment_id, reason, manifest, str(path))
    db.emit(service.experiment_id, M.EV_CHECKPOINT_CREATED,
            payload={"reason": reason, "path": str(path)},
            source="coordinator")
    return manifest


def pause(service) -> None:
    db = service.db
    service.paused = True
    db.set_experiment_status(service.experiment_id, M.EXP_PAUSED)
    db.emit(service.experiment_id, M.EV_PAUSED, source="coordinator")


def resume(service) -> None:
    db = service.db
    service.paused = False
    db.set_experiment_status(service.experiment_id, M.EXP_RUNNING)
    db.emit(service.experiment_id, M.EV_RESUMED, source="coordinator")


def stop(service, timeout_s: float | None = None) -> None:
    """Graceful stop: stop claims, wait for RUNNING jobs, checkpoint, exit."""
    db = service.db
    cfg_timeout = float(service.config.get("stop", {}).get("timeout_s", 120))
    timeout = cfg_timeout if timeout_s is None else float(timeout_s)
    service.paused = True
    db.set_experiment_status(service.experiment_id, M.EXP_STOPPING)
    db.emit(service.experiment_id, M.EV_STOPPING,
            payload={"timeout_s": timeout}, source="coordinator")
    deadline = time.time() + timeout
    while time.time() < deadline:
        running = db.job_counts(service.experiment_id).get(M.JOB_RUNNING, 0)
        if not running:
            break
        time.sleep(0.5)
    stuck = db.mark_running_unknown(service.experiment_id)
    if stuck:
        db.emit(service.experiment_id, M.EV_JOB_MARKED_UNKNOWN, "warn",
                {"job_ids": stuck, "reason": "stop_timeout"}, "coordinator")
    checkpoint(service, reason="stop")
    db.set_experiment_status(service.experiment_id, M.EXP_STOPPED)
    db.emit(service.experiment_id, M.EV_STOPPED, source="coordinator")
    service.request_shutdown()


def resume_experiment(service) -> None:
    """Bring a reopened experiment DB back to a runnable state."""
    db = service.db
    stuck = db.mark_running_unknown(service.experiment_id)
    if stuck:
        db.emit(service.experiment_id, M.EV_JOB_MARKED_UNKNOWN, "warn",
                {"job_ids": stuck, "reason": "coordinator_restart"},
                "coordinator")
    from . import jobs
    for job_id, action in jobs.requeue(db, service.experiment_id,
                                       service.config):
        db.emit(service.experiment_id, M.EV_JOB_REQUEUED,
                payload={"job_id": job_id, "action": action},
                source="coordinator")
    # offline all workers; they must re-register
    for w in db.list_workers(service.experiment_id):
        if w["status"] == "online":
            db.mark_worker_status(service.experiment_id, w["worker_id"],
                                  "offline")
            db.emit(service.experiment_id, M.EV_WORKER_OFFLINE,
                    payload={"worker_id": w["worker_id"]},
                    source="coordinator")
    db.set_experiment_status(service.experiment_id, M.EXP_RUNNING)
    service.paused = False
    db.emit(service.experiment_id, M.EV_RESUMED_FROM_CKPT,
            source="coordinator")
