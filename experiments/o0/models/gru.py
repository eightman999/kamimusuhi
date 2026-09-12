import torch
from torch import nn

from .base import ModelBase


class GRUCore(ModelBase):
    def __init__(self, obs_dim=20, hidden_size=64):
        super().__init__()
        self.obs_dim = obs_dim
        self.hidden_size = hidden_size
        self.recurrent = nn.GRU(obs_dim, hidden_size)
        self.pos_head = nn.Linear(hidden_size, 2)
        self.exist_head = nn.Linear(hidden_size, 1)
        self.id_head = nn.Linear(hidden_size, 1)

    def initial_state(self, batch, device):
        return torch.zeros(batch, self.hidden_size, device=device)

    def _out(self, out):
        return (
            self.pos_head(out),
            self.exist_head(out).squeeze(-1),
            self.id_head(out).squeeze(-1),
        )

    def forward(self, observation, state):
        output, hidden = self.recurrent(
            observation.unsqueeze(0), state.unsqueeze(0).contiguous()
        )
        state = hidden[0]
        p, e, i = self._out(output[0])
        return p, e, i, state

    def forward_sequence(self, observations, state):
        output, hidden = self.recurrent(
            observations, state.unsqueeze(0).contiguous()
        )
        p, e, i = self._out(output)
        return p, e, i, hidden[0]

    def stats(self):
        result = super().stats()
        result["gates"] = self.hidden_size * 3
        return result
