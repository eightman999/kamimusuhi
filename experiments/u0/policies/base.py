"""Policy protocol for U0.

Every policy exposes the same batched interface (torch tensors in/out):

    u(obs)        -> (B, U_DIM) uncertainty feature vector
                     [conf, margin, var_z, n_frac]
    probs(obs)    -> (B, K) predicted class distribution (for calibration)
    decide(obs,u) -> (B,) flat action ids (0..K-1 answer, K abstain,
                     K+1 observe)
    act(obs)      -> decide(obs, u(obs))

Splitting `u` out of `decide` is what makes the causal tests (U-C1
shuffle, U-C2 mask, U-C5 injection) uniform across subjects: the eval
harness perturbs u between the two calls.
"""

from __future__ import annotations

import numpy as np
import torch

from ..models.nets import U_CONF, U_DIM, U_MARGIN, U_N, U_UNC, U_VAR


class Policy:
    name = "policy"
    uses_u = True               # False for policies that ignore uncertainty

    def reset(self) -> None:
        pass

    def u(self, obs: torch.Tensor) -> torch.Tensor:
        raise NotImplementedError

    def probs(self, obs: torch.Tensor) -> torch.Tensor:
        raise NotImplementedError

    def decide(self, obs: torch.Tensor, u: torch.Tensor) -> torch.Tensor:
        raise NotImplementedError

    def act(self, obs: torch.Tensor) -> torch.Tensor:
        return self.decide(obs, self.u(obs))


def u_from_probs(probs: torch.Tensor, conf: torch.Tensor,
                 obs: torch.Tensor, feat_dim: int) -> torch.Tensor:
    """Assemble u for policies whose confidence is not a ConfNet."""
    top2 = torch.topk(probs, 2, dim=-1).values
    margin = top2[:, 0] - top2[:, 1]
    return torch.stack([conf, margin, obs[:, feat_dim],
                        obs[:, feat_dim + 1]], dim=-1)


def perturb_u(u: torch.Tensor, mode: str,
              rng: torch.Generator) -> torch.Tensor:
    """Causal manipulations of the uncertainty signal (U-C1/C2/C5).

    Only the first U_UNC dims (conf, margin, var) are touched; n_frac is
    observation-budget bookkeeping, not an uncertainty signal.
    """
    if mode is None or mode == "none":
        return u
    out = u.clone()
    if mode == "uc1_shuffle":            # U-C1: swap signals across episodes
        perm = torch.randperm(u.shape[0], generator=rng)
        out[:, :U_UNC] = u[perm][:, :U_UNC]
    elif mode == "uc2_mask":             # U-C2: constant uncertainty
        out[:, :U_UNC] = torch.tensor([0.5, 0.2, 0.5])
    elif mode == "uc5_high":             # U-C5a: injected false confidence
        out[:, :U_UNC] = torch.tensor([0.98, 0.9, 0.05])
    elif mode == "uc5_low":              # U-C5b: injected false doubt
        out[:, :U_UNC] = torch.tensor([0.20, 0.02, 1.5])
    else:
        raise ValueError(f"unknown perturbation {mode}")
    return out
