"""Tiny diagonal SSM (real-valued S4D-style / leaky-integrator bank).

``h_i <- rho_i * h_i + (1 - rho_i) * u_i`` with learned decay rates
``rho_i`` initialized on a log-spaced grid of timescales, a learned input
projection ``u = B x``, and a nonlinear readout mix.  Decay rates are
input-independent, so persistent information lives in a bank of linear
modes rather than in gates.
"""
import math

import torch
from torch import nn

from .base import ModelBase


class TinySSMCore(ModelBase):
    def __init__(self, obs_dim=20, hidden_size=64,
                 min_timescale=2.0, max_timescale=256.0):
        super().__init__()
        self.obs_dim = obs_dim
        self.hidden_size = hidden_size
        rates = torch.exp(
            torch.linspace(math.log(min_timescale), math.log(max_timescale), hidden_size)
        )
        rho_init = torch.exp(-1.0 / rates).clamp(0.0, 0.999)
        self.rho_logit = nn.Parameter(torch.logit(rho_init))
        self.input_proj = nn.Linear(obs_dim, hidden_size)
        self.mix = nn.Sequential(nn.Linear(hidden_size, hidden_size), nn.Tanh())
        self.pos_head = nn.Linear(hidden_size, 2)
        self.exist_head = nn.Linear(hidden_size, 1)
        self.id_head = nn.Linear(hidden_size, 1)

    def initial_state(self, batch, device):
        return torch.zeros(batch, self.hidden_size, device=device)

    def _step(self, observation, state):
        rho = torch.sigmoid(self.rho_logit)
        u = self.input_proj(observation)
        return rho * state + (1.0 - rho) * u

    def _out(self, features):
        return (
            self.pos_head(features),
            self.exist_head(features).squeeze(-1),
            self.id_head(features).squeeze(-1),
        )

    def forward(self, observation, state):
        state = self._step(observation, state)
        p, e, i = self._out(self.mix(state))
        return p, e, i, state

    def forward_sequence(self, observations, state):
        pos, exist, idl = [], [], []
        for observation in observations.unbind(0):
            state = self._step(observation, state)
            p, e, i = self._out(self.mix(state))
            pos.append(p)
            exist.append(e)
            idl.append(i)
        return torch.stack(pos), torch.stack(exist), torch.stack(idl), state
