"""Learned policies for C0.

A policy maps its input vector -> (action logits, answer logits, value).
The answer head is only consulted when the action is ANSWER, giving a
factorized action space (act in 3) x (ans in V).

Variants:
    mlp        stateless probe (no recurrence, no memory): must fail any
               query that needs information from earlier steps
    gru64/128  GRUCell hidden state; the only carry-over is the vector h
    mem64/128  GRUCell + agent-side slot memory; the memory contents are
               concatenated into the input each step
"""
from __future__ import annotations

import torch
import torch.nn as nn
from torch.distributions import Categorical

from ..env.c0_env import N_ACTIONS, ANSWER

MODEL_SPECS = {
    "mlp":    {"hidden": 256, "memory": False, "recurrent": False},
    "gru64":  {"hidden": 64,  "memory": False, "recurrent": True},
    "gru128": {"hidden": 128, "memory": False, "recurrent": True},
    "mem64":  {"hidden": 64,  "memory": True,  "recurrent": True},
    "mem128": {"hidden": 128, "memory": True,  "recurrent": True},
}


def _heads(hidden: int, n_values: int):
    return (nn.Linear(hidden, N_ACTIONS),
            nn.Linear(hidden, n_values),
            nn.Linear(hidden, 1))


class MLPPolicy(nn.Module):
    def __init__(self, input_dim: int, n_values: int, hidden: int = 256):
        super().__init__()
        self.trunk = nn.Sequential(
            nn.Linear(input_dim, hidden), nn.Tanh(),
            nn.Linear(hidden, hidden), nn.Tanh())
        self.act_head, self.ans_head, self.val_head = _heads(hidden, n_values)

    def initial_state(self, batch: int, device):
        return None

    def forward(self, x: torch.Tensor, h=None):
        z = self.trunk(x)
        return (self.act_head(z), self.ans_head(z),
                self.val_head(z).squeeze(-1), None)


class GRUPolicy(nn.Module):
    def __init__(self, input_dim: int, n_values: int, hidden: int = 64):
        super().__init__()
        self.hidden = hidden
        self.inp = nn.Linear(input_dim, hidden)
        self.gru = nn.GRUCell(hidden, hidden)
        self.act_head, self.ans_head, self.val_head = _heads(hidden, n_values)

    def initial_state(self, batch: int, device):
        return torch.zeros(batch, self.hidden, device=device)

    def forward(self, x: torch.Tensor, h: torch.Tensor):
        h = self.gru(torch.tanh(self.inp(x)), h)
        return (self.act_head(h), self.ans_head(h),
                self.val_head(h).squeeze(-1), h)


def build_policy(name: str, env_cfg) -> nn.Module:
    """Build policy; input width includes memory contents for mem* models."""
    spec = MODEL_SPECS[name]
    in_dim = env_cfg.obs_dim
    if spec["memory"]:
        in_dim += env_cfg.memory_slots * env_cfg.payload_dim
    if spec["recurrent"]:
        return GRUPolicy(in_dim, env_cfg.num_values, spec["hidden"])
    return MLPPolicy(in_dim, env_cfg.num_values, spec["hidden"])


def factorized_logp(act_logits, ans_logits, act, ans):
    """Joint logp of (act, ans); the ans factor only counts on ANSWER."""
    act_logp = Categorical(logits=act_logits).log_prob(act)
    ans_logp = Categorical(logits=ans_logits).log_prob(ans)
    is_ans = (act == ANSWER).float()
    return act_logp + ans_logp * is_ans, is_ans


def factorized_entropy(act_logits, ans_logits):
    return (Categorical(logits=act_logits).entropy().mean()
            + 0.5 * Categorical(logits=ans_logits).entropy().mean())
