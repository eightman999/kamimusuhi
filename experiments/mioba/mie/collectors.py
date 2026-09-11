"""MIE collectors: machine-interoception sensor sampling.

A Collector has ``name``, ``domain`` and ``collect() -> list[SensorEvent]``.
Deltas are computed per (source, signal_type) against the previous value.
normalized_value uses a per-signal scale table (temp/100, util/100,
fractions as-is).
"""
from __future__ import annotations

import socket
import time
from typing import Protocol

from ..genome.schema import utcnow
from .events import SensorEvent

# normalization scales per signal_type
SCALE = {
    "temperature_c": 100.0,
    "utilization_pct": 100.0,
    "cpu_percent": 100.0,
    "ram_percent": 100.0,
    "vram_used_fraction": 1.0,
    "ram_used_fraction": 1.0,
    "latency_ms": 1000.0,
}


class Collector(Protocol):
    name: str
    domain: str

    def collect(self) -> list[SensorEvent]:
        ...


class Normalizer:
    """Tracks previous values per (source, signal_type) for deltas."""

    def __init__(self):
        self._prev: dict[tuple[str, str], float] = {}

    def apply(self, ev: SensorEvent) -> SensorEvent:
        if ev.value is not None:
            key = (ev.source, ev.signal_type)
            prev = self._prev.get(key)
            ev.delta = None if prev is None else ev.value - prev
            self._prev[key] = ev.value
            scale = SCALE.get(ev.signal_type)
            if scale:
                ev.normalized_value = ev.value / scale
        return ev

    def wrap(self, collector: Collector) -> "NormalizedCollector":
        return NormalizedCollector(collector, self)


class NormalizedCollector:
    def __init__(self, inner: Collector, normalizer: Normalizer):
        self.inner = inner
        self.normalizer = normalizer
        self.name = inner.name
        self.domain = inner.domain

    def collect(self) -> list[SensorEvent]:
        return [self.normalizer.apply(e) for e in self.inner.collect()]


class GpuCollector:
    name = "gpu"
    domain = "machine_interoception"

    def collect(self) -> list[SensorEvent]:
        from ..workers.gpu_info import query_gpus
        events = []
        for gpu in query_gpus():
            src = f"gpu:{gpu['uuid']}"
            for sig, key in (("temperature_c", "temperature_c"),
                             ("utilization_pct", "utilization_pct")):
                events.append(SensorEvent(
                    timestamp=utcnow(), source=src,
                    domain=self.domain, signal_type=sig,
                    value=gpu.get(key), metadata={"index": gpu["index"],
                                                  "name": gpu["name"]}))
            total = gpu.get("memory_total_mb") or 0
            used = gpu.get("memory_used_mb") or 0
            events.append(SensorEvent(
                timestamp=utcnow(), source=src, domain=self.domain,
                signal_type="vram_used_fraction",
                value=(used / total) if total else None,
                metadata={"index": gpu["index"], "used_mb": used,
                          "total_mb": total}))
        return events


class CpuCollector:
    name = "cpu"
    domain = "machine_interoception"

    def collect(self) -> list[SensorEvent]:
        try:
            import psutil
            val = psutil.cpu_percent(interval=None)
        except ImportError:
            val = None
        return [SensorEvent(timestamp=utcnow(), source="host:cpu",
                            domain=self.domain, signal_type="cpu_percent",
                            value=val)]


class RamCollector:
    name = "ram"
    domain = "machine_interoception"

    def collect(self) -> list[SensorEvent]:
        try:
            import psutil
            mem = psutil.virtual_memory()
            pct, frac = mem.percent, mem.percent / 100.0
        except ImportError:
            pct = frac = None
        return [SensorEvent(timestamp=utcnow(), source="host:ram",
                            domain=self.domain, signal_type="ram_percent",
                            value=pct,
                            metadata={"used_fraction": frac})]


class NetworkLatencyCollector:
    """Optional: TCP connect time to a host:port. Disabled by default."""

    name = "network_latency"
    domain = "network"

    def __init__(self, host: str = "127.0.0.1", port: int = 80,
                 timeout_s: float = 2.0):
        self.host = host
        self.port = int(port)
        self.timeout_s = timeout_s

    def collect(self) -> list[SensorEvent]:
        t0 = time.perf_counter()
        ok, val = True, None
        try:
            with socket.create_connection((self.host, self.port),
                                          timeout=self.timeout_s):
                val = (time.perf_counter() - t0) * 1000.0
        except OSError:
            ok = False
        return [SensorEvent(timestamp=utcnow(),
                            source=f"net:{self.host}:{self.port}",
                            domain=self.domain, signal_type="latency_ms",
                            value=val, confidence=1.0 if ok else 0.0,
                            metadata={"reachable": ok})]


BUILTIN = {
    "gpu": GpuCollector,
    "cpu": CpuCollector,
    "ram": RamCollector,
    "network_latency": NetworkLatencyCollector,
}
