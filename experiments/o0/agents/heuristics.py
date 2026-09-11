"""Hand-coded predictor baselines for O0.

Each heuristic keeps its own internal tracker state and answers the same
three questions as the learned models at every step:

    x_hat, v_hat, exist_prob, same_prob = h.predict(obs_row)

Positions/velocities are in the same normalized units as the labels
(x / world_len, v * VEL_SCALE / world_len).  `reset_memory()` clears the
tracker (the causal "hidden reset" analogue); `reset()` restarts the
whole tracker for a new episode.

Baselines:
    prior    -- geometry prior: predict occluder midpoint while hidden
    lastobs  -- freeze the last visible position
    constvel -- constant-velocity extrapolation from the last observation
    oracle   -- same but with the true dynamics (drag + acceleration
                estimate and the boundary-absorption rule); reference
                ceiling, not a competitor
"""

from __future__ import annotations

import numpy as np

from ..env import dynamics as dyn


class BaseHeuristic:
    name = "base"
    app_tol = 0.15  # |app - remembered app| tolerance for identity

    def __init__(self, params: dyn.EnvParams):
        self.p = params
        self.reset()

    def reset(self):
        self.seen = False
        self.imposter = False
        self.reset_memory()

    def reset_memory(self):
        """Forget the tracked object state (hidden-reset analogue)."""
        self.last_x = 0.0      # normalized
        self.last_v = 0.0      # normalized
        self.last_app = 0.0
        self.dt = 0            # steps since last seen
        self.seen = False
        self.imposter = False

    # ------------------------------------------------------------------
    def _current_guess(self, obs):
        """Position guess while the target is invisible."""
        raise NotImplementedError

    def _swap_detected(self, obs):
        """Reappearance-step identity check (memory needed -> dt > 0)."""
        return abs(float(obs[dyn.T_APP]) - self.last_app) >= self.app_tol

    def predict(self, obs):
        vis = obs[dyn.T_VIS] > 0.5
        if vis:
            x_hat = float(obs[dyn.T_X])
            v_hat = float(obs[dyn.T_V])
            if self.dt > 0 and self.seen and self._swap_detected(obs):
                self.imposter = True
            same = 0.0 if self.imposter else 1.0
            self.last_x, self.last_v = x_hat, v_hat
            self.last_app = float(obs[dyn.T_APP])
            self.dt = 0
            self.seen = True
            return x_hat, v_hat, 1.0, same
        # invisible step
        self.dt += 1
        x_hat, v_hat = self._current_guess(obs)
        exist = self._exist(obs)
        return x_hat, v_hat, exist, 0.0 if self.imposter else 1.0

    def _exist(self, obs):
        if not self.seen:
            return 0.0
        return 1.0


class PriorHeuristic(BaseHeuristic):
    """Occluder-midpoint guess while hidden; needs no memory."""

    name = "prior"

    def _current_guess(self, obs):
        if not self.seen:
            return 0.5, 0.0
        mid = 0.5 * (obs[dyn.OCC_LO_IDX] + obs[dyn.OCC_HI_IDX])
        return float(mid), 0.0


class LastObsHeuristic(BaseHeuristic):
    name = "lastobs"

    def _current_guess(self, obs):
        return self.last_x, 0.0


class ConstVelHeuristic(BaseHeuristic):
    """x_hat = last_x + last_v * dt (no drag/accel model)."""

    name = "constvel"

    def _extrapolated_x(self):
        return self.last_x + self.last_v * self.dt / dyn.VEL_SCALE

    def _current_guess(self, obs):
        return float(np.clip(self._extrapolated_x(), -0.5, 1.5)), 0.0

    def _exist(self, obs):
        if not self.seen:
            return 0.0
        x = self._extrapolated_x()
        return 1.0 if 0.0 <= x <= 1.0 else 0.0


class OracleTracker(BaseHeuristic):
    """Tracking with the *true* motion model (reference ceiling).

    Integrates drag plus a finite-difference acceleration estimate; knows
    the boundary-absorption rule; identity check uses both appearance
    and predicted position.
    """

    name = "oracle"

    def reset_memory(self):
        super().reset_memory()
        self.acc_est = 0.0
        self._prev_v = None
        self._prev_dt = 0

    def _integrate(self):
        x, v = self.last_x, self.last_v
        for _ in range(self.dt):
            v = (v + self.acc_est) * (1.0 - self.p.drag)
            x += v / dyn.VEL_SCALE
        return x

    def predict(self, obs):
        vis = obs[dyn.T_VIS] > 0.5
        if vis and self._prev_v is not None and self.dt == 0 and self.seen:
            # consecutive visible steps: update acceleration estimate
            self.acc_est = 0.9 * self.acc_est + 0.1 * (
                float(obs[dyn.T_V]) - self._prev_v
            )
        if vis:
            self._prev_v = float(obs[dyn.T_V])
        return super().predict(obs)

    def _current_guess(self, obs):
        return float(np.clip(self._integrate(), -0.5, 1.5)), 0.0

    def _exist(self, obs):
        if not self.seen:
            return 0.0
        return 1.0 if 0.0 <= self._integrate() <= 1.0 else 0.0

    def _swap_detected(self, obs):
        app_bad = abs(float(obs[dyn.T_APP]) - self.last_app) >= self.app_tol
        pos_bad = abs(float(obs[dyn.T_X]) - self._integrate()) >= 0.10
        return app_bad or pos_bad


HEURISTICS = {
    "prior": PriorHeuristic,
    "lastobs": LastObsHeuristic,
    "constvel": ConstVelHeuristic,
    "oracle": OracleTracker,
}


def make_heuristic(name: str, params: dyn.EnvParams) -> BaseHeuristic:
    if name not in HEURISTICS:
        raise ValueError(f"unknown heuristic {name}; choices {sorted(HEURISTICS)}")
    return HEURISTICS[name](params)
