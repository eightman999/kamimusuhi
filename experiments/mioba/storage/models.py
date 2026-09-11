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
# M1: best by the combined selection score, which is what the
# population is actually selected on (EV_NEW_BEST stays the
# M0-comparable placeholder so the two runs remain comparable)
EV_NEW_BEST_SELECTION = "new_best_selection_score"
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

# M1: a worker hit a *scheduling* resource limit (GPU OOM at the chosen
# slot count / execution batch), backed off and asked for the job to be
# re-run under the same evaluation seed. This is an operational event:
# it is never a FAILED evaluation and never a fitness penalty (M1 §2.4-7).
EV_RUNTIME_RESOURCE_RETRY = "runtime_resource_retry"
# an evaluation whose activity makes event-driven propagation a bad fit
EV_HIGH_ACTIVITY = "high_activity_individual"
# M1: every evaluated individual in a generation fell below
# fitness.minimum_viable_task_score, so the resource-efficiency pressure
# (section 8/9) contributed nothing to any of them. Not an error - but a
# silently inert objective is worse than a loud one, so it is an event.
EV_EFFICIENCY_GATE_INERT = "efficiency_gate_inert"

# M1 structural mutation outcomes (§4 / §13)
EV_STRUCTURAL_MUTATION = "structural_mutation"
EV_NEUTRAL_STRUCTURE = "neutral_structure"
EV_INVALID_STRUCTURE = "invalid_structure"

RETRY_RUNTIME_RESOURCE = "runtime_resource"
RETRY_INFRASTRUCTURE = "retryable_infrastructure_failure"
