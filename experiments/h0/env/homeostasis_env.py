"""H0 homeostasis environment.

Minimal Gym-like API (no gym dependency):

    env = HomeostasisEnv(params, seed=0)
    obs = env.reset()
    obs, reward, done, info = env.step(action)

Observation = concat(internal[5], external[5])  (float64, shape (10,))
Reward      = -homeostatic_error  (+ terminal -death_penalty on death)
"""

from __future__ import annotations

from typing import Optional

import numpy as np

from . import dynamics as dyn


class HomeostasisEnv:
    def __init__(self, params: Optional[dyn.EnvParams] = None, seed: int = 0):
        self.params = params or dyn.EnvParams()
        self.seed_value = seed
        self.rng = np.random.default_rng(seed)
        self.t = 0
        self.internal = np.zeros(dyn.N_INTERNAL)
        self.external = np.zeros(dyn.N_EXTERNAL)
        self.ambient_phase = 0.0
        self.done = True
        self.death_cause: Optional[str] = None
        # per-episode trace (kept small: only what analysis needs)
        self.trace = {"internal": [], "external": [], "action": [], "reward": []}

    # ------------------------------------------------------------------
    def reset(self) -> np.ndarray:
        self.rng = np.random.default_rng(self.seed_value)
        self.t = 0
        self.internal = dyn.initial_internal(self.rng)
        self.external = dyn.initial_external(self.rng, self.params)
        self.ambient_phase = float(self.rng.uniform(0.0, 2.0 * np.pi))
        self.done = False
        self.death_cause = None
        self.trace = {"internal": [], "external": [], "action": [], "reward": []}
        return self._obs()

    def reseed(self, seed: int) -> np.ndarray:
        """Reset with a new seed (used between episodes in rollouts)."""
        self.seed_value = seed
        return self.reset()

    # ------------------------------------------------------------------
    def _obs(self) -> np.ndarray:
        return np.concatenate([self.internal, self.external]).astype(np.float64)

    def step(self, action: int):
        if self.done:
            raise RuntimeError("step() called on a finished episode; call reset()")
        p = self.params
        self.trace["internal"].append(self.internal.copy())
        self.trace["external"].append(self.external.copy())
        self.trace["action"].append(int(action))

        self.internal = dyn.update_internal(
            self.rng, self.internal, self.external, int(action), p
        )
        self.external = dyn.update_external(
            self.rng, self.external, self.t, self.ambient_phase, p,
            action=int(action),
        )
        self.t += 1

        err = dyn.homeostatic_error(self.internal)
        reward = -err
        cause = dyn.check_death(self.internal)
        if cause is not None:
            self.done = True
            self.death_cause = cause
            reward -= p.death_penalty + p.death_forfeit_rate * (
                p.episode_length - self.t
            )
        elif self.t >= p.episode_length:
            self.done = True

        self.trace["reward"].append(float(reward))
        info = {
            "homeostatic_error": err,
            "stable": dyn.stable_mask(self.internal),
            "death_cause": cause,
            "t": self.t,
        }
        return self._obs(), float(reward), self.done, info

    # ------------------------------------------------------------------
    # Causal-test helpers (operate on the *observation* the agent sees;
    # the true env state is never touched by ablations).
    # ------------------------------------------------------------------
    @staticmethod
    def shuffle_internal(obs: np.ndarray, other_internal: np.ndarray) -> np.ndarray:
        """C1: replace internal channels of obs with another episode's."""
        out = obs.copy()
        out[: dyn.N_INTERNAL] = other_internal
        return out

    @staticmethod
    def mask_internal(obs: np.ndarray, index: int, value: float) -> np.ndarray:
        """C2: fix one internal channel to a constant."""
        out = obs.copy()
        out[index] = value
        return out

    @staticmethod
    def permute_obs(obs: np.ndarray, permutation: np.ndarray) -> np.ndarray:
        """C3: permute observation channels."""
        return obs[permutation]
