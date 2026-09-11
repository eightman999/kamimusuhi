"""H0 homeostasis environment dynamics.

All quantities live in [0, 1]. The module is pure numpy + dataclasses so that
dynamics can be unit-tested and replayed deterministically without torch.
"""

from __future__ import annotations

from dataclasses import dataclass, field, replace
from typing import Optional

import numpy as np

# ---------------------------------------------------------------------------
# Indices / names
# ---------------------------------------------------------------------------

INTERNAL_NAMES = ["energy", "fatigue", "novelty", "certainty", "temperature"]
EXTERNAL_NAMES = [
    "ambient_temperature",
    "environment_novelty",
    "resource_availability",
    "observation_quality",
    "hazard_level",
]
ACTION_NAMES = ["IGNORE", "WAIT", "REST", "EXPLORE", "OBSERVE"]

N_INTERNAL = len(INTERNAL_NAMES)
N_EXTERNAL = len(EXTERNAL_NAMES)
N_ACTIONS = len(ACTION_NAMES)
OBS_DIM = N_INTERNAL + N_EXTERNAL

ENERGY, FATIGUE, NOVELTY, CERTAINTY, TEMPERATURE = range(N_INTERNAL)
AMBIENT, ENV_NOV, RESOURCE, OBS_QUAL, HAZARD = range(N_EXTERNAL)
IGNORE, WAIT, REST, EXPLORE, OBSERVE = range(N_ACTIONS)

# Preferred (homeostatic) ranges per spec section 4.
PREFERRED_RANGES = {
    "energy": (0.4, 0.8),
    "fatigue": (0.0, 0.4),
    "novelty": (0.3, 0.7),
    "certainty": (0.4, 0.9),
    "temperature": (0.35, 0.65),
}

# Weights wi in E = sum_i wi * deviation(state_i, preferred_range_i).
ERROR_WEIGHTS = {
    "energy": 1.0,
    "fatigue": 1.0,
    "novelty": 0.7,
    "certainty": 0.7,
    "temperature": 1.5,
}

# Critical thresholds (spec section 9). (low, high); None = unbounded side.
CRITICAL_THRESHOLDS = {
    "energy": (0.05, None),
    "fatigue": (None, 0.98),
    "temperature": (0.05, 0.95),
}


# ---------------------------------------------------------------------------
# Parameters
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class EnvParams:
    """All tunable environment parameters.

    OOD configs override a small subset (see configs/ood_*.yaml).
    """

    episode_length: int = 1024

    # --- passive internal dynamics ---
    passive_energy_cost: float = 0.004
    passive_fatigue_rate: float = 0.0015
    novelty_relax_rate: float = 0.02      # toward 0.6 * environment_novelty
    novelty_baseline_gain: float = 0.6
    certainty_decay_rate: float = 0.01    # toward 0.3
    certainty_baseline: float = 0.3
    temp_relax_rate: float = 0.03         # toward ambient_temperature

    # --- action effects ---
    ignore_energy_cost: float = 0.001
    ignore_fatigue_rate: float = 0.001
    ignore_novelty_loss: float = 0.005
    ignore_certainty_loss: float = 0.008

    wait_energy_cost: float = 0.002
    wait_fatigue_recovery: float = 0.008
    wait_shelter_rate: float = 0.04       # pull of temperature toward 0.5

    rest_energy_gain: float = 0.05        # scaled by resource_availability
    rest_fatigue_recovery: float = 0.045
    rest_novelty_loss: float = 0.015
    rest_temp_coupling: float = 0.08      # faster equilibration to ambient

    explore_energy_cost: float = 0.012
    explore_fatigue_rate: float = 0.015
    explore_novelty_gain: float = 0.05
    explore_certainty_spread: float = 0.025  # uniform(-s, +s) certainty change
    microclimate_std: float = 0.15        # EXPLORE samples local microclimate
    microclimate_coupling: float = 0.10

    observe_energy_cost: float = 0.003
    observe_fatigue_rate: float = 0.006
    observe_certainty_gain: float = 0.08  # scaled by observation_quality
    observe_novelty_gain: float = 0.015

    # --- hazard ---
    explore_hazard_mult: float = 0.6      # P(hazard event) = hazard*mult
    ignore_hazard_mult: float = 0.4
    wait_hazard_mult: float = 0.15        # WAIT is not fully safe
    observe_hazard_reduction: float = 0.06  # OBSERVE lowers hazard_level
    hazard_energy_loss_lo: float = 0.03
    hazard_energy_loss_hi: float = 0.08
    hazard_fatigue_gain: float = 0.05
    hazard_temp_jolt: float = 0.10

    # --- endogenous resource coupling ---
    rest_resource_drain: float = 0.025    # REST depletes local resource
    explore_resource_gain: float = 0.05   # EXPLORE finds new resource

    # --- external dynamics ---
    ambient_mean: float = 0.5
    ambient_amp: float = 0.20             # sinusoid amplitude; ambient leaves
                                          # the comfort zone each cycle
    ambient_period: int = 800             # steps per cycle
    ambient_noise: float = 0.01

    novelty_mean: float = 0.4
    resource_mean: float = 0.65
    obs_quality_mean: float = 0.7
    hazard_mean: float = 0.15
    ou_relax: float = 0.02                # mean reversion for OU processes
    resource_ou_relax: float = 0.03       # resource recovers a bit faster
    ou_noise: float = 0.02
    hazard_ou_relax: float = 0.05
    hazard_ou_noise: float = 0.03
    hazard_spike_prob: float = 0.01       # per-step chance of a hazard spike
    hazard_spike_lo: float = 0.2
    hazard_spike_hi: float = 0.5

    # --- noise / reward ---
    state_noise: float = 0.005            # additive internal noise per step
    death_penalty: float = 2.0            # terminal -survival_violation (flat)
    # Additional per-remaining-step forfeit: dying at step t costs
    # death_penalty + death_forfeit_rate * (episode_length - t).
    # Equivalent to an alive bonus; prevents suicide being optimal when
    # sustained error exceeds ~death_forfeit_rate.
    death_forfeit_rate: float = 0.05

    # --- OOD multipliers (1.0 = ID) ---
    energy_cost_mult: float = 1.0         # OOD-energy: 1.5

    # --- causal test hook ---
    # Optional permutation applied to the incoming action index before
    # dynamics. action_permutation[i] = which *physical* action index the
    # agent's choice i actually triggers. None = identity.
    action_permutation: Optional[tuple] = None

    def with_overrides(self, **kw) -> "EnvParams":
        return replace(self, **kw)


# ---------------------------------------------------------------------------
# Core update functions (pure)
# ---------------------------------------------------------------------------


def deviation(x: float, lo: float, hi: float) -> float:
    """Distance of x outside [lo, hi]; 0 inside."""
    if x < lo:
        return lo - x
    if x > hi:
        return x - hi
    return 0.0


def homeostatic_error(internal: np.ndarray) -> float:
    """E = sum_i wi * deviation(state_i, preferred_range_i)."""
    err = 0.0
    for i, name in enumerate(INTERNAL_NAMES):
        lo, hi = PREFERRED_RANGES[name]
        err += ERROR_WEIGHTS[name] * deviation(float(internal[i]), lo, hi)
    return err


def stable_mask(internal: np.ndarray) -> bool:
    """True if every internal variable is inside its preferred range."""
    for i, name in enumerate(INTERNAL_NAMES):
        lo, hi = PREFERRED_RANGES[name]
        if internal[i] < lo or internal[i] > hi:
            return False
    return True


def check_death(internal: np.ndarray) -> Optional[str]:
    """Return name of the variable that crossed a critical threshold, or None."""
    for name, (lo, hi) in CRITICAL_THRESHOLDS.items():
        v = float(internal[INTERNAL_NAMES.index(name)])
        if lo is not None and v < lo:
            return name
        if hi is not None and v > hi:
            return name
    return None


def update_external(
    rng: np.random.Generator,
    ext: np.ndarray,
    t: int,
    ambient_phase: float,
    p: EnvParams,
    action: int = IGNORE,
) -> np.ndarray:
    """One step of external world dynamics. Returns new ext vector.

    `action` matters because resource_availability and hazard_level are
    partially endogenous: REST depletes resource, EXPLORE replenishes it,
    OBSERVE reduces hazard.
    """
    e = ext.copy()
    if p.action_permutation is not None:
        action = int(p.action_permutation[action])
    if action == REST:
        e[RESOURCE] -= p.rest_resource_drain
    elif action == EXPLORE:
        e[RESOURCE] += p.explore_resource_gain
    elif action == OBSERVE:
        e[HAZARD] -= p.observe_hazard_reduction
    # ambient temperature: slow sinusoid + noise
    e[AMBIENT] = (
        p.ambient_mean
        + p.ambient_amp * np.sin(2.0 * np.pi * t / p.ambient_period + ambient_phase)
        + rng.normal(0.0, p.ambient_noise)
    )
    # OU mean-reverting processes
    e[ENV_NOV] += p.ou_relax * (p.novelty_mean - e[ENV_NOV]) + rng.normal(0.0, p.ou_noise)
    e[RESOURCE] += p.resource_ou_relax * (p.resource_mean - e[RESOURCE]) + rng.normal(0.0, p.ou_noise)
    e[OBS_QUAL] += p.ou_relax * (p.obs_quality_mean - e[OBS_QUAL]) + rng.normal(0.0, p.ou_noise)
    e[HAZARD] += p.hazard_ou_relax * (p.hazard_mean - e[HAZARD]) + rng.normal(
        0.0, p.hazard_ou_noise
    )
    # occasional hazard spikes
    if rng.random() < p.hazard_spike_prob:
        e[HAZARD] += rng.uniform(p.hazard_spike_lo, p.hazard_spike_hi)
    np.clip(e, 0.0, 1.0, out=e)
    return e


def update_internal(
    rng: np.random.Generator,
    internal: np.ndarray,
    ext: np.ndarray,
    action: int,
    p: EnvParams,
) -> np.ndarray:
    """One step of internal dynamics given action. Returns new internal vector.

    Applies passive drift, the action effect, hazard exposure and noise.
    """
    s = internal.copy()
    d = np.zeros(N_INTERNAL, dtype=np.float64)

    # --- passive drift ---
    d[ENERGY] -= p.passive_energy_cost * p.energy_cost_mult
    d[FATIGUE] += p.passive_fatigue_rate
    d[NOVELTY] += p.novelty_relax_rate * (
        p.novelty_baseline_gain * ext[ENV_NOV] - s[NOVELTY]
    )
    d[CERTAINTY] += p.certainty_decay_rate * (p.certainty_baseline - s[CERTAINTY])
    d[TEMPERATURE] += p.temp_relax_rate * (ext[AMBIENT] - s[TEMPERATURE])

    # --- action effect ---
    if p.action_permutation is not None:
        action = int(p.action_permutation[action])

    if action == IGNORE:
        d[ENERGY] -= p.ignore_energy_cost
        d[FATIGUE] += p.ignore_fatigue_rate
        d[NOVELTY] -= p.ignore_novelty_loss
        d[CERTAINTY] -= p.ignore_certainty_loss
    elif action == WAIT:
        d[ENERGY] -= p.wait_energy_cost
        d[FATIGUE] -= p.wait_fatigue_recovery
        d[TEMPERATURE] += p.wait_shelter_rate * (0.5 - s[TEMPERATURE])
    elif action == REST:
        d[ENERGY] += p.rest_energy_gain * ext[RESOURCE]
        d[FATIGUE] -= p.rest_fatigue_recovery
        d[NOVELTY] -= p.rest_novelty_loss
        d[TEMPERATURE] += p.rest_temp_coupling * (ext[AMBIENT] - s[TEMPERATURE])
    elif action == EXPLORE:
        d[ENERGY] -= p.explore_energy_cost * p.energy_cost_mult
        d[FATIGUE] += p.explore_fatigue_rate
        d[NOVELTY] += p.explore_novelty_gain * (0.5 + 0.5 * ext[ENV_NOV])
        d[CERTAINTY] += rng.uniform(
            -p.explore_certainty_spread, p.explore_certainty_spread
        )
        microclimate = ext[AMBIENT] + rng.normal(0.0, p.microclimate_std)
        d[TEMPERATURE] += p.microclimate_coupling * (microclimate - s[TEMPERATURE])
    elif action == OBSERVE:
        d[ENERGY] -= p.observe_energy_cost
        d[FATIGUE] += p.observe_fatigue_rate
        d[CERTAINTY] += p.observe_certainty_gain * ext[OBS_QUAL]
        d[NOVELTY] += p.observe_novelty_gain
    else:
        raise ValueError(f"unknown action {action}")

    s += d

    # --- hazard exposure ---
    if action == EXPLORE and rng.random() < ext[HAZARD] * p.explore_hazard_mult:
        s[ENERGY] -= rng.uniform(p.hazard_energy_loss_lo, p.hazard_energy_loss_hi)
        s[FATIGUE] += p.hazard_fatigue_gain
    if action == IGNORE and rng.random() < ext[HAZARD] * p.ignore_hazard_mult:
        s[ENERGY] -= rng.uniform(p.hazard_energy_loss_lo, p.hazard_energy_loss_hi)
        s[TEMPERATURE] += rng.uniform(-p.hazard_temp_jolt, p.hazard_temp_jolt)
    if action == WAIT and rng.random() < ext[HAZARD] * p.wait_hazard_mult:
        s[ENERGY] -= rng.uniform(p.hazard_energy_loss_lo, p.hazard_energy_loss_hi)

    s += rng.normal(0.0, p.state_noise, size=N_INTERNAL)
    np.clip(s, 0.0, 1.0, out=s)
    return s


def initial_internal(rng: np.random.Generator) -> np.ndarray:
    """Sample an initial internal state near the middle of preferred ranges."""
    s = np.zeros(N_INTERNAL, dtype=np.float64)
    for i, name in enumerate(INTERNAL_NAMES):
        lo, hi = PREFERRED_RANGES[name]
        mid = 0.5 * (lo + hi)
        half = 0.25 * (hi - lo)
        s[i] = mid + rng.uniform(-half, half)
    np.clip(s, 0.0, 1.0, out=s)
    return s


def initial_external(rng: np.random.Generator, p: EnvParams) -> np.ndarray:
    e = np.array(
        [
            p.ambient_mean,
            p.novelty_mean,
            p.resource_mean,
            p.obs_quality_mean,
            p.hazard_mean,
        ],
        dtype=np.float64,
    )
    e += rng.normal(0.0, 0.03, size=N_EXTERNAL)
    np.clip(e, 0.0, 1.0, out=e)
    return e
