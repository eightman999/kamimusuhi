"""Learned policies for U0 (mlp, gru64, gru128).

A policy maps observation -> (action logits, value). Single 6-way action
head; the GRU hidden state resets to zero at every episode boundary, so
PPO can re-run complete stored observation sequences during the update
epochs.
"""

from __future__ import annotations

import torch
import torch.nn as nn

from ..env.u0_env import N_ACTIONS, OBS_DIM


def _init(layer: nn.Linear, std: float = 2 ** 0.5, bias: float = 0.0):
    nn.init.orthogonal_(layer.weight, std)
    nn.init.constant_(layer.bias, bias)
    return layer


class MLPPolicy(nn.Module):
    recurrent = False

    def __init__(self, obs_dim: int = OBS_DIM, hidden: int = 256):
        super().__init__()
        self.trunk = nn.Sequential(
            _init(nn.Linear(obs_dim, hidden)), nn.Tanh(),
            _init(nn.Linear(hidden, hidden)), nn.Tanh())
        self.act_head = _init(nn.Linear(hidden, N_ACTIONS), std=0.01)
        self.val_head = _init(nn.Linear(hidden, 1), std=1.0)

    def initial_state(self, batch: int, device):
        return None

    def forward(self, obs: torch.Tensor, h=None):
        x = self.trunk(obs)
        return (self.act_head(x), self.val_head(x).squeeze(-1), None)


class GRUPolicy(nn.Module):
    recurrent = True

    def __init__(self, obs_dim: int = OBS_DIM, hidden: int = 64):
        super().__init__()
        self.hidden_size = hidden
        self.inp = _init(nn.Linear(obs_dim, hidden))
        self.gru = nn.GRUCell(hidden, hidden)
        self.act_head = _init(nn.Linear(hidden, N_ACTIONS), std=0.01)
        self.val_head = _init(nn.Linear(hidden, 1), std=1.0)

    def initial_state(self, batch: int, device):
        return torch.zeros(batch, self.hidden_size, device=device)

    def forward(self, obs: torch.Tensor, h: torch.Tensor):
        single = obs.dim() == 1
        if single:
            obs = obs.unsqueeze(0)
        h = self.gru(torch.tanh(self.inp(obs)), h)
        logits = self.act_head(h)
        value = self.val_head(h).squeeze(-1)
        if single:
            return logits.squeeze(0), value.squeeze(0), h
        return logits, value, h


def build_policy(name: str, obs_dim: int = OBS_DIM) -> nn.Module:
    if name == "mlp":
        return MLPPolicy(obs_dim)
    if name == "gru64":
        return GRUPolicy(obs_dim, hidden=64)
    if name == "gru128":
        return GRUPolicy(obs_dim, hidden=128)
    raise ValueError(f"unknown policy {name}")
