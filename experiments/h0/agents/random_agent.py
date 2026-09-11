"""B0: uniform random policy."""

from __future__ import annotations

import numpy as np

from ..env import dynamics as dyn
from .base import Agent


class RandomAgent(Agent):
    name = "random"

    def __init__(self, seed: int = 0):
        self.rng = np.random.default_rng(seed)

    def act(self, obs: np.ndarray) -> int:
        return int(self.rng.integers(dyn.N_ACTIONS))
