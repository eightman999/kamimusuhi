"""Feed-forward dynamics models (no temporal memory)."""

from __future__ import annotations

import torch
import torch.nn as nn
import torch.nn.functional as F


class MLPModel(nn.Module):
    """Predicts next observation (or delta) from (obs, action).

    forward(obs, act, h0=None) with obs (B,T,D), act (B,T) int64.
    Returns (pred, latent, None). Latent = penultimate activations,
    used by the linear probes.
    """

    is_recurrent = False

    def __init__(self, obs_dim: int, n_actions: int, hidden: int = 128,
                 use_action: bool = True, target_delta: bool = True):
        super().__init__()
        self.obs_dim = obs_dim
        self.n_actions = n_actions
        self.use_action = use_action
        self.target_delta = target_delta
        in_dim = obs_dim + (n_actions if use_action else 0)
        self.net = nn.Sequential(
            nn.Linear(in_dim, hidden), nn.ReLU(),
            nn.Linear(hidden, hidden), nn.ReLU(),
        )
        self.head = nn.Linear(hidden, obs_dim)
        self.latent_dim = hidden

    def _encode(self, obs: torch.Tensor, act: torch.Tensor) -> torch.Tensor:
        x = obs
        if self.use_action:
            x = torch.cat([obs, F.one_hot(act, self.n_actions).float()], dim=-1)
        return self.net(x)

    def forward(self, obs, act, h0=None):
        h = self._encode(obs, act)
        out = self.head(h)
        pred = obs + out if self.target_delta else out
        return pred, h, None
