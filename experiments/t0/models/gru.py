import torch
from torch import nn

from .base import ModelBase


class GRUCore(ModelBase):
    def __init__(self, obs_dim=8, num_actions=3, hidden_size=64):
        super().__init__()
        self.obs_dim, self.num_actions = obs_dim, num_actions
        self.hidden_size = hidden_size
        self.recurrent = nn.GRU(obs_dim, hidden_size)
        self.actor = nn.Linear(hidden_size, num_actions)
        self.critic = nn.Linear(hidden_size, 1)

    def initial_state(self, batch, device):
        return torch.zeros(batch, self.hidden_size, device=device)

    def forward(self, observation, state):
        output, hidden = self.recurrent(observation.unsqueeze(0),
                                        state.unsqueeze(0).contiguous())
        state = hidden[0]
        return self.actor(state), self.critic(state).squeeze(-1), state

    def forward_sequence(self, observations, state):
        output, hidden = self.recurrent(observations, state.unsqueeze(0).contiguous())
        return self.actor(output), self.critic(output).squeeze(-1), hidden[0]

    def stats(self):
        result = super().stats()
        result["gates"] = self.hidden_size * 3
        return result
