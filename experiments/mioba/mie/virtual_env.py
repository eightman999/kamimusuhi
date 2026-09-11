"""The organism's environment: machine state, crises, and their cost.

This is the half of M1 §5-§7 that the network can actually perceive and
be hurt by. :mod:`experiments.mioba.mie.disturbance` decides *when* a
crisis happens; this module decides *what it is like to be in one*.

Channels (M1 §5.1). Five normalised signals, each 0 = safe, 0.5 =
nominal, 1 = dangerous:

    virtual_temperature      thermal pressure
    virtual_memory_pressure  how little working memory is left
    virtual_compute_pressure how little compute budget is left
    virtual_latency          how delayed perception and action are
    virtual_sensor_quality   how trustworthy the senses are (inverted:
                             high value = degraded)

They are fed to the network as firing rates on reserved input neurons,
alongside the task stimulus. The organism therefore *has* interoception —
it can see its own state — but nothing tells it what that state means.

Consequences (M1 §6.1, §11). A crisis is not a number to read; it costs
something:

    sensor_noise      rates are perturbed, so perception gets unreliable
    sensor_dropout    channels go silent, so information disappears
    latency           the drive is applied from a delayed buffer
    compute budget    slices are skipped, so the organism loses time
    memory budget     the usable stimulus population shrinks

Homeostatic debt (M1 §6). Deviation that is *survived and corrected*
should cost less than deviation that is left to run. So violation
accumulates:

    debt[t+1] = decay * debt[t] + violation[t]

with violation the amount by which channels sit outside their safe band.
Debt is not a score — it is an environment variable with teeth: it feeds
back into sensor quality and compute budget, and past a threshold the
episode ends early. An organism that ignores a rising crisis therefore
loses episode time it needed for the task, which is a consequence rather
than a rule about how to behave.

Nothing here reads the host machine. Real GPU temperature and RAM live in
telemetry and the Observatory; letting them into this loop would make
fitness depend on which GPU a job landed on (M1 §2.4-10). Feeding
physical values in is possible but gated: it requires an explicit config
opt-in *and* a trace path, so such a run stays replayable.
"""
from __future__ import annotations

import random
from dataclasses import dataclass, field

from .disturbance import DisturbanceSchedule, sample_schedule

CHANNELS = ("virtual_temperature", "virtual_memory_pressure",
            "virtual_compute_pressure", "virtual_latency",
            "virtual_sensor_quality")

NOMINAL = 0.5          # the resting value of every channel
SAFE_BAND = 0.25       # |value - NOMINAL| within this is not a violation

DEFAULTS = {
    "enabled": True,
    # neurons reserved per sensor channel, driven at a rate proportional
    # to the channel value
    "sensor_neurons_per_channel": 64,
    "sensor_rate_hz": 80.0,
    "slices": 25,                    # environment updates per episode
    "debt_decay": 0.9,
    "debt_gain": 1.0,
    # debt above this ends the episode; the lost slices are scored as
    # failures, so terminating early can never be a way to win
    "debt_terminate": 6.0,
    # how strongly debt feeds back into the environment
    "debt_to_sensor_noise": 0.05,
    "debt_to_compute_loss": 0.04,
    "max_sensor_noise": 0.6,
    "max_dropout_fraction": 0.8,
    "max_latency_slices": 4,
    "min_compute_fraction": 0.4,
    "min_memory_fraction": 0.4,
    # physical telemetry as a scientific input is off, and stays off
    # unless a trace path is given (M1 §2.4-10)
    "use_physical_sensors": False,
    "physical_sensor_trace": None,
}


class PhysicalSensorsNotTraced(RuntimeError):
    """Physical machine state was requested as a *scientific* input
    without a trace path. Real telemetry is not reproducible by itself:
    without a recording of what the machine was doing, the evaluation
    cannot be replayed, and fitness would silently depend on the host."""


def merged_config(config: dict | None) -> dict:
    cfg = dict(DEFAULTS)
    user = ((config or {}).get("environment") or {}).get("virtual") or {}
    cfg.update({k: v for k, v in user.items()})
    return cfg


@dataclass
class EnvState:
    t_frac: float = 0.0
    channels: dict = field(default_factory=lambda: {c: NOMINAL
                                                    for c in CHANNELS})
    debt: float = 0.0
    violation: float = 0.0
    active: list = field(default_factory=list)
    terminated: bool = False

    def to_dict(self) -> dict:
        return {"t_frac": round(self.t_frac, 4),
                "channels": {k: round(v, 4) for k, v in self.channels.items()},
                "debt": round(self.debt, 4),
                "violation": round(self.violation, 4),
                "active": list(self.active),
                "terminated": self.terminated}


class VirtualEnvironment:
    """One episode's environment for one replicate lane.

    ``seed`` must be the replicate's disturbance seed (see
    ``fba/seeds.py``) so that lanes sample different crises rather than
    repeating one, while any replay of the same evaluation meets the same
    ones.
    """

    def __init__(self, seed: int, config: dict | None, n_base: int,
                 task_neurons: tuple[int, int] = (0, 0)):
        self.config = merged_config(config)
        self.cfg_all = config or {}
        self.seed = int(seed)
        self.n_base = int(n_base)
        self.schedule: DisturbanceSchedule = sample_schedule(seed, config)
        self._rng = random.Random(seed ^ 0x5EED)
        self.state = EnvState()
        self.history: list[dict] = []
        self._latency_buffer: list[dict] = []

        if self.config.get("use_physical_sensors"):
            if not self.config.get("physical_sensor_trace"):
                raise PhysicalSensorsNotTraced(
                    "environment.virtual.use_physical_sensors requires "
                    "physical_sensor_trace: physical machine state is not "
                    "reproducible unless it is recorded")

        # neuron layout: [task stimulus | one block per sensor channel]
        self.task_range = task_neurons
        per = int(self.config["sensor_neurons_per_channel"])
        start = max(task_neurons[1], 0)
        self.sensor_ranges: dict[str, tuple[int, int]] = {}
        for name in CHANNELS:
            end = min(self.n_base, start + per)
            self.sensor_ranges[name] = (start, end)
            start = end
        self.n_slices = max(1, int(self.config["slices"]))

    # ------------------------------------------------------------ update
    def advance(self, t_frac: float) -> EnvState:
        """Move the environment to ``t_frac`` of the episode."""
        cfg = self.config
        active = self.schedule.active(t_frac)
        channels = {c: NOMINAL for c in CHANNELS}
        for ev in active:
            if ev.kind == "thermal":
                channels["virtual_temperature"] = min(
                    1.0, NOMINAL + ev.magnitude / 2 + ev.magnitude / 2)
            elif ev.kind == "memory_squeeze":
                channels["virtual_memory_pressure"] = min(
                    1.0, NOMINAL + ev.magnitude)
            elif ev.kind == "latency_spike":
                channels["virtual_latency"] = min(1.0, NOMINAL + ev.magnitude)
            elif ev.kind in ("sensor_noise", "sensor_dropout"):
                channels["virtual_sensor_quality"] = min(
                    1.0, NOMINAL + ev.magnitude)
        # debt is itself a load: a long-running crisis crowds out compute
        channels["virtual_compute_pressure"] = min(
            1.0, NOMINAL + float(cfg["debt_to_compute_loss"])
            * self.state.debt)

        violation = sum(max(0.0, abs(v - NOMINAL) - SAFE_BAND)
                        for v in channels.values())
        debt = (float(cfg["debt_decay"]) * self.state.debt
                + float(cfg["debt_gain"]) * violation)
        terminated = debt >= float(cfg["debt_terminate"])
        self.state = EnvState(t_frac=t_frac, channels=channels, debt=debt,
                              violation=violation,
                              active=[e.kind for e in active],
                              terminated=terminated)
        self.history.append(self.state.to_dict())
        return self.state

    # ------------------------------------------------------- consequences
    def consequences(self) -> dict:
        """What the current state costs, before the network reacts."""
        cfg = self.config
        st = self.state
        quality = st.channels["virtual_sensor_quality"]
        noise = min(float(cfg["max_sensor_noise"]),
                    max(0.0, quality - NOMINAL) * 2
                    + float(cfg["debt_to_sensor_noise"]) * st.debt)
        dropout = 0.0
        if "sensor_dropout" in st.active:
            dropout = min(float(cfg["max_dropout_fraction"]),
                          max(0.0, quality - NOMINAL) * 2)
        latency_slices = int(round(
            max(0.0, st.channels["virtual_latency"] - NOMINAL) * 2
            * float(cfg["max_latency_slices"])))
        compute_fraction = max(
            float(cfg["min_compute_fraction"]),
            1.0 - max(0.0, st.channels["virtual_compute_pressure"]
                      - NOMINAL) * 2)
        memory_fraction = max(
            float(cfg["min_memory_fraction"]),
            1.0 - max(0.0, st.channels["virtual_memory_pressure"]
                      - NOMINAL) * 2)
        return {"sensor_noise": round(noise, 4),
                "sensor_dropout": round(dropout, 4),
                "latency_slices": latency_slices,
                "compute_fraction": round(compute_fraction, 4),
                "memory_fraction": round(memory_fraction, 4),
                "terminate": st.terminated}

    # -------------------------------------------------------------- drive
    def drive(self, task_rate_hz: float) -> dict:
        """The input the network receives this slice.

        Task stimulus and interoceptive channels are perturbed by the same
        consequences, because a crisis degrades perception as such — not
        only the part of it that is convenient to degrade.
        """
        cons = self.consequences()
        rates: dict[str, float] = {}
        silence: list[int] = []

        lo, hi = self.task_range
        if hi > lo:
            span = hi - lo
            usable = max(1, int(span * cons["memory_fraction"]))
            rate = task_rate_hz * self._noisy(1.0, cons["sensor_noise"])
            rates[f"slice:{lo}-{lo + usable}"] = max(0.0, rate)

        base = float(self.config["sensor_rate_hz"])
        for name, (s_lo, s_hi) in self.sensor_ranges.items():
            if s_hi <= s_lo:
                continue
            if self._rng.random() < cons["sensor_dropout"]:
                silence.extend(range(s_lo, s_hi))
                continue
            value = self._noisy(self.state.channels[name],
                                cons["sensor_noise"])
            rates[f"slice:{s_lo}-{s_hi}"] = max(0.0, base * value)

        drive = {"rates_hz": rates, "silence": silence}
        return self._delayed(drive, cons["latency_slices"])

    def _noisy(self, value: float, noise: float) -> float:
        if noise <= 0:
            return value
        return max(0.0, value * (1.0 + self._rng.uniform(-noise, noise)))

    def _delayed(self, drive: dict, slices: int) -> dict:
        """Perception arrives ``slices`` behind reality during a latency
        spike: the network acts on a stale picture, which is the actual
        cost of latency rather than a penalty term."""
        self._latency_buffer.append(drive)
        if slices <= 0:
            self._latency_buffer = self._latency_buffer[-1:]
            return drive
        idx = max(0, len(self._latency_buffer) - 1 - slices)
        keep = slices + 1
        self._latency_buffer = self._latency_buffer[-keep:]
        idx = max(0, len(self._latency_buffer) - 1 - slices)
        return self._latency_buffer[idx]

    # ------------------------------------------------------------ report
    def summary(self) -> dict:
        peak = max((h["debt"] for h in self.history), default=0.0)
        return {
            "seed": self.seed,
            "schedule": self.schedule.to_dict(),
            "slices": self.n_slices,
            "sensor_ranges": {k: list(v) for k, v in
                              self.sensor_ranges.items()},
            "final_debt": round(self.state.debt, 4),
            "peak_debt": round(peak, 4),
            "mean_debt": round(sum(h["debt"] for h in self.history)
                               / max(1, len(self.history)), 4),
            "terminated_early": self.state.terminated,
            "history": self.history,
        }
