import torch
from torch import nn
from .gru import ModelBase


class MLPCore(ModelBase):
    """No recurrence: identical current observations imply identical policies."""
    def __init__(self, hidden_size=128):
        super().__init__()
        self.hidden_size = 0
        self.encoder = nn.Sequential(nn.Linear(16, hidden_size), nn.Tanh(),
                                     nn.Linear(hidden_size, hidden_size), nn.Tanh())
        self.actor = nn.Linear(hidden_size, 6)
        self.critic = nn.Linear(hidden_size, 1)

    def initial_state(self, batch, device):
        return torch.empty(batch, 0, device=device)

    def forward(self, observation, state):
        encoded = self.encoder(observation)
        return self.actor(encoded), self.critic(encoded).squeeze(-1), state

    def forward_sequence(self, observations, state):
        encoded = self.encoder(observations)
        return self.actor(encoded), self.critic(encoded).squeeze(-1), state
