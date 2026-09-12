"""Shared model interface.

Every model exposes the same contract:

- ``initial_state(batch, device)`` -> state tensor ``[B, H]`` (``[B, 0]`` for
  memoryless models).  A single tensor keeps hidden-state interventions
  (reset / noise / quantization) uniform across architectures; LSTM packs
  ``h || c`` along the feature axis.
- ``forward(obs[B,C], state)`` -> ``(logits[B,A], value[B], new_state)``
- ``forward_sequence(obs[T,B,C], state)`` -> ``(logits[T,B,A], values[T,B],
  final_state)``
"""
import torch
from torch import nn


class ModelBase(nn.Module):
    obs_dim = 8
    num_actions = 3
    hidden_size = 0

    def stats(self):
        count = sum(p.numel() for p in self.parameters())
        return {"params": count, "hidden_size": self.hidden_size}

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
