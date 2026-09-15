"""Bounded, best-effort observation of the primary evaluation.

Only the simulator thread reads tensors. The sender receives detached
Python values and never changes simulator state or evaluation outcomes.
"""
from __future__ import annotations

from collections import deque
import logging
import math
import threading
import time

LOG = logging.getLogger(__name__)


def evenly_spaced_indices(n_neurons: int, sample_size: int = 256) -> list[int]:
    if (type(n_neurons) is not int or n_neurons < 1
            or type(sample_size) is not int or not 1 <= sample_size <= 512):
        raise ValueError("invalid neural activity sample size")
    count = min(n_neurons, sample_size)
    if count == 1:
        return [0]
    return [i * (n_neurons - 1) // (count - 1) for i in range(count)]


class NeuralActivityPublisher:
    """At most two waiting frames; old frames yield to recent activity.

    close() waits at most close_timeout_s. The HTTP client must honour
    its request timeout; the daemon is a final safeguard for a broken
    transport, not a reason to block an evaluation indefinitely.
    """

    def __init__(self, client, request_timeout_s: float = 0.3,
                 close_timeout_s: float = 1.0):
        self.client = client
        self.request_timeout_s = request_timeout_s
        self.close_timeout_s = close_timeout_s
        self._condition = threading.Condition()
        self._pending = deque(maxlen=2)
        self._closed = False
        self.sent = 0
        self.failed = 0
        self.dropped = 0
        self._thread = threading.Thread(target=self._send_loop,
                                        name="mioba-neural-activity", daemon=True)
        self._thread.start()

    def publish(self, frame: dict) -> None:
        with self._condition:
            if self._closed:
                return
            if len(self._pending) == self._pending.maxlen:
                self.dropped += 1
            self._pending.append(frame)
            self._condition.notify()

    def _send_loop(self):
        while True:
            with self._condition:
                while not self._pending and not self._closed:
                    self._condition.wait()
                if not self._pending:
                    return
                frame = self._pending.popleft()
            try:
                response = self.client.post("/api/worker/neural-activity",
                                            json=frame,
                                            timeout=self.request_timeout_s)
                if 200 <= response.status_code < 300:
                    self.sent += 1
                else:
                    self.failed += 1
            except Exception:
                self.failed += 1

    def close(self):
        with self._condition:
            self._closed = True
            # Flush only the latest pending observation at shutdown.
            while len(self._pending) > 1:
                self._pending.popleft()
                self.dropped += 1
            self._condition.notify_all()
        self._thread.join(timeout=self.close_timeout_s)
        if self._thread.is_alive():
            with self._condition:
                self.dropped += len(self._pending)
                self._pending.clear()
            LOG.warning("neural activity sender exceeded shutdown timeout")


class NeuralActivityObserver:
    """Count differences for lane zero of each current execution batch.

    Explicit begin_batch establishes a new counter origin after initialize.
    An unexpected rewind/counter decrease disables that batch and reports
    a diagnostic; it is never clamped into an apparently valid window.
    """

    def __init__(self, job: dict, worker_id: str, publish,
                 sample_size: int = 256, interval_s: float = 0.5,
                 clock=time.monotonic):
        evenly_spaced_indices(1, sample_size)
        if not math.isfinite(interval_s) or interval_s < 0:
            raise ValueError("invalid neural activity interval")
        self.identity = {"schema_version": 1,
                         **{key: job[key] for key in
                            ("experiment_id", "genome_id", "job_id", "backend")},
                         "worker_id": worker_id,
                         "attempt": int(job.get("attempt") or 0),
                         "phase": "evaluation"}
        self.publish = publish
        self.sample_size = sample_size
        self.interval_s = interval_s
        self.clock = clock
        self.sequence = 0
        self.evaluation_pass = 0
        self.last_error = None
        self._active = False

    def start_pass(self, evaluation_pass: int):
        self.evaluation_pass = evaluation_pass
        self._active = False

    def _disable(self, reason):
        self._active = False
        self.last_error = reason
        LOG.warning("neural activity observation disabled for batch: %s", reason)

    def begin_batch(self, backend, lanes, seeds):
        self._active = False
        if backend.name not in {"mock", "torch"}:
            return
        try:
            self._indices = evenly_spaced_indices(backend.n, self.sample_size)
            self._previous = backend.get_neuron_activity(self._indices, lane=0)
            if self._previous is None:
                return
            self._validate(self._previous)
            self.replicate_index = int(lanes[0])
            self.replicate_seed = int(seeds[self.replicate_index])
            self._last_wall = self.clock()
            self._active = True
        except Exception:
            self._disable("initial snapshot unavailable")

    def _validate(self, snapshot):
        t = snapshot["t_ms"]
        if (not math.isfinite(t) or t < 0
                or snapshot["neuron_indices"] != self._indices
                or len(snapshot["spike_counts"]) != len(self._indices)
                or any(type(c) is not int or c < 0
                       for c in snapshot["spike_counts"])):
            raise ValueError("invalid neuron activity snapshot")

    def sample(self, backend, final: bool = False):
        if not self._active:
            return
        try:
            wall = self.clock()
            if not final and wall - self._last_wall < self.interval_s:
                return
            now = backend.get_neuron_activity(self._indices, lane=0)
            self._validate(now)
            before = self._previous
            deltas = [a - b for a, b in zip(now["spike_counts"],
                                           before["spike_counts"])]
            if now["t_ms"] < before["t_ms"] or any(c < 0 for c in deltas):
                self._disable("simulation time or spike counter rewound")
                return
            if now["t_ms"] == before["t_ms"]:
                if any(deltas):
                    self._disable("spike counts changed without simulated time")
                return
            self.sequence += 1
            frame = {**self.identity, "evaluation_pass": self.evaluation_pass,
                     "sequence": self.sequence,
                     "replicate_index": self.replicate_index,
                     "replicate_seed": self.replicate_seed,
                     "n_neurons": now["n_neurons"], "n_base": now["n_base"],
                     "neuron_indices": list(self._indices),
                     "spike_counts": deltas,
                     "window_start_ms": before["t_ms"],
                     "window_end_ms": now["t_ms"]}
            self._previous = now
            self._last_wall = wall
            self.publish(frame)
        except Exception:
            self._disable("snapshot or local publisher unavailable")
