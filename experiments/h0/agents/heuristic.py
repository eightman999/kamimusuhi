"""B1: hand-written threshold policy (Fixed).

Priority-ordered rules over internal state. This is the baseline a learned
policy must match in-distribution and beat under OOD shift.
"""

from __future__ import annotations

import numpy as np

from ..env import dynamics as dyn
from .base import Agent


class HeuristicAgent(Agent):
    name = "heuristic"

    def act(self, obs: np.ndarray) -> int:
        energy, fatigue, novelty, certainty, temp = obs[: dyn.N_INTERNAL]
        resource = obs[dyn.N_INTERNAL + dyn.RESOURCE]
        hazard = obs[dyn.N_INTERNAL + dyn.HAZARD]
        if energy < 0.45:
            # REST only pays off when local resource remains; else forage.
            return dyn.REST if resource > 0.25 else dyn.EXPLORE
        if fatigue > 0.55:
            return dyn.REST if resource > 0.25 else dyn.WAIT
        if temp < 0.38 or temp > 0.62:
            return dyn.WAIT  # shelter toward 0.5
        if hazard > 0.4:
            return dyn.OBSERVE  # reduce hazard exposure
        if certainty < 0.45:
            return dyn.OBSERVE
        if novelty < 0.35 or resource < 0.4:
            return dyn.EXPLORE
        return dyn.WAIT
