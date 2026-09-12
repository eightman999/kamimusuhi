import torch
from torch import nn

from .base import ModelBase


class MLPCore(ModelBase):
    """No recurrence: identical current observations imply identical policies.

    Serves as the memoryless baseline for T0-H1.  Any timing behaviour it shows
    can only come from the stochastic dynamics channels, not from an internal
    elapsed-time representation.
    """

    def __init__(self, obs_dim=8, num_actions=3, hidden_size=128):
        super().__init__()
        self.obs_dim, self.num_actions = obs_dim, num_actions
        self.hidden_size = 0
        self.encoder = nn.Sequential(nn.Linear(obs_dim, hidden_size), nn.Tanh(),
                                     nn.Linear(hidden_size, hidden_size), nn.Tanh())
        self.actor = nn.Linear(hidden_size, num_actions)
        self.critic = nn.Linear(hidden_size, 1)

    def initial_state(self, batch, device):
        return torch.empty(batch, 0, device=device)

    def forward(self, observation, state):
        encoded = self.encoder(observation)
        return self.actor(encoded), self.critic(encoded).squeeze(-1), state

    def forward_sequence(self, observations, state):
        encoded = self.encoder(observations)
        return self.actor(encoded), self.critic(encoded).squeeze(-1), state
