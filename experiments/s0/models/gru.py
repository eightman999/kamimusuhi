"""Recurrent dynamics models (GRU over observation/action sequences)."""

from __future__ import annotations

import torch
import torch.nn as nn
import torch.nn.functional as F


class GRUModel(nn.Module):
    """GRU dynamics model.

    forward(obs, act, h0=None) with obs (B,T,D), act (B,T) int64.
    Returns (pred, latent, hT) where latent is the GRU output sequence
    (B,T,H) — used by the linear probes.
    """

    is_recurrent = True

    def __init__(self, obs_dim: int, n_actions: int, hidden: int = 64,
                 use_action: bool = True, target_delta: bool = True):
        super().__init__()
        self.obs_dim = obs_dim
        self.n_actions = n_actions
        self.use_action = use_action
        self.target_delta = target_delta
        in_dim = obs_dim + (n_actions if use_action else 0)
        self.gru = nn.GRU(in_dim, hidden, batch_first=True)
        self.head = nn.Linear(hidden, obs_dim)
        self.latent_dim = hidden

    def forward(self, obs, act, h0=None):
        x = obs
        if self.use_action:
            x = torch.cat([obs, F.one_hot(act, self.n_actions).float()], dim=-1)
        h, hT = self.gru(x, h0)
        out = self.head(h)
        pred = obs + out if self.target_delta else out
        return pred, h, hT
