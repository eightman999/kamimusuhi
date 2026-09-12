"""Tiny diagonal SSM (real-valued S4D-style / LRU-lite).

``h_i <- rho_i * h_i + (1 - rho_i) * u_i`` with learned decay rates ``rho_i``
initialized on a log-spaced grid of timescales, a learned input projection
``u = B x``, a nonlinear readout mix, then actor/critic heads.  Decay rates are
input-independent, so any elapsed-time representation lives in the bank of
linear modes rather than in gates -- the hypothesis is that this produces more
linearly probeable time representations (spec section 13, Strong PASS).
"""
import math

import torch
from torch import nn

from .base import ModelBase


class TinySSMCore(ModelBase):
    def __init__(self, obs_dim=8, num_actions=3, hidden_size=64,
                 min_timescale=2., max_timescale=256.):
        super().__init__()
        self.obs_dim, self.num_actions = obs_dim, num_actions
        self.hidden_size = hidden_size
        rates = torch.exp(torch.linspace(math.log(min_timescale),
                                         math.log(max_timescale), hidden_size))
        rho_init = torch.exp(-1. / rates).clamp(0., .999)
        self.rho_logit = nn.Parameter(torch.logit(rho_init))
        self.input_proj = nn.Linear(obs_dim, hidden_size)
        self.mix = nn.Sequential(nn.Linear(hidden_size, hidden_size), nn.Tanh())
        self.actor = nn.Linear(hidden_size, num_actions)
        self.critic = nn.Linear(hidden_size, 1)

    def initial_state(self, batch, device):
        return torch.zeros(batch, self.hidden_size, device=device)

    def _step(self, observation, state):
        rho = torch.sigmoid(self.rho_logit)
        u = self.input_proj(observation)
        return rho * state + (1. - rho) * u

    def forward(self, observation, state):
        state = self._step(observation, state)
        features = self.mix(state)
        return self.actor(features), self.critic(features).squeeze(-1), state

    def forward_sequence(self, observations, state):
        rho = torch.sigmoid(self.rho_logit)
        outputs = []
        for observation in observations.unbind(0):
            state = rho * state + (1. - rho) * self.input_proj(observation)
            outputs.append(self.mix(state))
        features = torch.stack(outputs)
        return self.actor(features), self.critic(features).squeeze(-1), state


class LeakyCore(ModelBase):
    """Fixed-rate leaky integrator bank with a learned readout.

    Baseline with multi-timescale memory but no learned decay and no gating.
    """

    def __init__(self, obs_dim=8, num_actions=3, hidden_size=32,
                 min_timescale=2., max_timescale=256.):
        super().__init__()
        self.obs_dim, self.num_actions = obs_dim, num_actions
        self.hidden_size = hidden_size
        rates = torch.exp(torch.linspace(math.log(min_timescale),
                                         math.log(max_timescale), hidden_size))
        self.register_buffer("alpha", 1. - torch.exp(-1. / rates))
        self.input_proj = nn.Linear(obs_dim, hidden_size)
        self.actor = nn.Linear(hidden_size, num_actions)
        self.critic = nn.Linear(hidden_size, 1)

    def initial_state(self, batch, device):
        return torch.zeros(batch, self.hidden_size, device=device)

    def _step(self, observation, state):
        u = torch.tanh(self.input_proj(observation))
        return (1. - self.alpha) * state + self.alpha * u

    def forward(self, observation, state):
        state = self._step(observation, state)
        return self.actor(state), self.critic(state).squeeze(-1), state

    def forward_sequence(self, observations, state):
        logits, values = [], []
        for observation in observations.unbind(0):
            logit, value, state = self(observation, state)
            logits.append(logit)
            values.append(value)
        return torch.stack(logits), torch.stack(values), state
