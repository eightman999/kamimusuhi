"""Torch actor-critic models for P0 (MLP and GRU).

Same interface as h0 so PPO and evaluation can treat them uniformly:

    logits, hidden = model.policy_logits(obs, hidden)   # obs: (..., OBS_DIM)
    value,  hidden = model.value(obs, hidden)
    logits, value, hidden = model.forward(obs, hidden)

`hidden` is None for MLP; a (1, batch, H) tensor for GRU.

OBS_DIM = N_CHANNELS * FEATS_PER_CHANNEL = 4 * 6 = 24.
"""

from __future__ import annotations

from typing import Optional

import torch
import torch.nn as nn

from ..config import AgentSpec
from ..env import dynamics as dyn
from ..env.attention_env import AttentionEnv
from .predictor import layer_init

OBS_DIM = dyn.N_CHANNELS * AttentionEnv.FEATS_PER_CHANNEL  # 24


class MLPActorCritic(nn.Module):
    """Reactive MLP policy over the belief+surprise state (no memory)."""

    recurrent = False

    def __init__(self, obs_dim: int = OBS_DIM, n_actions: int = dyn.N_ACTIONS,
                 hidden: int = 64):
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
    """GRU actor-critic. hidden: (1, batch, H) or None."""

    recurrent = True

    def __init__(self, obs_dim: int = OBS_DIM, n_actions: int = dyn.N_ACTIONS,
                 hidden: int = 64):
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
        h_prev = hidden.squeeze(0) if hidden is not None else None
        h = self.gru(torch.tanh(self.inp(obs)), h_prev)
        logits = self.actor(h)
        value = self.critic(h).squeeze(-1)
        h_out = h.unsqueeze(0)
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
