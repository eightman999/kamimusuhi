import torch
from torch import nn

from .base import ModelBase


class LSTMCore(ModelBase):
    """LSTM with h and c packed into a single [B, 2*H] state tensor."""

    def __init__(self, obs_dim=8, num_actions=3, hidden_size=64):
        super().__init__()
        self.obs_dim, self.num_actions = obs_dim, num_actions
        self.hidden_size = hidden_size
        self.recurrent = nn.LSTM(obs_dim, hidden_size)
        self.actor = nn.Linear(hidden_size, num_actions)
        self.critic = nn.Linear(hidden_size, 1)

    def initial_state(self, batch, device):
        return torch.zeros(batch, 2 * self.hidden_size, device=device)

    def _split(self, state):
        h, c = state[:, :self.hidden_size], state[:, self.hidden_size:]
        return h.unsqueeze(0).contiguous(), c.unsqueeze(0).contiguous()

    def forward(self, observation, state):
        h, c = self._split(state)
        output, (h_new, c_new) = self.recurrent(observation.unsqueeze(0), (h, c))
        out = output[0]
        new_state = torch.cat([h_new[0], c_new[0]], dim=-1)
        return self.actor(out), self.critic(out).squeeze(-1), new_state

    def forward_sequence(self, observations, state):
        h, c = self._split(state)
        output, (h_new, c_new) = self.recurrent(observations, (h, c))
        return self.actor(output), self.critic(output).squeeze(-1), \
            torch.cat([h_new[0], c_new[0]], dim=-1)

    def stats(self):
        result = super().stats()
        result["gates"] = self.hidden_size * 4
        return result
