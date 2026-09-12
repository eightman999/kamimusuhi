"""Policy interface for U0.

Non-torch policies implement ``decide(env) -> action`` and may read the
env's ground-truth state (baselines/oracle only). Torch policies are
wrapped by ``evaluate.PolicyWrapper`` and act on observations only.
"""

from __future__ import annotations


class Policy:
    name = "policy"
    evict_policy = "fifo"

    def reset(self) -> None:
        """Called at the start of each episode."""

    def decide(self, env) -> int:
        raise NotImplementedError
