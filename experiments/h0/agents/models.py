"""Torch actor-critic models for H0 (B2 MLP, B3/B4 GRU).

Both expose the same interface used by PPO and evaluation:

    logits, hidden = model.policy_logits(obs, hidden)   # obs: (..., OBS_DIM)
    value,  hidden = model.value(obs, hidden)
    logits, value, hidden = model.forward(obs, hidden)

`hidden` is None for MLP; a (1, batch, H) tensor for GRU.
"""

from __future__ import annotations

from typing import Optional, Tuple

import torch
import torch.nn as nn

from ..config import AgentSpec
from ..env import dynamics as dyn


def layer_init(layer: nn.Linear, std: float = 2**0.5, bias: float = 0.0):
    nn.init.orthogonal_(layer.weight, std)
    nn.init.constant_(layer.bias, bias)
    return layer


class MLPActorCritic(nn.Module):
    """B2: reactive MLP, no memory."""

    recurrent = False

    def __init__(self, obs_dim: int = dyn.OBS_DIM, n_actions: int = dyn.N_ACTIONS, hidden: int = 64):
        super().__init__()
        self.body = nn.Sequential(
            layer_init(nn.Linear(obs_dim, hidden)),
            nn.Tanh(),
            layer_init(nn.Linear(hidden, hidden)),
            nn.Tanh(),
        )
        self.actor = layer_init(nn.Linear(hidden, n_actions), std=0.01)
        self.critic = layer_init(nn.Linear(hidden, 1), std=1.0)

    def _feat(self, obs: torch.Tensor) -> torch.Tensor:
        return self.body(obs)

    def policy_logits(self, obs, hidden=None):
        return self.actor(self._feat(obs)), None

    def value(self, obs, hidden=None):
        return self.critic(self._feat(obs)).squeeze(-1), None

    def forward(self, obs, hidden=None):
        f = self._feat(obs)
        return self.actor(f), self.critic(f).squeeze(-1), None


class GRUActorCritic(nn.Module):
    """B3/B4: GRU actor-critic. hidden: (1, batch, H) or None."""

    recurrent = True

    def __init__(self, obs_dim: int = dyn.OBS_DIM, n_actions: int = dyn.N_ACTIONS, hidden: int = 64):
        super().__init__()
        self.hidden_size = hidden
        self.inp = layer_init(nn.Linear(obs_dim, hidden))
        self.gru = nn.GRUCell(hidden, hidden)
        self.actor = layer_init(nn.Linear(hidden, n_actions), std=0.01)
        self.critic = layer_init(nn.Linear(hidden, 1), std=1.0)

    def _step(self, obs: torch.Tensor, hidden: Optional[torch.Tensor]):
        single = obs.dim() == 1
        if single:
            obs = obs.unsqueeze(0)
        h_prev = None
        if hidden is not None:
            h_prev = hidden.squeeze(0)  # (batch, H)
        h = self.gru(torch.tanh(self.inp(obs)), h_prev)
        logits = self.actor(h)
        value = self.critic(h).squeeze(-1)
        h_out = h.unsqueeze(0)  # (1, batch, H)
        if single:
            return logits.squeeze(0), value.squeeze(0), h_out
        return logits, value, h_out

    def policy_logits(self, obs, hidden=None):
        logits, _, h = self._step(obs, hidden)
        return logits, h

    def value(self, obs, hidden=None):
        _, v, h = self._step(obs, hidden)
        return v, h

    def forward(self, obs, hidden=None):
        return self._step(obs, hidden)


def build_model(spec: AgentSpec) -> nn.Module:
    if spec.kind == "mlp":
        return MLPActorCritic(hidden=spec.hidden)
    if spec.kind == "gru":
        return GRUActorCritic(hidden=spec.hidden)
    raise ValueError(f"build_model: unknown kind {spec.kind}")
