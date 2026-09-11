"""Learned memory-gate policies for R0.

A policy maps observation -> (action logits, answer logits, value). The
answer head is only consulted when the sampled action is ANSWER, giving a
factorized action space (act in 4) x (ans in V) without a joint softmax.

MLPGate has no recurrence: it can still solve the task because the recall
buffer and memory summary are part of the observation. GRUGate adds a
GRUCell hidden state (64/128) that persists within an episode and resets
to zero at episode boundaries, which lets PPO re-run full episodes from
h0 = 0 during the update epochs.
"""

from __future__ import annotations

import torch
import torch.nn as nn
from torch.distributions import Categorical

from ..env.r0_env import N_ACTIONS, ANSWER


def _heads(hidden: int, n_values: int):
    return (nn.Linear(hidden, N_ACTIONS),
            nn.Linear(hidden, n_values),
            nn.Linear(hidden, 1))


class MLPGate(nn.Module):
    def __init__(self, obs_dim: int, n_values: int, hidden: int = 256):
        super().__init__()
        self.trunk = nn.Sequential(
            nn.Linear(obs_dim, hidden), nn.Tanh(),
            nn.Linear(hidden, hidden), nn.Tanh())
        self.act_head, self.ans_head, self.val_head = _heads(hidden, n_values)
        self.recurrent = False

    def initial_state(self, batch: int, device):
        return None

    def forward(self, obs: torch.Tensor, h=None):
        x = self.trunk(obs)
        return self.act_head(x), self.ans_head(x), self.val_head(x).squeeze(-1), None


class GRUGate(nn.Module):
    def __init__(self, obs_dim: int, n_values: int, hidden: int = 64):
        super().__init__()
        self.hidden = hidden
        self.inp = nn.Linear(obs_dim, hidden)
        self.gru = nn.GRUCell(hidden, hidden)
        self.act_head, self.ans_head, self.val_head = _heads(hidden, n_values)
        self.recurrent = True

    def initial_state(self, batch: int, device):
        return torch.zeros(batch, self.hidden, device=device)

    def forward(self, obs: torch.Tensor, h: torch.Tensor):
        h = self.gru(torch.tanh(self.inp(obs)), h)
        return self.act_head(h), self.ans_head(h), self.val_head(h).squeeze(-1), h


def build_policy(name: str, obs_dim: int, n_values: int) -> nn.Module:
    if name == "mlp":
        return MLPGate(obs_dim, n_values)
    if name == "gru64":
        return GRUGate(obs_dim, n_values, hidden=64)
    if name == "gru128":
        return GRUGate(obs_dim, n_values, hidden=128)
    raise ValueError(f"unknown policy {name}")


def factorized_logp(act_logits, ans_logits, act, ans):
    """Joint logp of (act, ans); the ans factor only counts on ANSWER."""
    act_logp = Categorical(logits=act_logits).log_prob(act)
    ans_logp = Categorical(logits=ans_logits).log_prob(ans)
    is_ans = (act == ANSWER).float()
    return act_logp + ans_logp * is_ans, is_ans


def factorized_entropy(act_logits, ans_logits):
    return (Categorical(logits=act_logits).entropy().mean()
            + 0.5 * Categorical(logits=ans_logits).entropy().mean())
