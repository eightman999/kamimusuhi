import torch
from torch import nn
from .gru import ModelBase
from .sparse_rnn import MaskedLinear


class FlyModularCore(ModelBase):
    """Four small recurrent modules with sparse directed communication.

    This borrows modularity, persistence and global modulation only; it is not
    an anatomical fly-brain reconstruction.
    """
    def __init__(self, hidden_size=128, density=.1):
        super().__init__()
        if hidden_size % 4 or not 64 <= hidden_size <= 256:
            raise ValueError("fly hidden_size must be 64..256 and divisible by 4")
        self.hidden_size = hidden_size
        self.module_size = m = hidden_size // 4
        self.module_names = ("sensory", "salience", "persistence", "action_selection")
        allowed = torch.zeros(hidden_size, hidden_size)
        # Matrix rows receive from columns. No dense bypass between modules.
        for destination, sources in enumerate(((0,), (0, 1), (1, 2), (0, 2, 3))):
            for source in sources:
                allowed[destination*m:(destination+1)*m, source*m:(source+1)*m] = 1
        available = allowed.flatten().nonzero().flatten()
        selected = available[torch.randperm(len(available))[:max(1, round(len(available) * density))]]
        mask = torch.zeros_like(allowed).flatten()
        mask[selected] = 1
        self.register_buffer("allowed_connections", allowed)
        self.recurrent = MaskedLinear(hidden_size, hidden_size, density, mask.reshape_as(allowed))
        self.sensory_input = nn.Linear(16, m)
        self.global_modulator = nn.Linear(m * 2, 1)
        self.retention_logit = nn.Parameter(torch.tensor([-.4, 0., 3., 0.]).repeat_interleave(m))
        self.actor = nn.Linear(m, 6)
        self.critic = nn.Linear(m, 1)

    def initial_state(self, batch, device):
        return torch.zeros(batch, self.hidden_size, device=device)

    def forward(self, observation, state):
        m = self.module_size
        sensory = self.sensory_input(observation)
        drive = torch.cat((sensory, torch.zeros_like(state[:, m:])), -1)
        candidate = torch.tanh(self.recurrent(state) + drive)
        modulation = self.global_modulator(torch.cat((sensory, state[:, m:2*m]), -1)).sigmoid()
        retention = (self.retention_logit[None, :] + modulation - .5).sigmoid()
        state = retention * state + (1 - retention) * candidate
        action_state = state[:, 3*m:]
        return self.actor(action_state), self.critic(action_state).squeeze(-1), state

    def state_metrics(self, state):
        result = super().state_metrics(state)
        for i, name in enumerate(self.module_names):
            result[f"module_{name}"] = state[:, i*self.module_size:(i+1)*self.module_size].detach().abs().mean().item()
        result["novelty"] = result["module_salience"]
        result["salience"] = result["module_salience"]
        result["persistence"] = result["module_persistence"]
        return result
