"""Per-channel next-observation predictor for P0.

One small MLP, shared across channels (permutation-equivariant), maps a
channel's belief features to (predicted value, predicted error):

    input  per channel i: [v_i, s_i, pv_i, g_i]
            v  = last observed value        pv = observed value before that
            s  = staleness / STALENESS_CAP   g = staleness before last obs
    output per channel i: (xhat_i, shat_i)
            xhat = predicted current value
            shat = predicted |x - xhat| ("anticipated surprise": always
                   computable, needs no ground truth)

Trained online: when channel i is observed, target = the observed value.
Loss = MSE(xhat, target) + MSE(shat, |xhat - target|.detach()), masked to
observed channels only. The predictor NEVER sees unobserved truth -- this
is what keeps the surprise features leakage-free.

The two surprise features fed to the agent:
    e_i = EMA of |xhat - target| measured at observation times ("recent
          prediction error": high for C always, high for D mid-ramp/event)
    d_i = shat_i ("how surprising do I expect this channel to be now")
Both are squashed to ~[0, 1] by clip(err_scale * ., 0, 1) in the harness.
"""

from __future__ import annotations

from typing import Optional, Tuple

import numpy as np
import torch
import torch.nn as nn

from ..env import dynamics as dyn

# predictor input features per channel (subset of the agent block)
PRED_IN = 4   # [v, s, pv, g]
PRED_OUT = 2  # [xhat, shat]


def layer_init(layer: nn.Linear, std: float = 2**0.5, bias: float = 0.0):
    nn.init.orthogonal_(layer.weight, std)
    nn.init.constant_(layer.bias, bias)
    return layer


class ChannelPredictor(nn.Module):
    """Shared-weight per-channel predictor + online SGD trainer.

    Also owns the per-channel EMA error state `e` (one value per channel,
    reset per episode by `reset_state`).
    """

    def __init__(self, hidden: int = 32, lr: float = 3e-3, ema: float = 0.9,
                 seed: Optional[int] = None):
        # Seeded init so evaluations are deterministic per seed (the global
        # torch RNG is consumed by layer_init; seeding here makes a fresh
        # predictor reproducible instead of init-order dependent).
        if seed is not None:
            torch.manual_seed(seed)
        super().__init__()
        self.net = nn.Sequential(
            layer_init(nn.Linear(PRED_IN, hidden)),
            nn.Tanh(),
            layer_init(nn.Linear(hidden, hidden)),
            nn.Tanh(),
            layer_init(nn.Linear(hidden, PRED_OUT), std=1.0),
        )
        self.opt = torch.optim.Adam(self.parameters(), lr=lr)
        self.ema = ema
        self.n_envs: Optional[int] = None
        self.e: Optional[torch.Tensor] = None  # (n_envs, n_channels)

    # ------------------------------------------------------------------
    def reset_state(self, n_envs: int, device: str = "cpu"):
        """Fresh EMA-error state for a batch of episodes."""
        self.n_envs = n_envs
        self.e = torch.zeros(n_envs, dyn.N_CHANNELS, device=device)

    def forward(self, feats: torch.Tensor) -> Tuple[torch.Tensor, torch.Tensor]:
        """feats: (..., n_channels, PRED_IN) -> (xhat, shat) each (..., C)."""
        out = self.net(feats)  # Linear acts on last dim -> (..., C, 2)
        return out[..., 0], torch.nn.functional.softplus(out[..., 1])

    # ------------------------------------------------------------------
    @torch.no_grad()
    def features(self, feats: torch.Tensor) -> Tuple[np.ndarray, np.ndarray]:
        """Return (e, d) agent features as numpy (n_envs, n_channels)."""
        _, shat = self.forward(feats)
        return self.e.cpu().numpy(), shat.cpu().numpy()

    def update(self, feats: torch.Tensor, obs_mask: torch.Tensor,
               targets: torch.Tensor, train: bool = True) -> float:
        """One online SGD step on newly observed channels.

        feats:    (n_envs, C, PRED_IN) beliefs *before* the observation
        obs_mask: (n_envs, C) bool/float -- channels observed this step
        targets:  (n_envs, C) observed values (only obs_mask entries used)
        train:    if False, skip the gradient step (frozen-predictor eval).
                  The EMA error state `e` still updates: it is an agent-side
                  running statistic, not learned weights.

        Also updates the EMA error state `e` for observed channels.
        Returns (masked loss, abs_err ndarray (n_envs, C); 0 where unobserved).
        """
        with torch.set_grad_enabled(train):
            xhat, shat = self.forward(feats)
            abs_err = (xhat - targets).abs()
            m = obs_mask.float()
            denom = m.sum().clamp_min(1.0)
            loss_val = ((xhat - targets) ** 2 * m).sum() / denom
            loss_err = ((shat - abs_err.detach()) ** 2 * m).sum() / denom
            loss = loss_val + loss_err
            if train:
                self.opt.zero_grad()
                loss.backward()
                self.opt.step()
        with torch.no_grad():
            self.e = torch.where(
                obs_mask,
                self.ema * self.e + (1.0 - self.ema) * abs_err.detach(),
                self.e,
            )
            err_np = (abs_err * m).cpu().numpy()
        return float(loss.item()), err_np
