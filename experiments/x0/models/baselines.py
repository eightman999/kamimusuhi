"""Non-torch baselines for X0 (numpy only).

Every method exposes::

    pair_sim(m, rec_m, n, rec_n) -> (n_m_tokens, n_n_tokens) float matrix
    embed(m, feats)             -> (n, d) per-token representation (probes)

where ``rec = {"times": (k,), "feats": (k, d_m)}``.

Methods:

  chance    random scores — floor
  timing    -|Δt| — the synchrony heuristic; must fail the shifted-timing
            and false-synchrony controls
  concat    zero-padded union vector — cross-modal cosine is identically 0,
            i.e. raw concatenation cannot bind (documented chance)
  pca       per-modality PCA embedding — no shared space
  cca       CCA fitted on *temporally nearest* pseudo-pairs — the strongest
            timing-driven unsupervised alignment; content-blind
"""

from __future__ import annotations

from typing import Dict, List, Sequence, Tuple

import numpy as np


def cosine_sim(A: np.ndarray, B: np.ndarray) -> np.ndarray:
    a = A / (np.linalg.norm(A, axis=1, keepdims=True) + 1e-9)
    b = B / (np.linalg.norm(B, axis=1, keepdims=True) + 1e-9)
    return a @ b.T


class ChanceMethod:
    name = "chance"

    def __init__(self, seed: int = 0):
        self.rng = np.random.default_rng(seed)

    def pair_sim(self, m, rec_m, n, rec_n):
        return self.rng.random((len(rec_m["times"]), len(rec_n["times"])))

    def embed(self, m, feats):
        return feats


class TimingMethod:
    """Rank candidates by temporal proximity only."""
    name = "timing"

    def pair_sim(self, m, rec_m, n, rec_n):
        dt = (rec_m["times"][:, None].astype(float)
              - rec_n["times"][None, :].astype(float))
        return -np.abs(dt)

    def embed(self, m, feats):
        return feats


class ConcatMethod:
    """Zero-padded union vector: [vis | 0 | 0 | 0] vs [0 | aud | 0 | 0].
    Cross-modal cosine is exactly 0 — raw concat has no binding."""
    name = "concat"

    def __init__(self, modalities: Sequence[str], dims: Dict[str, int]):
        self.mods = list(modalities)
        self.offset = {}
        off = 0
        for m in self.mods:
            self.offset[m] = off
            off += dims[m]
        self.total = off

    def embed(self, m, feats):
        out = np.zeros((len(feats), self.total))
        o = self.offset[m]
        out[:, o:o + feats.shape[1]] = feats
        return out

    def pair_sim(self, m, rec_m, n, rec_n):
        return cosine_sim(self.embed(m, rec_m["feats"]),
                          self.embed(n, rec_n["feats"]))


class PcaMethod:
    name = "pca"

    def __init__(self, d_z: int = 8):
        self.d_z = d_z
        self.bases: Dict[str, np.ndarray] = {}
        self.means: Dict[str, np.ndarray] = {}

    def fit(self, tokens: Dict[str, np.ndarray]) -> "PcaMethod":
        # one shared projection size so cross-modal cosine is defined
        k_eff = min(self.d_z, min(X.shape[1] for X in tokens.values()))
        for m, X in tokens.items():
            mu = X.mean(0)
            Xc = X - mu
            _, _, vt = np.linalg.svd(Xc, full_matrices=False)
            self.bases[m] = vt[:k_eff].T              # (d_m, k_eff)
            self.means[m] = mu
        return self

    def embed(self, m, feats):
        return (feats - self.means[m]) @ self.bases[m]

    def pair_sim(self, m, rec_m, n, rec_n):
        return cosine_sim(self.embed(m, rec_m["feats"]),
                          self.embed(n, rec_n["feats"]))


def _whiten(X: np.ndarray, reg: float = 1e-3):
    Xc = X - X.mean(0)
    C = Xc.T @ Xc / max(1, len(Xc)) + reg * np.eye(X.shape[1])
    w, V = np.linalg.eigh(C)
    return V @ np.diag(1.0 / np.sqrt(np.maximum(w, 1e-9))) , Xc, X.mean(0)


class CcaMethod:
    """Canonical correlation between modalities, fitted on pseudo-pairs
    formed by taking each token's temporally nearest cross-modal token —
    i.e. synchrony used *as* the pairing signal (no ground truth)."""

    name = "cca"

    def __init__(self, d_z: int = 8, window: int = 99):
        self.d_z = d_z
        self.maps: Dict[Tuple[str, str], Tuple[np.ndarray, np.ndarray,
                                               np.ndarray, np.ndarray]] = {}

    def fit(self, scenes, modalities: Sequence[str]) -> "CcaMethod":
        import itertools
        for a, b in itertools.combinations(modalities, 2):
            X, Y = [], []
            for sc in scenes:
                ra, rb = sc.emissions[a], sc.emissions[b]
                if len(ra.times) == 0 or len(rb.times) == 0:
                    continue
                dt = np.abs(ra.times[:, None] - rb.times[None, :])
                j = dt.argmin(1)
                X.append(ra.feats)
                Y.append(rb.feats[j])
            X = np.concatenate(X)
            Y = np.concatenate(Y)
            Wx, Xc, mx = _whiten(X)
            Wy, Yc, my = _whiten(Y)
            Sxy = (Xc.T @ Yc) / max(1, len(Xc))
            M = Wx.T @ Sxy @ Wy
            U, _, Vt = np.linalg.svd(M, full_matrices=False)
            k = min(self.d_z, U.shape[1])
            A = Wx @ U[:, :k]
            B = Wy @ Vt[:k].T
            self.maps[(a, b)] = (A, B, mx, my)
            self.maps[(b, a)] = (B, A, my, mx)
        return self

    def pair_sim(self, m, rec_m, n, rec_n):
        A, B, mx, my = self.maps[(m, n)]
        return cosine_sim((rec_m["feats"] - mx) @ A, (rec_n["feats"] - my) @ B)

    def embed(self, m, feats):
        # per-modality canonical projection into its first fitted pair space
        key = next(k for k in self.maps if k[0] == m)
        A, _, mx, _ = self.maps[key]
        return (feats - mx) @ A
