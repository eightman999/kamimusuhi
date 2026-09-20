import torch
from torch import nn


class ModelBase(nn.Module):
    def stats(self):
        count = sum(p.numel() for p in self.parameters())
        active = 0
        for module in self.modules():
            for name, parameter in module.named_parameters(recurse=False):
                if "weight" not in name:
                    continue
                if name == "weight" and hasattr(module, "mask"):
                    active += int(module.mask.count_nonzero().item())
                else:
                    active += parameter.numel()
        return {"params": count, "active_connections": active, "gates": 0}

    def state_metrics(self, state):
        if not state.numel():
            return {"hidden_norm": 0., "hidden_variance": 0.}
        return {"hidden_norm": state.detach().norm(dim=-1).mean().item(),
                "hidden_variance": state.detach().var(unbiased=False).item()}

    def forward_sequence(self, observations, state):
        logits, values = [], []
        for observation in observations.unbind(0):
            logit, value, state = self(observation, state)
            logits.append(logit)
            values.append(value)
        return torch.stack(logits), torch.stack(values), state


class GRUCore(ModelBase):
    def __init__(self, hidden_size=64):
        super().__init__()
        self.hidden_size = hidden_size
        self.recurrent = nn.GRU(16, hidden_size)
        self.actor = nn.Linear(hidden_size, 6)
        self.critic = nn.Linear(hidden_size, 1)

    def initial_state(self, batch, device):
        return torch.zeros(batch, self.hidden_size, device=device)

    def forward(self, observation, state):
        output, hidden = self.recurrent(observation.unsqueeze(0), state.unsqueeze(0).contiguous())
        state = hidden[0]
        return self.actor(state), self.critic(state).squeeze(-1), state

    def forward_sequence(self, observations, state):
        output, hidden = self.recurrent(observations, state.unsqueeze(0).contiguous())
        return self.actor(output), self.critic(output).squeeze(-1), hidden[0]

    def stats(self):
        result = super().stats()
        result["gates"] = self.hidden_size * 3
        return result
