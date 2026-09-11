"""Phase-level profiling of one evaluation (M1 §2).

M0 measured only "about 50 s per evaluation". M1 requires the cost to be
broken down before any optimisation is accepted, so every phase of the
evaluation path is timed under the *same* names everywhere (worker,
``mioba profile``, replay):

    genome_decode           Genome.from_json
    mutation_resolve        develop(): parameter mutations -> absolute params
    topology_construction   edge lists / coalesce / CSR build (cache miss)
    host_to_device          moving the built network onto the GPU
    state_init              v/g/delay ring buffer/refractory allocation + reset
    simulation_loop         the stepping loop itself
    cuda_sync               explicit torch.cuda.synchronize() cost
    metrics                 rate/activity aggregation for the summary
    db_write                coordinator-side evaluation insert
    telemetry               MIE sample collection / recording
    cleanup                 backend teardown, cache eviction

A ``PhaseTimer`` is cheap when disabled and never changes results: it
only measures. On CUDA the timer synchronises at phase boundaries when a
``sync`` callable is given, because torch kernel launches are
asynchronous and unsynchronised wall time would attribute the simulation
cost to whatever phase happens to read a tensor next.
"""
from __future__ import annotations

import time
from contextlib import contextmanager

PHASES = (
    "genome_decode",
    "mutation_resolve",
    "topology_construction",
    "host_to_device",
    "state_init",
    "simulation_loop",
    "cuda_sync",
    "metrics",
    "db_write",
    "telemetry",
    "cleanup",
)


class PhaseTimer:
    """Accumulates seconds and call counts per phase name.

    ``sync`` (e.g. ``torch.cuda.synchronize``) is invoked at both ends of
    every timed phase; the time it costs is attributed to ``cuda_sync``
    rather than to the phase, so the phase rows stay comparable between
    CPU and GPU runs.
    """

    def __init__(self, enabled: bool = True, sync=None, label: str | None = None):
        self.enabled = bool(enabled)
        self._sync = sync
        self.label = label
        self._seconds: dict[str, float] = {}
        self._calls: dict[str, int] = {}
        self._t0 = time.perf_counter()

    # ------------------------------------------------------------ record
    def add(self, name: str, seconds: float, calls: int = 1) -> None:
        if not self.enabled:
            return
        self._seconds[name] = self._seconds.get(name, 0.0) + float(seconds)
        self._calls[name] = self._calls.get(name, 0) + int(calls)

    def _do_sync(self) -> float:
        if self._sync is None:
            return 0.0
        t = time.perf_counter()
        self._sync()
        return time.perf_counter() - t

    @contextmanager
    def phase(self, name: str):
        if not self.enabled:
            yield self
            return
        pre = self._do_sync()
        t0 = time.perf_counter()
        try:
            yield self
        finally:
            post = self._do_sync()
            elapsed = time.perf_counter() - t0 - post
            self.add(name, max(0.0, elapsed))
            if pre or post:
                self.add("cuda_sync", pre + post)

    # ------------------------------------------------------------ report
    @property
    def wall_s(self) -> float:
        return time.perf_counter() - self._t0

    def to_dict(self) -> dict:
        wall = self.wall_s
        accounted = sum(self._seconds.values())
        return {
            "label": self.label,
            "wall_s": round(wall, 6),
            "accounted_s": round(accounted, 6),
            "unaccounted_s": round(max(0.0, wall - accounted), 6),
            "phases": {
                name: {"seconds": round(self._seconds[name], 6),
                       "calls": self._calls.get(name, 0),
                       "pct_of_wall": (round(100.0 * self._seconds[name] / wall, 2)
                                       if wall > 0 else None)}
                for name in sorted(self._seconds)
            },
        }


def merge_timings(timings: list[dict]) -> dict:
    """Sum a list of ``PhaseTimer.to_dict()`` results (e.g. one per
    replicate chunk) into one report."""
    seconds: dict[str, float] = {}
    calls: dict[str, int] = {}
    wall = 0.0
    for t in timings or []:
        wall += float(t.get("wall_s") or 0.0)
        for name, row in (t.get("phases") or {}).items():
            seconds[name] = seconds.get(name, 0.0) + float(row.get("seconds") or 0.0)
            calls[name] = calls.get(name, 0) + int(row.get("calls") or 0)
    accounted = sum(seconds.values())
    return {
        "label": "merged",
        "wall_s": round(wall, 6),
        "accounted_s": round(accounted, 6),
        "unaccounted_s": round(max(0.0, wall - accounted), 6),
        "phases": {
            name: {"seconds": round(seconds[name], 6), "calls": calls[name],
                   "pct_of_wall": (round(100.0 * seconds[name] / wall, 2)
                                   if wall > 0 else None)}
            for name in sorted(seconds)
        },
    }
