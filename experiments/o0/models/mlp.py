import torch
from torch import nn

from .base import ModelBase


class MLPCore(ModelBase):
    """Memoryless baseline: identical current obs -> identical outputs.

    During occlusion the target channels are identically zero, so the MLP
    can only exploit the occluder bounds and the episode prior; it cannot
    track position, direction, or appearance across the gap.
    """

    def __init__(self, obs_dim=20, hidden_size=128):
        super().__init__()
        self.obs_dim = obs_dim
        self.hidden_size = 0
        self.encoder = nn.Sequential(
            nn.Linear(obs_dim, hidden_size), nn.Tanh(),
            nn.Linear(hidden_size, hidden_size), nn.Tanh(),
        )
        self.pos_head = nn.Linear(hidden_size, 2)
        self.exist_head = nn.Linear(hidden_size, 1)
        self.id_head = nn.Linear(hidden_size, 1)

    def initial_state(self, batch, device):
        return torch.empty(batch, 0, device=device)

    def _out(self, observation):
        f = self.encoder(observation)
        return (
            self.pos_head(f),
            self.exist_head(f).squeeze(-1),
            self.id_head(f).squeeze(-1),
        )

    def forward(self, observation, state):
        p, e, i = self._out(observation)
        return p, e, i, state

    def forward_sequence(self, observations, state):
        p, e, i = self._out(observations)
        return p, e, i, state
