"""Bounded, ephemeral observations of neurons during individual evaluation.

The cache never changes a job, evaluation, genome, RNG or fitness. Frames
are accepted only from the current owner/attempt and cannot resurrect a
finished or retried job. A cached frame is not proof of a successful run.
"""
from __future__ import annotations

import copy
import math
import re
import threading
import time
from collections import OrderedDict

from ..fba.replicates import replicate_seed
from ..storage import models as M

MAX_FRAME_BYTES = 65_536
MAX_SAMPLES = 512
TTL_MS = 5_000
_IDENTIFIER = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.:-]{0,255}\Z")
_FIELDS = {
    "schema_version", "experiment_id", "genome_id", "job_id", "worker_id",
    "attempt", "evaluation_pass", "sequence", "backend", "phase",
    "replicate_index", "replicate_seed", "n_neurons", "n_base",
    "neuron_indices", "spike_counts", "window_start_ms", "window_end_ms",
}


class InvalidFrame(ValueError):
    pass


class StaleFrame(ValueError):
    pass


def _uint(value, maximum=(1 << 63) - 1):
    return type(value) is int and 0 <= value <= maximum


def _time(value):
    return (type(value) in (float, int)
            and 0 <= value <= 1e12 and math.isfinite(value))


def validate_frame(frame):
    if not isinstance(frame, dict) or set(frame) != _FIELDS:
        raise InvalidFrame("invalid activity fields")
    if type(frame["schema_version"]) is not int or frame["schema_version"] != 1:
        raise InvalidFrame("unsupported activity schema")
    for key in ("experiment_id", "genome_id", "job_id", "worker_id"):
        if not isinstance(frame[key], str) or not _IDENTIFIER.fullmatch(frame[key]):
            raise InvalidFrame("invalid activity identity")
    if frame["backend"] not in ("mock", "torch") or frame["phase"] != "evaluation":
        raise InvalidFrame("unsupported activity source")
    if any(not _uint(frame[key]) for key in
           ("attempt", "sequence", "replicate_index", "replicate_seed")):
        raise InvalidFrame("invalid activity sequence or replicate")
    if not _uint(frame["evaluation_pass"], 64):
        raise InvalidFrame("invalid activity pass")
    total, base = frame["n_neurons"], frame["n_base"]
    if (not _uint(total, 10_000_000) or total == 0
            or not _uint(base, total)):
        raise InvalidFrame("invalid neuron population")
    indices, counts = frame["neuron_indices"], frame["spike_counts"]
    if (not isinstance(indices, list) or not isinstance(counts, list)
            or not 1 <= len(indices) <= MAX_SAMPLES or len(indices) != len(counts)
            or any(not _uint(i, total - 1) for i in indices)
            or any(not _uint(n) for n in counts)
            or indices != sorted(set(indices))):
        raise InvalidFrame("invalid neuron sample")
    start, end = frame["window_start_ms"], frame["window_end_ms"]
    if not _time(start) or not _time(end) or end <= start:
        raise InvalidFrame("invalid activity time window")


class NeuralActivityCache:
    """At most one current frame per job, with a bounded LRU of jobs.

    ``clock`` is monotonic; publisher clocks do not determine freshness.
    Lock order is database -> cache for both publication and inspection.
    ``max_jobs`` bounds memory even if the observer visits every generation.
    """

    def __init__(self, service, *, max_jobs=128, clock=time.monotonic,
                 wall_clock=time.time):
        self.service = service
        self.max_jobs = max_jobs
        self.clock = clock
        self.wall_clock = wall_clock
        self._lock = threading.Lock()
        self._frames = OrderedDict()

    def publish(self, frame):
        validate_frame(frame)
        service, db = self.service, self.service.db
        if frame["experiment_id"] != service.experiment_id:
            raise StaleFrame("activity experiment mismatch")
        with db._lock:
            job = db.get_job(frame["job_id"])
            if (job is None or job["experiment_id"] != service.experiment_id
                    or job["genome_id"] != frame["genome_id"]
                    or job["status"] != M.JOB_RUNNING
                    or job["claimed_by_worker"] != frame["worker_id"]
                    or job["attempt"] != frame["attempt"]
                    or job["backend"] != frame["backend"]):
                raise StaleFrame("activity job ownership mismatch")
            replicates = int(job.get("replicates") or 1)
            index = frame["replicate_index"]
            if (index >= replicates
                    or frame["replicate_seed"] != replicate_seed(job["seed"], index)):
                raise StaleFrame("activity replicate mismatch")
            with self._lock:
                previous = self._frames.get(frame["job_id"])
                if previous and previous["frame"]["attempt"] == frame["attempt"]:
                    old = previous["frame"]
                    if (frame["sequence"] <= old["sequence"]
                            or frame["evaluation_pass"] < old["evaluation_pass"]):
                        raise StaleFrame("activity is out of order")
                    if frame["evaluation_pass"] == old["evaluation_pass"]:
                        if frame["replicate_index"] < old["replicate_index"]:
                            raise StaleFrame("activity replicate moved backwards")
                        if frame["replicate_index"] == old["replicate_index"]:
                            if (frame["window_start_ms"] < old["window_end_ms"]
                                    or frame["neuron_indices"] != old["neuron_indices"]
                                    or frame["n_neurons"] != old["n_neurons"]
                                    or frame["n_base"] != old["n_base"]):
                                raise StaleFrame("activity window or population changed")
                saved = copy.deepcopy(frame)
                saved["received_at_unix_ms"] = int(self.wall_clock() * 1000)
                self._frames[frame["job_id"]] = {
                    "frame": saved, "received_monotonic": self.clock()}
                self._frames.move_to_end(frame["job_id"])
                while len(self._frames) > self.max_jobs:
                    self._frames.popitem(last=False)

    def snapshot(self, genome_id):
        service, db = self.service, self.service.db
        with db._lock:
            genome = db.conn.execute(
                "SELECT genome_id FROM genomes WHERE experiment_id=? AND genome_id=?",
                (service.experiment_id, genome_id)).fetchone()
            if genome is None:
                return None
            job = db.conn.execute(
                "SELECT * FROM evaluation_jobs WHERE experiment_id=? AND genome_id=? "
                "ORDER BY CASE WHEN status='RUNNING' THEN 0 ELSE 1 END, "
                "COALESCE(claimed_at, created_at) DESC, rowid DESC LIMIT 1",
                (service.experiment_id, genome_id)).fetchone()
            job = dict(job) if job else None
            with self._lock:
                cached = self._frames.get(job["job_id"]) if job else None
                if cached and cached["frame"]["attempt"] != job["attempt"]:
                    cached = None
                saved = copy.deepcopy(cached)
        frame = saved["frame"] if saved else None
        age = (max(0, int((self.clock() - saved["received_monotonic"]) * 1000))
               if saved else None)
        if frame and job["status"] == M.JOB_RUNNING and age <= TTL_MS:
            kind, status = "LIVE", "running"
        elif frame and job["status"] not in M.JOB_TERMINAL:
            kind, status = "RECORDED", "stale"
        elif frame:
            kind, status = "RECORDED", "recorded"
        else:
            kind = "UNAVAILABLE"
            status = ("waiting" if job and job["status"] in (M.JOB_QUEUED, M.JOB_RUNNING)
                      else "unavailable")
        return {
            "schema_version": 1, "kind": kind,
            "experiment_id": service.experiment_id, "genome_id": genome_id,
            "status": status, "observed_at_unix_ms": int(self.wall_clock() * 1000),
            "age_ms": age, "ttl_ms": TTL_MS,
            "job_status": job["status"] if job else None, "frame": frame,
        }
