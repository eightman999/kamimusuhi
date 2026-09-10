"""E-series adapter contract + reference implementation on the mock path.

Translates K0-E2 style sensor events into MIE SensorEvents and into FBA
input drives, and FBA population activity back into an E action index.
Supports ``delay_steps`` queueing for delayed returned information.
"""
from __future__ import annotations

from collections import deque
from dataclasses import dataclass

from ..genome.schema import utcnow
from .events import SensorEvent

E_ACTIONS = ("IGNORE", "WAIT", "ORIENT", "OBSERVE", "RECALL",
             "INVOKE_LANGUAGE")

# The 16 K0-E2 sensor channel names (from exp/k0-e2-active-info env.py).
SENSOR_NAMES = ("motion", "motion_delta", "sound_energy", "light_level",
                "proximity", "touch", "temperature_delta", "task_relevance",
                "speech_activity", "acquired_value", "acquired_confidence",
                "resource_level", "language_cost", "language_reliability",
                "decision_ready", "language_latency")


@dataclass
class EStyleEvent:
    t: int
    channel: str            # one of SENSOR_NAMES
    value: float
    response_mode: str | None = None   # "correct" | "shuffled" | ...
    ood: str | None = None
    state_reset: bool = False


class EStreamToMieAdapter:
    """E-style events -> MIE SensorEvents (domain synthetic)."""

    def to_sensor_events(self, events: list[EStyleEvent]) -> list[SensorEvent]:
        out = []
        for ev in events:
            out.append(SensorEvent(
                timestamp=utcnow(), source="e-series", domain="synthetic",
                signal_type=ev.channel, value=float(ev.value),
                confidence=1.0,
                metadata={"t": ev.t, "response_mode": ev.response_mode,
                          "ood": ev.ood, "state_reset": ev.state_reset}))
        return out


@dataclass
class ChannelMapping:
    """channel -> disjoint slice of stimulus neurons, with rate scaling."""

    n_neurons: int
    rate_hz: float = 50.0
    channels: tuple[str, ...] = SENSOR_NAMES

    def slice_for(self, channel: str) -> tuple[int, int] | None:
        try:
            idx = self.channels.index(channel)
        except ValueError:
            return None
        width = max(1, self.n_neurons // len(self.channels))
        lo = idx * width
        hi = min(self.n_neurons, lo + width)
        return (lo, hi)


class MieToFbaInputAdapter:
    """MIE sensor events -> FBA InputDrive.

    With ``delay_steps > 0`` events are queued and emitted delay_steps
    calls later (delayed returned information).
    """

    def __init__(self, mapping: ChannelMapping, delay_steps: int = 0,
                 shuffle: bool = False):
        self.mapping = mapping
        self.delay_steps = int(delay_steps)
        self.shuffle = shuffle
        self._queue: deque[list[SensorEvent]] = deque()
        if shuffle:
            chans = list(mapping.channels)
            shift = chans[1:] + chans[:1]
            self._remap = dict(zip(mapping.channels, shift))
        else:
            self._remap = {}

    def to_input_drive(self, sensor_events: list[SensorEvent]) -> dict:
        if self.delay_steps:
            self._queue.append(sensor_events)
            if len(self._queue) <= self.delay_steps:
                return {"rates_hz": {}, "silence": []}
            sensor_events = self._queue.popleft()
        elif self._queue:
            self._queue.clear()

        rates: dict[str, float] = {}
        for ev in sensor_events:
            chan = self._remap.get(ev.signal_type, ev.signal_type)
            sl = self.mapping.slice_for(chan)
            if sl is None or ev.value is None:
                continue
            rates[f"slice:{sl[0]}-{sl[1]}"] = max(
                rates.get(f"slice:{sl[0]}-{sl[1]}", 0.0),
                float(ev.value) * self.mapping.rate_hz)
        return {"rates_hz": rates, "silence": []}


class FbaOutputToEActionAdapter:
    """FBA population activity -> an E_ACTIONS index per batch element.

    Placeholder readout: argmax over 6 disjoint readout groups drawn from
    the "all" activity vector (or per-group values if provided).
    """

    def to_action(self, activity: dict[str, list[float]]) -> list[int]:
        # activity maps group name -> per-batch rates
        groups = [g for g in activity if g.startswith("readout:")]
        if not groups and "all" in activity:
            vals = activity["all"]
            n = len(E_ACTIONS)
            # split the batch dimension into n buckets — placeholder
            return [int(abs(v) * 1000) % n for v in vals]
        if not groups:
            return [0] * (len(next(iter(activity.values()), [0])))
        batch = len(next(iter(activity.values())))
        out = []
        for b in range(batch):
            best_i, best_v = 0, float("-inf")
            for i, g in enumerate(sorted(groups)[: len(E_ACTIONS)]):
                v = activity[g][b]
                if v > best_v:
                    best_v, best_i = v, i
            out.append(best_i)
        return out
