"""Virtual disturbances (M1 §5-§7).

M0 ran every individual in one constant environment, so there was nothing
for an organism to be *better at* — only a fixed rate to drift towards.
M1 injects crises: the episode is perturbed at a seeded, non-fixed time,
and what the organism does about it is the thing under selection.

Two separations matter and are load-bearing.

**Virtual, not physical.** Disturbances are sampled from the evaluation's
disturbance seed and never read the host. The machine's real temperature,
RAM and GPU load are recorded as telemetry and shown in the Observatory,
but they never reach the network and never touch fitness — otherwise an
individual's score would depend on whether it landed on the P100 or the
3060, and the run would stop being an experiment (M1 §2.4-10). Nothing
here overheats or starves a real machine either: the pressure is a number
in a simulated environment.

**State, not reflex.** A disturbance presents the network with sensor
values *and* real consequences (noise, dropout, latency, a shrinking
compute budget). It never prescribes a response. There is no
"flee when hot" rule anywhere in this package; which individuals cope is
the experiment's question, not its input.

Timing is deliberately not fixed. An organism that met a crisis at the
same step of every episode could adapt to the clock instead of to the
crisis, so onsets are drawn uniformly in the middle 20-80% of the
episode, and episodes come in three flavours (none / single / multiple)
so that "no disturbance" is a control condition inside the same run.
"""
from __future__ import annotations

import random
from dataclasses import asdict, dataclass, field

# The disturbance kinds M1 requires. `compound` is not a kind of its own:
# it is two of the others overlapping, which is how a real crisis arrives.
KINDS = ("thermal", "memory_squeeze", "sensor_dropout", "sensor_noise",
         "latency_spike")

SEVERITIES = ("mild", "medium", "severe")

DEFAULTS = {
    "enabled": True,
    # how many disturbances an episode gets: none is a control condition
    "count_weights": {"0": 0.25, "1": 0.5, "2": 0.2, "3": 0.05},
    "kind_weights": {k: 1.0 for k in KINDS},
    "severity_weights": {"mild": 0.5, "medium": 0.35, "severe": 0.15},
    # onset anywhere in the middle of the episode, never at a fixed step
    "onset_window": [0.2, 0.8],
    "duration_frac": [0.1, 0.35],
    # magnitude of each severity, in the channel's own normalised units
    "severity_magnitude": {"mild": 0.3, "medium": 0.6, "severe": 0.95},
    # probability that a sampled disturbance is made to overlap the
    # previous one instead of being placed independently
    "compound_probability": 0.25,
}


@dataclass
class DisturbanceEvent:
    kind: str
    severity: str
    magnitude: float          # 0..1 in the affected channel's units
    start_frac: float         # fraction of the episode
    end_frac: float
    compound_with: str | None = None

    def active(self, t_frac: float) -> bool:
        return self.start_frac <= t_frac < self.end_frac

    def to_dict(self) -> dict:
        return asdict(self)


@dataclass
class DisturbanceSchedule:
    events: list = field(default_factory=list)
    seed: int = 0

    @property
    def kinds(self) -> list[str]:
        return sorted({e.kind for e in self.events})

    def active(self, t_frac: float) -> list:
        return [e for e in self.events if e.active(t_frac)]

    def window(self) -> tuple[float, float] | None:
        """(first onset, last end) over all events, or None."""
        if not self.events:
            return None
        return (min(e.start_frac for e in self.events),
                max(e.end_frac for e in self.events))

    def to_dict(self) -> dict:
        return {"seed": self.seed, "n_events": len(self.events),
                "kinds": self.kinds,
                "events": [e.to_dict() for e in self.events]}


# A weight map is a distribution, and partially overriding a distribution
# is ill-defined: writing `count_weights: {"1": 1.0}` means "always one",
# not "always one, plus the default 25% chance of none". Weight maps are
# therefore replaced wholesale; everything else merges.
_REPLACED_KEYS = ("count_weights", "kind_weights", "severity_weights",
                  "severity_magnitude")


def merged_config(config: dict | None) -> dict:
    cfg = {k: (dict(v) if isinstance(v, dict) else v)
           for k, v in DEFAULTS.items()}
    user = ((config or {}).get("environment") or {}).get("disturbance") or {}
    for key, value in user.items():
        if (isinstance(value, dict) and isinstance(cfg.get(key), dict)
                and key not in _REPLACED_KEYS):
            cfg[key] = {**cfg[key], **value}
        else:
            cfg[key] = value
    return cfg


def _weighted(rng: random.Random, options: dict):
    items = [(k, float(v)) for k, v in options.items() if float(v) > 0]
    if not items:
        return None
    total = sum(w for _, w in items)
    r = rng.random() * total
    upto = 0.0
    for key, weight in items:
        upto += weight
        if r <= upto:
            return key
    return items[-1][0]


def sample_schedule(seed: int, config: dict | None = None
                    ) -> DisturbanceSchedule:
    """Draw one episode's disturbances from ``seed``.

    Pure function of (seed, config): the same evaluation replayed on any
    device meets the same crises at the same times, which is what makes
    "this individual recovered and that one did not" a comparison rather
    than an anecdote.
    """
    cfg = merged_config(config)
    if not cfg.get("enabled", True):
        return DisturbanceSchedule(events=[], seed=int(seed))
    rng = random.Random(int(seed))
    n = int(_weighted(rng, cfg["count_weights"]) or 0)
    lo_on, hi_on = cfg["onset_window"]
    lo_dur, hi_dur = cfg["duration_frac"]
    magnitudes = cfg["severity_magnitude"]

    events: list[DisturbanceEvent] = []
    for _ in range(n):
        kind = _weighted(rng, cfg["kind_weights"]) or KINDS[0]
        severity = _weighted(rng, cfg["severity_weights"]) or "mild"
        duration = rng.uniform(float(lo_dur), float(hi_dur))
        compound_with = None
        if events and rng.random() < float(cfg["compound_probability"]):
            # overlap the previous event: two pressures at once is a
            # different problem from two in sequence
            prev = events[-1]
            start = min(float(hi_on),
                        rng.uniform(prev.start_frac, prev.end_frac))
            compound_with = prev.kind
        else:
            start = rng.uniform(float(lo_on), float(hi_on))
        events.append(DisturbanceEvent(
            kind=kind, severity=severity,
            magnitude=float(magnitudes.get(severity, 0.5)),
            start_frac=round(start, 6),
            end_frac=round(min(1.0, start + duration), 6),
            compound_with=compound_with))
    events.sort(key=lambda e: e.start_frac)
    return DisturbanceSchedule(events=events, seed=int(seed))
