"""Bounded, read-only observations for the dialogue adapter.

These are the latest completed evaluation's recorded measurements and a
derived genome structure. They are not live neural state, sensations or
Kamimusuhi's canonical identity. Unknown measurements are omitted, never
filled from defaults or inferred from generated prose.
"""
from __future__ import annotations

import json
import math
import re
from datetime import datetime, timezone

from ..storage import models as M


class InvalidSnapshot(ValueError):
    """Required stored metadata cannot satisfy the observation contract."""


_STATUSES = {M.EXP_CREATED, M.EXP_RUNNING, M.EXP_PAUSED,
             M.EXP_STOPPING, M.EXP_STOPPED}
_BACKENDS = {"mock", "torch", "genn"}
_IDENTIFIER = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.:-]{0,255}\Z")
_HASH = re.compile(r"[0-9a-f]{64}\Z")
_U64_MAX = (1 << 64) - 1


def _object(encoded) -> dict:
    if not isinstance(encoded, str):
        return {}
    try:
        value = json.loads(encoded)
    except (ValueError, TypeError, RecursionError):
        return {}
    return value if isinstance(value, dict) else {}


def _identifier(value) -> bool:
    return isinstance(value, str) and _IDENTIFIER.fullmatch(value) is not None


def _uint(value) -> bool:
    # bool is an int subclass in Python, but not a recorded count.
    return type(value) is int and 0 <= value <= _U64_MAX


def _number(value, minimum=None, maximum=None) -> bool:
    if type(value) not in (int, float):
        return False
    try:
        if not math.isfinite(value):
            return False
    except OverflowError:
        return False
    return ((minimum is None or value >= minimum)
            and (maximum is None or value <= maximum))


def _finished_at_unix_ms(value) -> int | None:
    if not isinstance(value, str):
        return None
    try:
        at = datetime.fromisoformat(value.replace("Z", "+00:00"))
        # A naive timestamp has no unambiguous epoch. Do not assign the
        # coordinator's timezone, or replace an unknown time with 'now'.
        if at.utcoffset() is None:
            return None
        stamp = int(at.timestamp() * 1000)
        return stamp if stamp >= 0 else None
    except (ValueError, TypeError, OverflowError, OSError):
        return None


def _structure(encoded) -> dict:
    raw = _object(encoded)
    out = {key: raw[key] for key in
           ("n_artificial_neurons", "n_enabled_organs", "n_enabled_attachments")
           if _uint(raw.get(key))}
    counts = raw.get("counts")
    if isinstance(counts, dict):
        clean = {key: counts[key] for key in
                 ("functional", "neutral_structure", "invalid_structure", "disabled")
                 if _uint(counts.get(key))}
        if clean:
            out["counts"] = clean
    return out


def _evaluation(row: dict) -> dict | None:
    if (not _identifier(row.get("evaluation_id"))
            or not _identifier(row.get("job_id"))
            or row.get("backend") not in _BACKENDS):
        return None
    out = {"kind": "RECORDED", "evaluation_id": row["evaluation_id"],
           "job_id": row["job_id"], "backend": row["backend"]}
    finished = _finished_at_unix_ms(row.get("finished_at"))
    if finished is not None:
        out["finished_at_unix_ms"] = finished
    scientific_hash = row.get("scientific_config_hash")
    if isinstance(scientific_hash, str) and _HASH.fullmatch(scientific_hash):
        out["scientific_config_hash"] = scientific_hash

    summary = _object(row.get("summary_json"))
    clean_summary = {key: summary[key] for key in
                     ("mean_rate_hz", "rate_std_hz", "t_ms")
                     if _number(summary.get(key), minimum=0)}
    if _number(summary.get("active_fraction"), minimum=0, maximum=1):
        clean_summary["active_fraction"] = summary["active_fraction"]
    if _uint(summary.get("spikes_total")):
        clean_summary["spikes_total"] = summary["spikes_total"]
    out["summary"] = clean_summary

    metrics = _object(row.get("metrics_json"))
    clean_metrics = {key: metrics[key] for key in
                     ("homeostasis_score", "disturbance_recovery_score", "terminated_fraction")
                     if _number(metrics.get(key), minimum=0, maximum=1)}
    if _number(metrics.get("peak_debt"), minimum=0):
        clean_metrics["peak_debt"] = metrics["peak_debt"]
    if _number(metrics.get("task_score")):
        clean_metrics["task_score"] = metrics["task_score"]
    out["metrics"] = clean_metrics
    return out


def organism_snapshot(db, experiment_id: str, genome_id: str) -> dict | None:
    """Read an existing service DB; opening/migrating a Database is forbidden.

    The coordinator is the single writer. Its existing connection lock keeps
    the identity and evaluation reads together without updating any state.
    Job/evaluation ownership is joined explicitly; a later failed or running
    job is not evidence of a completed evaluation.
    """
    with db._lock:
        row = db.conn.execute(
            "SELECT g.genome_id, g.experiment_id, g.generation, g.structure_json, "
            "x.status AS experiment_status FROM genomes g "
            "JOIN experiments x ON g.experiment_id=x.experiment_id "
            "WHERE g.experiment_id=? AND g.genome_id=?",
            (experiment_id, genome_id)).fetchone()
        if row is None:
            return None
        row = dict(row)
        latest = db.conn.execute(
            "SELECT e.evaluation_id, e.job_id, e.backend, e.finished_at, "
            "e.scientific_config_hash, e.summary_json, e.metrics_json "
            "FROM evaluations e JOIN evaluation_jobs j ON e.job_id=j.job_id "
            "AND e.experiment_id=j.experiment_id AND e.genome_id=j.genome_id "
            "WHERE e.experiment_id=? AND e.genome_id=? AND j.status=? "
            "ORDER BY e.finished_at DESC, e.rowid DESC LIMIT 1",
            (experiment_id, genome_id, M.JOB_SUCCEEDED)).fetchone()
        observed_at = int(datetime.now(timezone.utc).timestamp() * 1000)

    if (not _identifier(row["experiment_id"])
            or not _identifier(row["genome_id"])
            or not _uint(row["generation"])
            or row["experiment_status"] not in _STATUSES):
        raise InvalidSnapshot("invalid stored observation metadata")
    snapshot = {
        "schema_version": 1, "kind": "DERIVED",
        "state_scope": "recorded_evaluation",
        "observed_at_unix_ms": observed_at,
        "experiment_id": row["experiment_id"], "genome_id": row["genome_id"],
        "experiment_status": row["experiment_status"],
        "generation": row["generation"],
        "structure": _structure(row["structure_json"]),
        # Declaration of this adapter's current implementation, not an
        # experimental fitness claim or a capability inferred from activity.
        "implementation": {
            "action_feedback": "not_connected",
            "lifetime_plasticity": "configured_not_applied",
            "neural_state_scope": "evaluation_local",
        },
    }
    evaluation = _evaluation(dict(latest)) if latest is not None else None
    if evaluation is not None:
        snapshot["evaluation"] = evaluation
    return snapshot
