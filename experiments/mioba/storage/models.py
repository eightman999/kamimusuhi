"""Shared constants: event types, job/experiment statuses."""
from __future__ import annotations

# job lifecycle
JOB_QUEUED = "QUEUED"
JOB_RUNNING = "RUNNING"
JOB_SUCCEEDED = "SUCCEEDED"
JOB_FAILED = "FAILED"
JOB_CANCELLED = "CANCELLED"
JOB_UNKNOWN = "UNKNOWN"
JOB_TERMINAL = (JOB_SUCCEEDED, JOB_FAILED, JOB_CANCELLED)

# experiment lifecycle
EXP_CREATED = "created"
EXP_RUNNING = "running"
EXP_PAUSED = "paused"
EXP_STOPPING = "stopping"
EXP_STOPPED = "stopped"

# experiment_events.type (extensible plain strings)
EV_WORKER_JOINED = "worker_joined"
EV_WORKER_LOST = "worker_lost"
EV_WORKER_OFFLINE = "worker_offline"
EV_WORKER_FAILED = "worker_failed"
EV_CHECKPOINT_CREATED = "checkpoint_created"
EV_GENOME_BORN = "genome_born"
EV_EVALUATION_SUCCEEDED = "evaluation_succeeded"
EV_EVALUATION_FAILED = "evaluation_failed"
EV_JOB_MARKED_UNKNOWN = "job_marked_unknown"
EV_JOB_REQUEUED = "job_requeued"
EV_NEW_BEST = "new_best_candidate"
EV_NEW_CLADE = "new_clade_detected"
EV_SENSOR_ANOMALY = "sensor_anomaly"
EV_PAUSED = "experiment_paused"
EV_RESUMED = "experiment_resumed"
EV_STOPPING = "experiment_stopping"
EV_STOPPED = "experiment_stopped"
EV_STARTED = "experiment_started"
EV_RESUMED_FROM_CKPT = "experiment_resumed_from_checkpoint"
EV_RUNTIME_CONFIG_CHANGED = "runtime_config_changed"
EV_SCIENTIFIC_CONFIG_MISMATCH = "scientific_config_mismatch"
