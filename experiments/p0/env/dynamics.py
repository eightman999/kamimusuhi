"""P0 predictive-surprise environment dynamics.

A small synthetic multi-channel world. Four scalar channels, all in [0, 1]:

    A ("constant"):   nearly constant; tiny OU wiggle. Easy to predict.
    B ("periodic"):   sinusoid + small noise. Predictable if phase is tracked.
    C ("stochastic"): OU process with occasional large jumps. Chronically
                      unpredictable, but NOT a pure distractor: each C jump
                      opens a cd_coupling_window-step boost of the D-event
                      hazard, so C is a legitimate leading indicator for D.
                      It still punishes naive surprise-maximizing attention.
    D ("relevant"):   rare latent events. FSM QUIET -> RAMP -> EVENT -> QUIET.
                      RAMP is a subtle precursor regime (elevated mean/variance
                      but overlapping marginals); EVENT is a *single-step*
                      window in which observing D yields reward. Missed events
                      cost.

v2 (post-review) design note: the detection window is ONE step. A pure
periodic clock polling D every k steps can only land on a 1-step event with
probability ~1/k, so the clock frontier is (det ~1/k, cost 0.02/k) --
efficiency ~50 at every poll rate. Periodic polling is structurally
dominated. HOWEVER (round-3 review correction): beating the clock does NOT
require calibrated prediction -- the env admits a "belief-reactive
frontier": (a) RAMP's elevated mean makes the raw observed value v_D
itself a usable trigger, and (b) C jumps are a leading indicator for the
hazard boost. A policy can exploit either with only belief features
(v, s, pv) -- see vtrigger / cwatch baselines. No current env feature makes
the learned predictor's e/d features load-bearing in principle.

The module is pure numpy + dataclasses so dynamics can be unit-tested and
replayed deterministically without torch.
"""

from __future__ import annotations

from dataclasses import dataclass, replace
from typing import Optional, Tuple

import numpy as np

# ---------------------------------------------------------------------------
# Indices / names
# ---------------------------------------------------------------------------

CHANNEL_NAMES = ["A", "B", "C", "D"]
ACTION_NAMES = ["IGNORE", "OBSERVE_A", "OBSERVE_B", "OBSERVE_C", "OBSERVE_D", "OBSERVE_ALL"]

N_CHANNELS = len(CHANNEL_NAMES)
N_ACTIONS = len(ACTION_NAMES)

CH_A, CH_B, CH_C, CH_D = range(N_CHANNELS)
IGNORE, OBS_A, OBS_B, OBS_C, OBS_D, OBS_ALL = range(N_ACTIONS)

# OBSERVE_* action -> channel index; OBS_ALL -> all channels; IGNORE -> none.
ACTION_CHANNELS = {
    IGNORE: (),
    OBS_A: (CH_A,),
    OBS_B: (CH_B,),
    OBS_C: (CH_C,),
    OBS_D: (CH_D,),
    OBS_ALL: (CH_A, CH_B, CH_C, CH_D),
}

# D latent phases
D_QUIET, D_RAMP, D_EVENT = 0, 1, 2

# Belief bookkeeping: staleness is capped (belief is "unknown" beyond this).
STALENESS_CAP = 32.0


# ---------------------------------------------------------------------------
# Parameters
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class EnvParams:
    """All tunable environment parameters.

    OOD configs override a small subset (see configs/ood_*.yaml).
    """

    episode_length: int = 512

    # --- channel A: nearly constant ---
    a_mean: float = 0.5
    a_relax: float = 0.02
    a_noise: float = 0.008

    # --- channel B: periodic ---
    b_mean: float = 0.5
    b_amp: float = 0.30
    b_period: int = 64
    b_noise: float = 0.01

    # --- channel C: stochastic + jumps ---
    c_mean: float = 0.5
    c_relax: float = 0.08
    c_noise: float = 0.04
    c_jump_prob: float = 0.02          # per-step chance of a jump
    c_jump_lo: float = 0.2             # jump magnitude range
    c_jump_hi: float = 0.5

    # --- channel D: rare relevant events ---
    d_base_mean: float = 0.15
    d_base_noise: float = 0.07         # noisy baseline so ramp is subtle
    d_ramp_mean: float = 0.32          # modest elevation; marginals overlap
    d_ramp_noise: float = 0.11         # elevated variance: "unstable" regime
    d_event_mean: float = 0.85
    d_event_noise: float = 0.05
    d_ramp_steps: int = 5              # precursor length (timing is learnable)
    d_event_steps: int = 1             # detection window: ONE step (v2)
    d_cooldown_steps: int = 10         # quiet lockout after an event
    p_event: float = 0.008             # base per-step event hazard in QUIET
    # C->D coupling: a C jump elevates the D-event hazard for a few steps.
    cd_coupling_mult: float = 10.0     # hazard multiplier while boosted (v2: 6->10)
    cd_coupling_window: int = 8        # steps of elevated hazard after a C jump

    # --- observation ---
    obs_noise: float = 0.02            # noise on observed values
    cost_observe: float = 0.02         # per single-channel OBSERVE_*
    cost_observe_all: float = 0.06     # OBSERVE_ALL (3x single, < 4x)
    cost_ignore: float = 0.0

    # --- reward ---
    detect_bonus: float = 1.0          # once per event, on first in-window obs
    miss_penalty: float = 1.0          # once per event that expires unseen

    # --- OOD multipliers (1.0 = ID) ---
    event_freq_mult: float = 1.0       # OOD-event-freq: 2.0 / 0.5
    noise_mult: float = 1.0            # OOD-noise: 2.0
    cost_mult: float = 1.0             # OOD-cost: 2.0

    def with_overrides(self, **kw) -> "EnvParams":
        return replace(self, **kw)


# ---------------------------------------------------------------------------
# Latent D state (kept in a small struct updated by the env)
# ---------------------------------------------------------------------------


@dataclass
class DState:
    phase: int = D_QUIET
    phase_t: int = 0            # steps elapsed in current phase
    cooldown: int = 0           # steps of trigger lockout remaining
    boost: int = 0              # steps of C->D elevated hazard remaining
    detected: bool = False      # current event already detected
    # per-episode counters (metrics)
    n_events: int = 0
    n_detected: int = 0
    n_missed: int = 0


def c_jumped(x_prev: float, x_now: float, p: EnvParams) -> bool:
    """A 'C jump' is a large single-step move (the jump process adds >= lo)."""
    return abs(x_now - x_prev) >= p.c_jump_lo * 0.9


def update_channels(
    rng: np.random.Generator,
    x: np.ndarray,
    t: int,
    b_phase: float,
    d: DState,
    p: EnvParams,
    d_observed: bool = False,
) -> Tuple[np.ndarray, bool, bool]:
    """Advance all four channels one step.

    `d_observed` is True when this step's action observes channel D; the FSM
    uses it for detection bookkeeping (detecting = observing D while EVENT).

    Returns (x_new, detected_now, event_missed_now).
    Mutates `d` (the latent D FSM) in place.
    """
    x = x.copy()
    nm = p.noise_mult

    # --- A: tiny OU wiggle around its mean ---
    x[CH_A] += p.a_relax * (p.a_mean - x[CH_A]) + rng.normal(0.0, p.a_noise * nm)

    # --- B: sinusoid + noise ---
    x[CH_B] = (
        p.b_mean
        + p.b_amp * np.sin(2.0 * np.pi * t / p.b_period + b_phase)
        + rng.normal(0.0, p.b_noise * nm)
    )

    # --- C: OU + occasional jumps ---
    prev_c = x[CH_C]
    x[CH_C] += p.c_relax * (p.c_mean - x[CH_C]) + rng.normal(0.0, p.c_noise * nm)
    if rng.random() < p.c_jump_prob:
        x[CH_C] += rng.uniform(p.c_jump_lo, p.c_jump_hi) * rng.choice([-1.0, 1.0])
    if c_jumped(prev_c, x[CH_C], p):
        d.boost = p.cd_coupling_window

    # --- D: latent FSM ---
    detected_now = False
    event_missed = False
    d.phase_t += 1
    if d.cooldown > 0:
        d.cooldown -= 1
    if d.boost > 0:
        d.boost -= 1

    if d.phase == D_QUIET:
        x[CH_D] = p.d_base_mean + rng.normal(0.0, p.d_base_noise * nm)
        if d.cooldown == 0:
            hazard = p.p_event * p.event_freq_mult
            if d.boost > 0:
                hazard *= p.cd_coupling_mult
            if rng.random() < hazard:
                d.phase = D_RAMP
                d.phase_t = 0
    elif d.phase == D_RAMP:
        x[CH_D] = p.d_ramp_mean + rng.normal(0.0, p.d_ramp_noise * nm)
        if d.phase_t >= p.d_ramp_steps:
            d.phase = D_EVENT
            d.phase_t = 0
            d.detected = False
            d.n_events += 1
    elif d.phase == D_EVENT:
        x[CH_D] = p.d_event_mean + rng.normal(0.0, p.d_event_noise * nm)
        if d_observed and not d.detected:
            d.detected = True
            d.n_detected += 1
            detected_now = True
        if d.phase_t >= p.d_event_steps:
            d.phase = D_QUIET
            d.phase_t = 0
            d.cooldown = p.d_cooldown_steps
            if not d.detected:
                d.n_missed += 1
                event_missed = True
    np.clip(x, 0.0, 1.0, out=x)
    return x, detected_now, event_missed


def action_cost(action: int, p: EnvParams) -> float:
    if action == IGNORE:
        return p.cost_ignore
    if action == OBS_ALL:
        return p.cost_observe_all * p.cost_mult
    return p.cost_observe * p.cost_mult


def initial_channels(rng: np.random.Generator, p: EnvParams) -> np.ndarray:
    x = np.array(
        [p.a_mean, p.b_mean, p.c_mean, p.d_base_mean], dtype=np.float64
    )
    x += rng.normal(0.0, 0.02, size=N_CHANNELS)
    np.clip(x, 0.0, 1.0, out=x)
    return x


def channel_priors(p: EnvParams) -> np.ndarray:
    """Prior mean per channel -- used as the 'never observed' belief value.

    Baseline means are environment statistics, not future information.
    """
    return np.array([p.a_mean, p.b_mean, p.c_mean, p.d_base_mean], dtype=np.float64)
