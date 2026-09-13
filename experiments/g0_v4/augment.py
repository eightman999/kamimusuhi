"""Legal augmentations for G0-v4 — derived from observations ONLY.

These must NOT implement the true context transform C_j (that would
*teach* context invariance instead of letting it be discovered — see
the v4 spec, "shortcut prevention"). Allowed perturbations:

  * Gaussian jitter on observations (sensor-noise robustness)
  * sensor-dim dropout (zeroing a random subset per view)
  * both are applied to whole episodes; the temporal structure and the
    action stream are left untouched
"""

from __future__ import annotations

import torch


def _rand(shape, gen, device, dtype=torch.float32):
    """Generator-safe randn/rand: draw on CPU (gen lives there), move."""
    if gen is None:
        return torch.rand(shape, device=device, dtype=dtype)
    return torch.rand(shape, generator=gen, device="cpu",
                      dtype=dtype).to(device)


def _randn(shape, gen, device, dtype=torch.float32):
    if gen is None:
        return torch.randn(shape, device=device, dtype=dtype)
    return torch.randn(shape, generator=gen, device="cpu",
                       dtype=dtype).to(device)


def jitter(obs: torch.Tensor, std: float, gen) -> torch.Tensor:
    if std <= 0:
        return obs
    return obs + std * _randn(obs.shape, gen, obs.device, obs.dtype)


def sensor_dropout(obs: torch.Tensor, p: float, gen) -> torch.Tensor:
    """Zero a random subset of sensor dims, shared across time within an
    episode (a soft stand-in for a harness change, with no oracle
    knowledge of the true C_j)."""
    if p <= 0:
        return obs
    e, t, d = obs.shape
    keep = _rand((e, 1, d), gen, obs.device) > p
    return obs * keep.to(obs.dtype)


def make_view(obs: torch.Tensor, jitter_std: float, drop_p: float,
              gen: torch.Generator) -> torch.Tensor:
    """One augmented view of an episode batch (E,T,D)."""
    v = sensor_dropout(obs, drop_p, gen)
    return jitter(v, jitter_std, gen)
