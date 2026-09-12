"""O0 occlusion environment — Gym-like wrapper over generate_batch.

Minimal API (no gym dependency), matching the sibling experiments:

    env = OcclusionEnv(params, seed=0)
    obs = env.reset()
    obs, reward, done, info = env.step(action)

The task is pure prediction, so `action` is ignored and reward is always
0.0.  `reset()` generates one full episode deterministically from the
seed; `step()` replays it.  `info` carries the trainer-only labels for
the current step (exist / pos / vel / same / occluded / visible /
id_mask); `env.episode` holds the whole labelled episode for analysis.
"""

from __future__ import annotations

from typing import Optional

import numpy as np

from . import dynamics as dyn


class OcclusionEnv:
    def __init__(self, params: Optional[dyn.EnvParams] = None, seed: int = 0):
        self.params = params or dyn.EnvParams()
        self.seed_value = seed
        self.rng = np.random.default_rng(seed)
        self.t = 0
        self.episode: Optional[dict] = None
        self.done = True

    # ------------------------------------------------------------------
    def reset(self) -> np.ndarray:
        self.rng = np.random.default_rng(self.seed_value)
        ep = dyn.generate_batch(self.params, 1, self.rng)
        self.episode = {k: (v[:, 0] if v.ndim == 2 else v[0]) for k, v in ep.items()}
        self.episode["obs"] = ep["obs"][:, 0]  # [T, OBS_DIM]
        self.t = 0
        self.done = False
        return self.episode["obs"][0].copy()

    def reseed(self, seed: int) -> np.ndarray:
        """Reset with a new seed (used between episodes in rollouts)."""
        self.seed_value = seed
        return self.reset()

    # ------------------------------------------------------------------
    def _label_info(self, t: int) -> dict:
        ep = self.episode
        return {
            "t": t,
            "exist": float(ep["exist"][t]),
            "pos": float(ep["pos"][t]),
            "vel": float(ep["vel"][t]),
            "same": float(ep["same"][t]),
            "occluded": bool(ep["occluded"][t]),
            "visible": bool(ep["visible"][t]),
            "id_mask": bool(ep["id_mask"][t]),
        }

    def step(self, action: int = 0):
        if self.done:
            raise RuntimeError("step() called on a finished episode; call reset()")
        del action  # prediction task: actions do not influence the world
        self.t += 1
        T = self.params.horizon
        if self.t >= T - 1:
            self.done = True
            self.t = T - 1
        info = self._label_info(self.t)
        return self.episode["obs"][self.t].copy(), 0.0, self.done, info
