"""Sensory modality transforms for X0.

A *cause* is a latent code ``z`` (concatenated one-hot factor codes).  Each
modality is a different fixed synthetic transform of ``z`` — distinct
functional families so that no raw-feature comparison across modalities can
succeed:

    vis   tanh projection      x_i = tanh(g * (W z + b)_i)
    aud   spectral mixture     x_i = sin((U z)_i) + 0.5 sin(2 (U z)_i)
    temp  RBF thermometer      x_i = exp(-||z - c_i||^2 / (2 s^2))
    mot   rectified projection x_i = relu((W z + b)_i)

Transforms are deterministic given the construction seed; observation noise
is added by the environment, not here.  ``make_transforms`` accepts an
``alt`` seed so evaluation can instantiate a *new* appearance mapping
(sensor-transform OOD test X-C4) without touching the training transforms.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Callable, Dict, Sequence

import numpy as np


@dataclass
class ModalityTransform:
    """Deterministic map from cause latent z (dim z_dim) to features."""

    name: str
    kind: str
    out_dim: int
    W: np.ndarray          # (out_dim, z_dim) linear / spectral / relu matrix
    b: np.ndarray          # (out_dim,) bias
    gain: float = 1.0
    rbf_sigma: float = 1.5

    def __call__(self, z: np.ndarray) -> np.ndarray:
        """z: (..., z_dim) -> features (..., out_dim)."""
        z = np.asarray(z, dtype=np.float64)
        u = z @ self.W.T + self.b
        if self.kind == "tanh":
            return np.tanh(self.gain * u)
        if self.kind == "spectral":
            return np.sin(u) + 0.5 * np.sin(2.0 * u)
        if self.kind == "rbf":
            # centers are stored in W; return exp(-||z - c_i||^2 / 2s^2)
            zc = np.expand_dims(z, axis=-2)                     # (..., 1, z_dim)
            cc = np.expand_dims(self.W, axis=0)                 # (1, out_dim, z_dim)
            d2 = ((zc - cc) ** 2).sum(axis=-1)                  # (..., out_dim)
            return np.exp(-d2 / (2.0 * self.rbf_sigma ** 2))
        if self.kind == "relu":
            return np.maximum(u, 0.0)
        raise ValueError(f"unknown transform kind {self.kind!r}")


def _one(rng: np.random.Generator, name: str, kind: str, out_dim: int,
         z_dim: int) -> ModalityTransform:
    W = rng.normal(0.0, np.sqrt(2.0 / z_dim), size=(out_dim, z_dim))
    b = rng.normal(0.0, 0.5, size=out_dim)
    if kind == "rbf":
        # W holds bump centers: spread them across the z-hypercube
        W = rng.normal(0.0, 1.0, size=(out_dim, z_dim))
        b = np.zeros(out_dim)
        return ModalityTransform(name, kind, out_dim, W, b, rbf_sigma=1.5)
    gain = {"tanh": 2.5, "spectral": 1.8, "relu": 1.5}.get(kind, 1.0)
    return ModalityTransform(name, kind, out_dim, W, b, gain=gain)


DEFAULT_KINDS = {"vis": ("tanh", 16), "aud": ("spectral", 16),
                 "temp": ("rbf", 8), "mot": ("relu", 12)}


def make_transforms(names: Sequence[str], z_dim: int, seed: int,
                    alt: bool = False) -> Dict[str, ModalityTransform]:
    """Build the transform set.  ``alt=True`` draws a different seed-offset
    family used only for the X-C4 sensor-transform OOD evaluation."""
    rng = np.random.default_rng(seed + (777_000 if alt else 0))
    out: Dict[str, ModalityTransform] = {}
    for i, name in enumerate(names):
        kind, dim = DEFAULT_KINDS[name]
        sub = np.random.default_rng(int(rng.integers(2**31)) + i)
        out[name] = _one(sub, name, kind, dim, z_dim)
    return out
