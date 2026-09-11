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
    corridor -- constvel clipped to the occluder interval while hidden
                (strong honest baseline: exploits the observable bounds)
    openloop -- constvel with the true dynamics (drag + acceleration
                estimate and the boundary-absorption rule).  OPEN-LOOP:
                it integrates without correction while hidden and
                overshoots the boundary -- NOT a Bayes-optimal ceiling.
    kalman   -- linear Kalman tracker over [x, v, a] with the true
                transition (drag, constant per-episode accel, process
                noise) and the true observation noise; fuses both the
                position and velocity channels.  Closest available
                approximation to an optimal filter on this world.
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

    def perturb_memory(self, rng, sigma):
        """Hidden-noise analogue: corrupt the tracked state."""
        self.last_x += float(rng.normal(0.0, sigma))
        self.last_v += float(rng.normal(0.0, sigma))

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


class CorridorConstVelHeuristic(ConstVelHeuristic):
    """constvel clipped to the occluder bounds while hidden.

    The occluder interval is always observable, so while the target is
    inside, its position is guaranteed to lie in [occ_lo, occ_hi].
    Clipping removes all corridor-overshoot error (in particular the
    unbounded extrapolation on long/gone bouts) at the price of being
    stuck at the far edge once the object exits -- the honest strong
    baseline the first review demanded.
    """

    name = "corridor"

    def _current_guess(self, obs):
        x = self._extrapolated_x()
        lo = float(obs[dyn.OCC_LO_IDX])
        hi = float(obs[dyn.OCC_HI_IDX])
        return float(np.clip(x, lo, hi)), 0.0


class OpenLoopTracker(BaseHeuristic):
    """Tracking with the *true* motion model, open loop while hidden.

    Integrates drag plus a finite-difference acceleration estimate; knows
    the boundary-absorption rule; identity check uses both appearance
    and predicted position.  It is a reference point, not an optimal
    filter: while hidden it integrates its estimate without any
    correction, so process noise and the accel mis-estimate accumulate
    and it overshoots the boundary on long bouts.
    """

    name = "openloop"

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


class KalmanTracker(BaseHeuristic):
    """Linear Kalman filter over [x, v, a] in normalized units.

    Transition matches the true world dynamics:
        v' = (1 - drag) * (v + a)      [per-step, constant accel]
        x' = x + v' / VEL_SCALE
    Process noise on v (the env's ``v_noise``), small diffusion on a so
    the accel estimate can adapt.  While the target channel is visible,
    both position (``T_X``) and velocity (``T_V``) observations are fused
    with the true observation-noise variance.  While hidden, the filter
    predicts open-loop with correctly growing covariance -- the
    information-theoretically honest way to propagate uncertainty.
    Existence: predicted beyond the boundary -> absorbed.
    """

    name = "kalman"

    def __init__(self, params: dyn.EnvParams):
        super().__init__(params)
        d = self.p.drag
        vs = dyn.VEL_SCALE
        # state transition in normalized units (x/L, v*vs/L, a*vs/L)
        self.F = np.array(
            [[1.0, (1.0 - d) / vs, (1.0 - d) / vs],
             [0.0, (1.0 - d), (1.0 - d)],
             [0.0, 0.0, 1.0]]
        )
        qv = (self.p.v_noise * vs / self.p.world_len) ** 2  # v process noise
        qa = (self.p.accel_std * vs / self.p.world_len) ** 2
        self.Q = np.diag([1e-8, qv, qa * 1e-2])
        self.H = np.array([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]])
        rn = self.p.obs_noise ** 2
        self.R = np.diag([rn, rn])
        self._I3 = np.eye(3)
        self.reset_memory()

    def reset_memory(self):
        super().reset_memory()
        self.m = np.zeros(3)
        # broad-but-sane prior; instantly tightened by first observation
        self.P = np.diag([0.25, 0.05, 1e-3])
        self.acc_est = 0.0
        self._prev_v = None

    def perturb_memory(self, rng, sigma):
        # corrupt the filter mean itself (the analogue of adding noise to
        # a recurrent hidden state)
        self.m = self.m + rng.normal(0.0, sigma, size=3)

    def _predict_cov(self):
        self.m = self.F @ self.m
        self.P = self.F @ self.P @ self.F.T + self.Q

    def predict(self, obs):
        vis = obs[dyn.T_VIS] > 0.5
        if vis:
            if self.seen:
                # predict then fuse the (x, v) measurement
                self._predict_cov()
                z = np.array([float(obs[dyn.T_X]), float(obs[dyn.T_V])])
                S = self.H @ self.P @ self.H.T + self.R
                K = self.P @ self.H.T @ np.linalg.inv(S)
                self.m = self.m + K @ (z - self.H @ self.m)
                self.P = (self._I3 - K @ self.H) @ self.P
            else:
                # first sighting: jump the state to the observation
                self.m = np.array(
                    [float(obs[dyn.T_X]), float(obs[dyn.T_V]), 0.0])
                self.P = np.diag([self.R[0, 0], self.R[1, 1], 1e-3])
            if self.dt > 0 and self.seen and self._swap_detected(obs):
                self.imposter = True
            same = 0.0 if self.imposter else 1.0
            self.last_x, self.last_v = self.m[0], self.m[1]
            self.last_app = float(obs[dyn.T_APP])
            self.dt = 0
            self.seen = True
            return self.m[0], self.m[1], 1.0, same
        # hidden step: open-loop predict only
        self.dt += 1
        if self.seen:
            self._predict_cov()
        x_hat = float(np.clip(self.m[0], -0.5, 1.5))
        exist = 1.0 if (self.seen and 0.0 <= self.m[0] <= 1.0) else 0.0
        return x_hat, self.m[1], exist, 0.0 if self.imposter else 1.0

    def _current_guess(self, obs):  # unused: predict() is overridden
        raise NotImplementedError


HEURISTICS = {
    "prior": PriorHeuristic,
    "lastobs": LastObsHeuristic,
    "constvel": ConstVelHeuristic,
    "corridor": CorridorConstVelHeuristic,
    "openloop": OpenLoopTracker,
    "kalman": KalmanTracker,
}


def make_heuristic(name: str, params: dyn.EnvParams) -> BaseHeuristic:
    if name not in HEURISTICS:
        raise ValueError(f"unknown heuristic {name}; choices {sorted(HEURISTICS)}")
    return HEURISTICS[name](params)
