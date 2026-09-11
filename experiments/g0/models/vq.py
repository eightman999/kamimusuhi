"""Vector quantization layer (EMA codebook, straight-through gradient)."""

from __future__ import annotations

import torch
import torch.nn as nn
import torch.nn.functional as F


class VectorQuantizerEMA(nn.Module):
    """VQ with EMA codebook updates.

    forward(z) with z (..., dim) -> (z_q_st, codes, loss, z_q)
      z_q_st : quantized tensor with straight-through gradient
      codes  : (...,) long code indices
      loss   : commitment loss (already scaled by `commitment`)
      z_q    : quantized tensor WITHOUT straight-through (eval/probes)
    """

    def __init__(self, n_codes: int, dim: int, commitment: float = 0.25,
                 decay: float = 0.99, eps: float = 1e-5):
        super().__init__()
        self.n_codes = n_codes
        self.dim = dim
        self.commitment = commitment
        self.decay = decay
        self.eps = eps
        embed = torch.randn(n_codes, dim) * 0.1
        self.register_buffer("embed", embed)
        self.register_buffer("cluster_size", torch.zeros(n_codes))
        self.register_buffer("embed_avg", embed.clone())

    def forward(self, z: torch.Tensor):
        shape = z.shape
        flat = z.reshape(-1, self.dim)
        d = (flat.pow(2).sum(1, keepdim=True)
             - 2 * flat @ self.embed.t()
             + self.embed.pow(2).sum(1))
        codes = d.argmin(dim=1)
        z_q = self.embed[codes].view(shape)

        if self.training:
            with torch.no_grad():
                one_hot = F.one_hot(codes, self.n_codes).float()
                self.cluster_size.mul_(self.decay).add_(
                    one_hot.sum(0), alpha=1 - self.decay)
                self.embed_avg.mul_(self.decay).add_(
                    one_hot.t() @ flat, alpha=1 - self.decay)
                n = self.cluster_size.sum()
                smoothed = (self.cluster_size + self.eps) / (
                    n + self.n_codes * self.eps) * n
                self.embed.copy_(self.embed_avg / smoothed.unsqueeze(1))

        loss = self.commitment * F.mse_loss(z, z_q.detach())
        z_q_st = z + (z_q - z).detach()
        return z_q_st, codes.view(shape[:-1]), loss, z_q
