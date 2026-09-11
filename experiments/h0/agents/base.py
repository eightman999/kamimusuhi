"""Agent interface for H0.

Non-torch agents implement act(obs) -> int.
Torch agents additionally expose a `model` (nn.Module) used by PPO.
"""

from __future__ import annotations

import numpy as np


class Agent:
    name = "agent"

    def reset(self) -> None:
        """Called at the start of each episode."""

    def act(self, obs: np.ndarray) -> int:
        raise NotImplementedError
