import torch
from torch import nn
from .gru import ModelBase


class SoftLogicCore(ModelBase):
    """Fixed sparse two-input wiring, learned probabilistic gate choice.

    AND=a*b; OR=a+b-a*b; XOR=a+b-2*a*b; NAND=1-a*b.
    Discretized evaluation hard-selects gate types while keeping continuous
    sensor/state values; it does not claim binary input quantization.
    """
    def __init__(self, hidden_size=128):
        super().__init__()
        self.hidden_size = hidden_size
        source_a = torch.randint(0, hidden_size + 16, (hidden_size,))
        source_b = torch.randint(0, hidden_size + 16, (hidden_size,))
        source_a[:hidden_size // 2] = torch.arange(hidden_size // 2) % 16
        source_b[:hidden_size // 2] = torch.arange(hidden_size // 2) % hidden_size + 16
        self.register_buffer("source_a", source_a)
        self.register_buffer("source_b", source_b)
        self.gate_logits = nn.Parameter(torch.randn(hidden_size, 4) * .15)
        self.retention_logit = nn.Parameter(torch.full((hidden_size,), 1.5))
        self.actor = nn.Linear(hidden_size, 6)
        self.critic = nn.Linear(hidden_size, 1)
        self.discretized = False

    def set_discretized(self, enabled=True):
        self.discretized = bool(enabled)
        return self

    def initial_state(self, batch, device):
        return torch.full((batch, self.hidden_size), .5, device=device)

    def forward(self, observation, state):
        sources = torch.cat((observation.clamp(0, 1), state), -1)
        a, b = sources[:, self.source_a], sources[:, self.source_b]
        product = a * b
        gates = torch.stack((product, a + b - product, a + b - 2 * product, 1 - product), -1)
        if self.discretized:
            weights = nn.functional.one_hot(self.gate_logits.argmax(-1), 4).to(gates.dtype)
        else:
            weights = self.gate_logits.softmax(-1)
        candidate = (gates * weights).sum(-1)
        retention = self.retention_logit.sigmoid()
        state = retention * state + (1 - retention) * candidate
        return self.actor(state), self.critic(state).squeeze(-1), state

    def stats(self):
        result = super().stats()
        # Fixed wires count as active connections even without trainable weights.
        result["active_connections"] += self.hidden_size * 2
        result["gates"] = self.hidden_size
        result["gate_choices"] = 4
        return result
