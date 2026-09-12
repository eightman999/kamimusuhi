"""P0 attention baselines.

All implement reset() / act(obs_24) -> action.

    random        uniform over all 6 actions
    never         always IGNORE (zero cost, zero detection floor)
    roundrobin    cycles OBSERVE_A..D, never IGNORE/ALL
    always        always OBSERVE_ALL (max coverage, max cost)
    fixed         hand-coded staleness-clock: OBSERVE_D every `poll` steps,
                  plus a surprise trigger on D (the sane reference policy)
    clockK        PURE step-grid clock: OBSERVE_D every K steps, no feedback
                  (the non-surprise frontier the learned agent must beat)
    pe_heuristic  hand-coded prediction-error heuristic: observe the
                  argmax-surprise channel every step (never ignores)
    pe_thresh     argmax-surprise channel iff surprise > theta, else IGNORE
                  (fair version of pe_heuristic: allowed to skip steps)
    trigger       prediction-informed burst policy: sparse D polling, and
                  on a D surprise spike dense-poll D until the spike decays
                  (hand-coded demo that prediction beats the clock frontier)
    vtrigger      BELIEF-reactive variant of trigger: same burst machinery
                  but driven by the raw last-observed value v_D (RAMP reads
                  ~0.32 vs QUIET ~0.15). No predictor at all -- shows that
                  beating the clock frontier does not require prediction.
    vtrigger22    vtrigger with tau_hi=0.22 (higher-detection point)
    cwatch        C->D coupling policy: poll C, burst D on detected C jumps
                  (C is a leading indicator, not a pure distractor)
"""

from __future__ import annotations

import numpy as np

from ..env import dynamics as dyn
from ..env.attention_env import AttentionEnv
from .base import Agent

F = AttentionEnv.FEATS_PER_CHANNEL


def _ch(obs: np.ndarray, ch: int, off: int) -> float:
    return float(obs[ch * F + off])


class RandomAgent(Agent):
    name = "random"

    def __init__(self, seed: int = 0):
        self.rng = np.random.default_rng(seed)

    def act(self, obs: np.ndarray) -> int:
        return int(self.rng.integers(dyn.N_ACTIONS))


class NeverAgent(Agent):
    name = "never"

    def act(self, obs: np.ndarray) -> int:
        return dyn.IGNORE


class RoundRobinAgent(Agent):
    name = "roundrobin"

    def __init__(self):
        self._i = 0

    def reset(self):
        self._i = 0

    def act(self, obs: np.ndarray) -> int:
        a = dyn.OBS_A + (self._i % dyn.N_CHANNELS)
        self._i += 1
        return int(a)


class AlwaysAgent(Agent):
    name = "always"

    def act(self, obs: np.ndarray) -> int:
        return dyn.OBS_ALL


class FixedAgent(Agent):
    """Poll D on a staleness clock; observe D early if it looks surprising.

    Uses only staleness (s_D) and the surprise features (e_D/d_D). On the v2
    single-step-event env a poll-2 clock lands on the event window only ~half
    the time; the e_D surprise trigger exists to catch ramps the clock would
    otherwise miss. It is the hand-coded analogue of learned attention, not a
    detection-ceiling policy (see clock1 for that).
    """

    name = "fixed"

    def __init__(self, poll: int = 2, surprise_thresh: float = 0.5):
        self.poll = poll
        self.surprise_thresh = surprise_thresh

    def act(self, obs: np.ndarray) -> int:
        s_d = _ch(obs, dyn.CH_D, AttentionEnv.S) * dyn.STALENESS_CAP
        e_d = _ch(obs, dyn.CH_D, AttentionEnv.E)
        if e_d > self.surprise_thresh and s_d >= 1:
            return dyn.OBS_D
        if s_d >= self.poll:
            return dyn.OBS_D
        return dyn.IGNORE


class PEHeuristicAgent(Agent):
    """Observe the argmax-surprise channel each step (hand-coded).

    surprise_i = e_i + d_i (recent measured error + anticipated error).
    Expected to fixate C (chronically unpredictable) -- the control showing
    raw surprise maximization is not enough; relevance must be learned.
    """

    name = "pe_heuristic"

    def __init__(self, min_steps: int = 1):
        self.min_steps = min_steps  # ignore channels seen this recently

    def act(self, obs: np.ndarray) -> int:
        best, best_s = -1.0, -1
        for ch in range(dyn.N_CHANNELS):
            s = _ch(obs, ch, AttentionEnv.S) * dyn.STALENESS_CAP
            if s < self.min_steps:
                continue
            surp = _ch(obs, ch, AttentionEnv.E) + _ch(obs, ch, AttentionEnv.D)
            if surp > best:
                best, best_s = surp, ch
        if best_s < 0:
            return dyn.IGNORE
        return dyn.OBS_A + best_s


class ClockAgent(Agent):
    """Pure periodic clock: OBSERVE_D every `poll` env steps, no feedback.

    This is the strongest NON-predictive policy class: it never reacts to
    surprise, beliefs, or detections. With the single-step event window a
    poll-k clock lands on an event with probability ~1/k, so the clock
    frontier is (det ~1/k, cost 0.02/k) -- all points have efficiency ~50.
    """

    def __init__(self, poll: int = 2):
        self.poll = poll
        self._t = 0

    def reset(self):
        self._t = 0

    @property
    def name(self):
        return f"clock{self.poll}"

    def act(self, obs: np.ndarray) -> int:
        a = dyn.OBS_D if (self._t % self.poll == 0) else dyn.IGNORE
        self._t += 1
        return int(a)


class PEThreshAgent(Agent):
    """Threshold surprise heuristic: observe the argmax-surprise channel iff
    its surprise exceeds theta, else IGNORE.

    Unlike pe_heuristic it is allowed to skip steps, so it is a fair
    efficiency comparison rather than a strawman that always pays cost.
    """

    name = "pe_thresh"

    def __init__(self, theta: float = 0.35, min_steps: int = 1):
        self.theta = theta
        self.min_steps = min_steps

    def act(self, obs: np.ndarray) -> int:
        best, best_s = -1.0, -1
        for ch in range(dyn.N_CHANNELS):
            s = _ch(obs, ch, AttentionEnv.S) * dyn.STALENESS_CAP
            if s < self.min_steps:
                continue
            surp = _ch(obs, ch, AttentionEnv.E) + _ch(obs, ch, AttentionEnv.D)
            if surp > best:
                best, best_s = surp, ch
        if best_s < 0 or best <= self.theta:
            return dyn.IGNORE
        return dyn.OBS_A + best_s


class TriggerAgent(Agent):
    """Prediction-informed burst policy (hand-coded upper-reference).

    Sparse background polling of D (every `bg_poll` steps via staleness);
    when the measured surprise e_D exceeds `theta_hi`, enter a burst: poll D
    every step until e_D stays below `theta_lo` for `patience` consecutive
    steps (or `burst_max` reached). On the 1-step-event env this catches the
    event window after a RAMP read -- something no pure clock can do cheaply.
    """

    name = "trigger"

    def __init__(self, bg_poll: int = 6, theta_hi: float = 0.45,
                 theta_lo: float = 0.08, patience: int = 2,
                 burst_max: int = 8):
        self.bg_poll = bg_poll
        self.theta_hi = theta_hi
        self.theta_lo = theta_lo
        self.patience = patience
        self.burst_max = burst_max
        self._burst = 0
        self._calm = 0

    def reset(self):
        self._burst = 0
        self._calm = 0

    def act(self, obs: np.ndarray) -> int:
        s_d = _ch(obs, dyn.CH_D, AttentionEnv.S) * dyn.STALENESS_CAP
        e_d = _ch(obs, dyn.CH_D, AttentionEnv.E)
        if self._burst > 0:
            self._burst -= 1
            # e_d reflects the read taken last step; keep watching while the
            # channel is still surprising, leave after `patience` calm reads.
            if e_d <= self.theta_lo:
                self._calm += 1
            else:
                self._calm = 0
            if self._calm >= self.patience:
                self._burst = 0
                self._calm = 0
                return dyn.IGNORE
            return dyn.OBS_D
        if e_d >= self.theta_hi:
            self._burst = self.burst_max
            self._calm = 0
            return dyn.OBS_D
        if s_d >= self.bg_poll:
            return dyn.OBS_D
        return dyn.IGNORE


class VTriggerAgent(Agent):
    """Belief-reactive burst policy -- NO predictor inputs at all.

    Identical machinery to TriggerAgent but the trigger is the raw
    last-observed value v_D (a belief feature) rather than predictor surprise
    e_D: RAMP reads are ~0.32 vs QUIET ~0.15, so a plain value threshold
    detects ramps. It demonstrates the "belief-reactive frontier": on this
    env, beating the pure-clock frontier does NOT require prediction -- the
    e_D feature degenerates to a lagged deviation detector and the raw value
    carries the same information earlier.

    `s_d == 0` gates the trigger so only fresh reads count (a stale high v_D
    left over from an expired burst must not retrigger it).
    """

    def __init__(self, bg_poll: int = 6, tau_hi: float = 0.30,
                 tau_lo: float = 0.20, patience: int = 2,
                 burst_max: int = 8):
        self.bg_poll = bg_poll
        self.tau_hi = tau_hi
        self.tau_lo = tau_lo
        self.patience = patience
        self.burst_max = burst_max
        self._burst = 0
        self._calm = 0

    @property
    def name(self):
        return "vtrigger" if self.tau_hi == 0.30 else f"vtrigger{int(self.tau_hi * 100)}"

    def reset(self):
        self._burst = 0
        self._calm = 0

    def act(self, obs: np.ndarray) -> int:
        s_d = _ch(obs, dyn.CH_D, AttentionEnv.S) * dyn.STALENESS_CAP
        v_d = _ch(obs, dyn.CH_D, AttentionEnv.V)
        if self._burst > 0:
            self._burst -= 1
            # v_d reflects the read taken last step; keep watching while the
            # channel stays elevated, leave after `patience` calm reads.
            if v_d <= self.tau_lo:
                self._calm += 1
            else:
                self._calm = 0
            if self._calm >= self.patience:
                self._burst = 0
                self._calm = 0
                return dyn.IGNORE
            return dyn.OBS_D
        if s_d == 0 and v_d >= self.tau_hi:
            self._burst = self.burst_max
            self._calm = 0
            return dyn.OBS_D
        if s_d >= self.bg_poll:
            return dyn.OBS_D
        return dyn.IGNORE


class CWatchAgent(Agent):
    """C->D coupling policy -- NO predictor inputs.

    Polls C every `c_poll` steps. When a fresh C read shows a jump
    (|v_C - pv_C| >= jump_min, i.e. a real C jump rather than OU drift), it
    burst-polls D for `cover` steps: the C->D boost lasts
    cd_coupling_window steps and a triggered ramp+event needs
    d_ramp_steps + d_event_steps more, so cover = window + ramp + event.

    Because ~40% of D ramps start outside a C-jump boost (base hazard), it
    also keeps a light D background poll with a raw-value trigger
    (v_D >= tau_v -> burst until calm), which catches non-coupled events.

    This agent is the proof that channel C is a legitimate leading indicator
    for D events -- not a pure distractor -- and that coupling structure is
    exploitable without any prediction machinery.
    """

    name = "cwatch"

    def __init__(self, c_poll: int = 3, jump_min: float = 0.18,
                 cover: int = 14, d_bg_poll: int = 6,
                 tau_v: float = 0.26, patience: int = 2):
        self.c_poll = c_poll
        self.jump_min = jump_min
        self.cover = cover
        self.d_bg_poll = d_bg_poll
        self.tau_v = tau_v
        self.patience = patience
        self._cover = 0
        self._calm = 0

    def reset(self):
        self._cover = 0
        self._calm = 0

    def act(self, obs: np.ndarray) -> int:
        s_c = _ch(obs, dyn.CH_C, AttentionEnv.S) * dyn.STALENESS_CAP
        s_d = _ch(obs, dyn.CH_D, AttentionEnv.S) * dyn.STALENESS_CAP
        v_c = _ch(obs, dyn.CH_C, AttentionEnv.V)
        pv_c = _ch(obs, dyn.CH_C, AttentionEnv.PV)
        v_d = _ch(obs, dyn.CH_D, AttentionEnv.V)

        # a fresh C read that moved a lot since the previous read = C jump
        c_jumped = s_c == 0 and abs(v_c - pv_c) >= self.jump_min
        # a fresh D read that is elevated = ramp signature (vtrigger logic)
        d_elevated = s_d == 0 and v_d >= self.tau_v

        if self._cover > 0:
            # stay on D while covered; re-arm on a fresh jump / elevated read
            if c_jumped:
                self._cover = self.cover
            elif not d_elevated:
                self._calm += 1
                if self._calm >= self.patience and self._cover <= self.cover - 6:
                    # event passed: D calm for `patience` reads and we're past
                    # the post-jump hazard window -> release early
                    self._cover = 0
                    self._calm = 0
                    return dyn.IGNORE
            else:
                self._calm = 0
            self._cover -= 1
            return dyn.OBS_D

        if c_jumped or d_elevated:
            self._cover = self.cover
            self._calm = 0
            return dyn.OBS_D
        if s_d >= self.d_bg_poll:
            return dyn.OBS_D
        if s_c >= self.c_poll:
            return dyn.OBS_C
        return dyn.IGNORE


def make_baseline(kind: str, seed: int = 0) -> Agent:
    if kind == "random":
        return RandomAgent(seed=seed)
    if kind == "never":
        return NeverAgent()
    if kind == "roundrobin":
        return RoundRobinAgent()
    if kind == "always":
        return AlwaysAgent()
    if kind == "fixed":
        return FixedAgent()
    if kind == "pe_heuristic":
        return PEHeuristicAgent()
    if kind == "pe_thresh":
        return PEThreshAgent()
    if kind == "trigger":
        return TriggerAgent()
    if kind == "vtrigger":
        return VTriggerAgent()
    if kind == "vtrigger22":
        return VTriggerAgent(tau_hi=0.22)
    if kind == "cwatch":
        return CWatchAgent()
    if kind.startswith("clock"):
        return ClockAgent(poll=int(kind[5:]))
    raise ValueError(f"unknown baseline {kind}")
