"""Shared model interface for O0.

Every model exposes the same contract (mirroring t0):

- ``initial_state(batch, device)`` -> state tensor ``[B, H]`` (``[B, 0]``
  for memoryless models).  A single tensor keeps hidden-state
  interventions (reset / noise) uniform across architectures; LSTM packs
  ``h || c`` along the feature axis.
- ``forward(obs[B,C], state)`` -> ``(pos[B,2], exist_logit[B],
  id_logit[B], new_state)`` where pos = (x, v) in normalized units.
- ``forward_sequence(obs[T,B,C], state)`` -> ``(pos[T,B,2],
  exist[T,B], idl[T,B], final_state)``.
"""
import torch
from torch import nn


class ModelBase(nn.Module):
    obs_dim = 20
    hidden_size = 0

    def stats(self):
        count = sum(p.numel() for p in self.parameters())
        return {"params": count, "hidden_size": self.hidden_size}

    def state_metrics(self, state):
        if not state.numel():
            return {"hidden_norm": 0.0, "hidden_variance": 0.0}
        return {
            "hidden_norm": state.detach().norm(dim=-1).mean().item(),
            "hidden_variance": state.detach().var(unbiased=False).item(),
        }

    def forward_sequence(self, observations, state):
        pos, exist, idl = [], [], []
        for observation in observations.unbind(0):
            p, e, i, state = self(observation, state)
            pos.append(p)
            exist.append(e)
            idl.append(i)
        return torch.stack(pos), torch.stack(exist), torch.stack(idl), state
