import torch
from torch import nn
from .gru import ModelBase


class MaskedLinear(nn.Module):
    """Permanent structural mask; dense storage with zero forbidden gradients.

    Sparse *connectivity* does not imply sparse-kernel acceleration at this size.
    """
    def __init__(self, inputs, outputs, density=.1, mask=None):
        super().__init__()
        if not 0 < density <= 1:
            raise ValueError("density must be in (0,1]")
        if mask is None:
            flat = torch.zeros(inputs * outputs)
            flat[torch.randperm(flat.numel())[:max(1, round(flat.numel() * density))]] = 1
            mask = flat.reshape(outputs, inputs)
        self.register_buffer("mask", mask.float())
        self.weight = nn.Parameter(torch.randn(outputs, inputs) / max(1., inputs * density) ** .5)
        self.bias = nn.Parameter(torch.zeros(outputs))

    def forward(self, values):
        return nn.functional.linear(values, self.weight * self.mask, self.bias)


class SparseRNNCore(ModelBase):
    def __init__(self, hidden_size=128, density=.1):
        super().__init__()
        self.hidden_size = hidden_size
        self.input_layer = nn.Linear(16, hidden_size)
        self.recurrent = MaskedLinear(hidden_size, hidden_size, density)
        self.retention_logit = nn.Parameter(torch.full((hidden_size,), 2.))
        self.actor = nn.Linear(hidden_size, 6)
        self.critic = nn.Linear(hidden_size, 1)

    def initial_state(self, batch, device):
        return torch.zeros(batch, self.hidden_size, device=device)

    def forward(self, observation, state):
        candidate = torch.tanh(self.input_layer(observation) + self.recurrent(state))
        retention = self.retention_logit.sigmoid()
        state = retention * state + (1 - retention) * candidate
        return self.actor(state), self.critic(state).squeeze(-1), state
