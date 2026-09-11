"""Torch models for U0.

The agent-side stack has three pieces:

    Classifier   obs -> class logits              (what is the class?)
    ConfNet      obs -> P(argmax correct) in [0,1] (will I be right?)
    MetaPolicy   u   -> {ANSWER, OBSERVE, ABSTAIN} + value

where ``u`` is the small *uncertainty feature vector* -- the ONLY
task-relevant input to the decision policy:

    u = [conf, margin, var_z, n_frac]
         0      1       2      3

conf / margin come from the (frozen) classifier+conf nets, var_z and
n_frac are read out of the observation bookkeeping channels.  This
bottleneck is what makes the causal tests surgical: shuffling or masking
u[:, :3] removes every uncertainty signal the decision head may use,
while n_frac (the observation budget, bookkeeping rather than
uncertainty) is left intact.
"""

from __future__ import annotations

import torch
import torch.nn as nn

# uncertainty-feature layout
U_CONF, U_MARGIN, U_VAR, U_N = 0, 1, 2, 3
U_DIM = 4
U_UNC = 3                      # first U_UNC dims are shuffled/masked/injected

META_ANSWER, META_OBSERVE, META_ABSTAIN = 0, 1, 2
N_META = 3


def mlp(in_dim: int, hidden: int, out_dim: int, depth: int = 2,
        dropout: float = 0.0) -> nn.Sequential:
    layers = []
    d = in_dim
    for _ in range(depth):
        layers += [nn.Linear(d, hidden), nn.Tanh()]
        if dropout:
            layers.append(nn.Dropout(dropout))
        d = hidden
    layers.append(nn.Linear(d, out_dim))
    return nn.Sequential(*layers)


class Classifier(nn.Module):
    def __init__(self, obs_dim: int, n_classes: int, hidden: int = 64):
        super().__init__()
        self.net = mlp(obs_dim, hidden, n_classes)
        self.obs_dim, self.n_classes = obs_dim, n_classes

    def forward(self, obs: torch.Tensor) -> torch.Tensor:
        return self.net(obs)


class DropoutClassifier(nn.Module):
    """Classifier with dropout kept live at eval for MC-dropout sampling."""

    def __init__(self, obs_dim: int, n_classes: int, hidden: int = 64,
                 p: float = 0.15):
        super().__init__()
        self.net = mlp(obs_dim, hidden, n_classes, dropout=p)
        self.obs_dim, self.n_classes = obs_dim, n_classes

    def forward(self, obs: torch.Tensor) -> torch.Tensor:
        return self.net(obs)

    def enable_dropout(self) -> None:
        for m in self.modules():
            if isinstance(m, nn.Dropout):
                m.train()


class ConfNet(nn.Module):
    """Learned confidence head: P(classifier argmax is correct | obs)."""

    def __init__(self, obs_dim: int, hidden: int = 64):
        super().__init__()
        self.net = mlp(obs_dim, hidden, 1)
        self.obs_dim = obs_dim

    def forward(self, obs: torch.Tensor) -> torch.Tensor:
        return torch.sigmoid(self.net(obs)).squeeze(-1)


class MetaPolicy(nn.Module):
    """Actor-critic over the uncertainty feature vector u only."""

    def __init__(self, u_dim: int = U_DIM, hidden: int = 64):
        super().__init__()
        self.actor = mlp(u_dim, hidden, N_META)
        self.critic = mlp(u_dim, hidden, 1)

    def forward(self, u: torch.Tensor):
        return self.actor(u), self.critic(u).squeeze(-1)


def uncertainty_features(class_logits: torch.Tensor, conf: torch.Tensor,
                         obs: torch.Tensor, feat_dim: int) -> torch.Tensor:
    """Build u = [conf, margin, var_z, n_frac] from model outputs + obs."""
    probs = torch.softmax(class_logits, dim=-1)
    top2 = torch.topk(probs, 2, dim=-1).values
    margin = top2[:, 0] - top2[:, 1]
    return torch.stack([conf, margin, obs[:, feat_dim],
                        obs[:, feat_dim + 1]], dim=-1)


def build_model(name: str, obs_dim: int, n_classes: int,
                hidden: int = 64) -> nn.Module:
    if name == "classifier":
        return Classifier(obs_dim, n_classes, hidden)
    if name == "dropout_classifier":
        return DropoutClassifier(obs_dim, n_classes, hidden)
    if name == "confnet":
        return ConfNet(obs_dim, hidden)
    if name == "metapolicy":
        return MetaPolicy(U_DIM, hidden)
    raise ValueError(f"unknown model {name}")
