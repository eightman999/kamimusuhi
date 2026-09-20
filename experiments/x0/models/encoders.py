"""Torch models for X0.

Three trainable kinds (``ModelSpec.kind``):

  binder     per-modality encoders into a SHARED embedding space; trained
             with windowed multi-positive InfoNCE (+ optional soft
             cross-modal reconstruction decoders).  Never sees pair labels:
             positives are "tokens of another modality within the lag
             window" — a set that contains the true partner plus random
             contaminants, so only the consistent content mapping is
             reinforced.
  bottleneck per-modality encoders -> shared z -> per-modality decoders;
             trained with within-modality AE loss plus min-over-window
             cross-modal reconstruction (reconstruction-driven matching).
  indep_ae   per-modality autoencoders; one private space per modality.
             Control: cross-modal comparison should be impossible.

Encoders take CONTENT FEATURES ONLY — never emission times — so a
timestamp/position shortcut is impossible by construction.
"""

from __future__ import annotations

from typing import Dict, List

import torch
from torch import nn


def _mlp(d_in: int, d_hidden: int, d_out: int) -> nn.Sequential:
    return nn.Sequential(nn.Linear(d_in, d_hidden), nn.ReLU(),
                         nn.Linear(d_hidden, d_out))


def pair_key(m: str, n: str) -> str:
    return f"{m}->{n}"


class Binder(nn.Module):
    def __init__(self, dims: Dict[str, int], d_z: int = 32, hidden: int = 64,
                 decoders: bool = True):
        super().__init__()
        self.dims = dict(dims)
        self.d_z = d_z
        self.encoders = nn.ModuleDict(
            {m: _mlp(d, hidden, d_z) for m, d in dims.items()})
        self.decoders = nn.ModuleDict(
            {pair_key(m, n): _mlp(d_z, hidden, dims[n])
             for m in dims for n in dims if m != n}) if decoders \
            else nn.ModuleDict()

    def embed(self, modality: str, x: torch.Tensor) -> torch.Tensor:
        z = self.encoders[modality](x)
        return nn.functional.normalize(z, dim=-1)

    def decode(self, m: str, n: str, z: torch.Tensor) -> torch.Tensor:
        return self.decoders[pair_key(m, n)](z)


class Bottleneck(nn.Module):
    """Shared latent z; each modality has an encoder and a decoder.
    Cross-modal prediction m->n = D_n(E_m(x))."""

    def __init__(self, dims: Dict[str, int], d_z: int = 32, hidden: int = 64):
        super().__init__()
        self.dims = dict(dims)
        self.d_z = d_z
        self.encoders = nn.ModuleDict(
            {m: _mlp(d, hidden, d_z) for m, d in dims.items()})
        self.decoders = nn.ModuleDict(
            {m: _mlp(d_z, hidden, d) for m, d in dims.items()})

    def embed(self, modality: str, x: torch.Tensor) -> torch.Tensor:
        z = self.encoders[modality](x)
        return nn.functional.normalize(z, dim=-1)

    def decode_into(self, modality: str, z: torch.Tensor) -> torch.Tensor:
        return self.decoders[modality](z)


class IndepAE(nn.Module):
    """Independent per-modality autoencoders — private latent spaces."""

    def __init__(self, dims: Dict[str, int], d_z: int = 32, hidden: int = 64):
        super().__init__()
        self.dims = dict(dims)
        self.d_z = d_z
        self.encoders = nn.ModuleDict(
            {m: _mlp(d, hidden, d_z) for m, d in dims.items()})
        self.decoders = nn.ModuleDict(
            {m: _mlp(d_z, hidden, d) for m, d in dims.items()})

    def embed(self, modality: str, x: torch.Tensor) -> torch.Tensor:
        z = self.encoders[modality](x)
        return nn.functional.normalize(z, dim=-1)

    def decode_into(self, modality: str, z: torch.Tensor) -> torch.Tensor:
        return self.decoders[modality](z)


KINDS = {"binder": Binder, "bottleneck": Bottleneck, "indep_ae": IndepAE}


def make_model(spec, dims: Dict[str, int]) -> nn.Module:
    kind = spec.kind
    if kind == "binder":
        return Binder(dims, spec.d_z, spec.hidden, decoders=True)
    if kind in ("bottleneck", "indep_ae"):
        return KINDS[kind](dims, spec.d_z, spec.hidden)
    raise ValueError(f"unknown model kind {kind!r}")
