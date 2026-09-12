"""Agent interface for P0.

Non-torch agents implement reset() / act(obs) -> int, where obs is the
24-dim blocked agent input (per channel: v, s, pv, g, e, d). Torch agents
additionally expose a `model` (nn.Module) used by PPO.
"""

from __future__ import annotations

import numpy as np


class Agent:
    name = "agent"

    def reset(self) -> None:
        """Called at the start of each episode."""

    def act(self, obs: np.ndarray) -> int:
        raise NotImplementedError
