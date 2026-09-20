"""Linear probes for X0 (eval-time only; labels never touch training).

Ridge-regression classifier: fit W = (X'X + lam I)^-1 X'Y with Y one-hot,
predict argmax(X @ W).  ``cross_modal_matrix`` trains a probe on one
modality's embeddings and tests on every modality — the binding signature:
a genuinely shared space transfers across encoders.
"""

from __future__ import annotations

from typing import Dict

import numpy as np


def fit_probe(X: np.ndarray, y: np.ndarray, n_classes: int,
              lam: float = 1.0) -> np.ndarray:
    X = np.asarray(X, float)
    Y = np.zeros((len(X), n_classes))
    Y[np.arange(len(X)), y] = 1.0
    Xb = np.concatenate([X, np.ones((len(X), 1))], axis=1)
    reg = lam * np.eye(Xb.shape[1])
    reg[-1, -1] = 0.0                          # don't penalize the bias
    W = np.linalg.solve(Xb.T @ Xb + reg, Xb.T @ Y)
    return W


def probe_acc(W: np.ndarray, X: np.ndarray, y: np.ndarray) -> float:
    Xb = np.concatenate([np.asarray(X, float),
                         np.ones((len(X), 1))], axis=1)
    return float((Xb @ W).argmax(1).__eq__(y).mean())


def cross_modal_matrix(embeds: Dict[str, np.ndarray],
                       labels: Dict[str, np.ndarray], n_classes: int,
                       lam: float = 1.0,
                       train_frac: float = 0.6,
                       seed: int = 0) -> Dict[str, Dict[str, float]]:
    """embeds/labels: modality -> (n_m, d) / (n_m,) arrays.
    Returns acc[train_mod][test_mod].  Train and test tokens are split
    per modality so no token appears in both."""
    rng = np.random.default_rng(seed)
    out: Dict[str, Dict[str, float]] = {}
    splits = {}
    for m, X in embeds.items():
        n = len(X)
        perm = rng.permutation(n)
        k = int(n * train_frac)
        splits[m] = (perm[:k], perm[k:])
    for m in embeds:
        tr, _ = splits[m]
        if len(tr) == 0:
            continue
        W = fit_probe(embeds[m][tr], labels[m][tr], n_classes, lam)
        out[m] = {}
        for n in embeds:
            _, te = splits[n]
            if len(te) == 0:
                out[m][n] = float("nan")
                continue
            out[m][n] = probe_acc(W, embeds[n][te], labels[n][te])
    return out
