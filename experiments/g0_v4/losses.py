"""Losses for G0-v4 self-supervised objectives.

All losses consume (obs, next_obs, actions) only — no labels, no context
ids, no oracle signals. `compute_loss` lives on each model; this module
holds the shared terms (InfoNCE, VICReg variance/covariance, latent
forward prediction) plus the default next-obs loss for the GRU control.
"""

from __future__ import annotations

import torch
import torch.nn.functional as F


def default_next_loss(model, obs, nxt, act) -> tuple[torch.Tensor, dict]:
    """G0-compatible next-observation delta prediction (gru control)."""
    pred, _, aux = model(obs, act)
    loss = F.mse_loss(pred, nxt)
    if "loss_extra" in aux:
        loss = loss + aux["loss_extra"]
    return loss, {"loss": loss.item()}


def off_diagonal(x: torch.Tensor) -> torch.Tensor:
    """Sum of squared off-diagonal entries of a square matrix (/n)."""
    n, m = x.shape
    assert n == m
    return (x.flatten()[:-1].view(n - 1, n + 1)[:, 1:]).pow(2).sum() / n


def variance_hinge(z: torch.Tensor, gamma: float = 1.0) -> torch.Tensor:
    """VICReg variance term: push per-dim std toward >= gamma."""
    std = torch.sqrt(z.var(dim=0, unbiased=False) + 1e-4)
    return torch.relu(gamma - std).mean()


def covariance_penalty(z: torch.Tensor) -> torch.Tensor:
    """VICReg covariance term: mean squared off-diagonal of cov(z)."""
    n, d = z.shape
    z = z - z.mean(0)
    cov = (z.T @ z) / max(n - 1, 1)
    return off_diagonal(cov) / d


def vicreg_terms(z1: torch.Tensor, z2: torch.Tensor, sim_w: float,
                 var_w: float, cov_w: float,
                 gamma: float) -> tuple[torch.Tensor, dict]:
    """VICReg over two view embeddings, each (N, D) or (E, T, D)."""
    z1f = z1.reshape(-1, z1.shape[-1])
    z2f = z2.reshape(-1, z2.shape[-1])
    sim = F.mse_loss(z1f, z2f)
    var = variance_hinge(z1f, gamma) + variance_hinge(z2f, gamma)
    cov = covariance_penalty(z1f) + covariance_penalty(z2f)
    total = sim_w * sim + var_w * var + cov_w * cov
    return total, {"inv": sim.item(), "var": var.item(),
                   "cov": cov.item()}


def infonce_future(pred: torch.Tensor, z_all: torch.Tensor,
                   targets: torch.Tensor, temperature: float
                   ) -> torch.Tensor:
    """InfoNCE: match predicted latents to the true future step embedding.

    pred    (N, Z)  predictions f_k(c_t)
    z_all   (M, Z)  all step embeddings in the batch (negatives + the
                    positive at `targets` index)
    targets (N,)    flat index of the true z_{t+k}
    """
    logits = pred @ z_all.t() / temperature          # (N, M)
    return F.cross_entropy(logits, targets)
