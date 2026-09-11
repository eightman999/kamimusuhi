import torch
from torch import nn

from .base import ModelBase


class LSTMCore(ModelBase):
    """LSTM with h and c packed into a single [B, 2*H] state tensor."""

    def __init__(self, obs_dim=20, hidden_size=64):
        super().__init__()
        self.obs_dim = obs_dim
        self.hidden_size = hidden_size
        self.recurrent = nn.LSTM(obs_dim, hidden_size)
        self.pos_head = nn.Linear(hidden_size, 2)
        self.exist_head = nn.Linear(hidden_size, 1)
        self.id_head = nn.Linear(hidden_size, 1)

    def initial_state(self, batch, device):
        return torch.zeros(batch, 2 * self.hidden_size, device=device)

    def _split(self, state):
        h, c = state[:, : self.hidden_size], state[:, self.hidden_size :]
        return h.unsqueeze(0).contiguous(), c.unsqueeze(0).contiguous()

    def _out(self, out):
        return (
            self.pos_head(out),
            self.exist_head(out).squeeze(-1),
            self.id_head(out).squeeze(-1),
        )

    def forward(self, observation, state):
        h, c = self._split(state)
        output, (h_new, c_new) = self.recurrent(observation.unsqueeze(0), (h, c))
        out = output[0]
        p, e, i = self._out(out)
        new_state = torch.cat([h_new[0], c_new[0]], dim=-1)
        return p, e, i, new_state

    def forward_sequence(self, observations, state):
        h, c = self._split(state)
        output, (h_new, c_new) = self.recurrent(observations, (h, c))
        p, e, i = self._out(output)
        return p, e, i, torch.cat([h_new[0], c_new[0]], dim=-1)

    def stats(self):
        result = super().stats()
        result["gates"] = self.hidden_size * 4
        return result
