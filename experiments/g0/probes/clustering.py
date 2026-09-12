"""Clustering utilities (no sklearn): k-means, NMI, purity, matching."""

from __future__ import annotations

import itertools
from typing import Optional, Tuple

import numpy as np


def kmeans(X: np.ndarray, k: int, seed: int = 0, iters: int = 60,
           n_init: int = 3) -> Tuple[np.ndarray, np.ndarray]:
    """Lloyd's k-means with k-means++ init. Returns (centers, assign)."""
    rng = np.random.default_rng(seed)
    best_inertia, best = np.inf, None
    for _ in range(n_init):
        # k-means++ seeding
        c = [X[rng.integers(len(X))]]
        for _ in range(k - 1):
            d2 = np.min(((X[:, None, :] - np.stack(c)[None]) ** 2)
                        .sum(-1), axis=1)
            s = float(d2.sum())
            if s <= 1e-12:
                # all points coincide with chosen centers (e.g. heavily
                # quantized latents) — fall back to uniform pick
                c.append(X[rng.integers(len(X))])
            else:
                c.append(X[rng.choice(len(X), p=d2 / s)])
        centers = np.stack(c)
        assign = np.zeros(len(X), dtype=np.int64)
        for _ in range(iters):
            d = ((X[:, None, :] - centers[None]) ** 2).sum(-1)
            new_assign = d.argmin(1)
            if (new_assign == assign).all():
                break
            assign = new_assign
            for j in range(k):
                m = assign == j
                if m.any():
                    centers[j] = X[m].mean(0)
                else:
                    centers[j] = X[rng.integers(len(X))]
        inertia = float(((X - centers[assign]) ** 2).sum())
        if inertia < best_inertia:
            best_inertia, best = inertia, (centers.copy(), assign.copy())
    return best


def contingency(a: np.ndarray, b: np.ndarray) -> np.ndarray:
    a, b = np.asarray(a).ravel(), np.asarray(b).ravel()
    ca = {v: i for i, v in enumerate(np.unique(a))}
    cb = {v: i for i, v in enumerate(np.unique(b))}
    t = np.zeros((len(ca), len(cb)))
    for x, y in zip(a, b):
        t[ca[x], cb[y]] += 1
    return t


def mutual_info(a: np.ndarray, b: np.ndarray) -> float:
    t = contingency(a, b)
    p = t / t.sum()
    pa, pb = p.sum(1, keepdims=True), p.sum(0, keepdims=True)
    m = (p > 0) & (pa > 0) & (pb > 0)
    return float((p[m] * np.log(p[m] / (pa @ pb)[m])).sum())


def nmi(true: np.ndarray, pred: np.ndarray) -> float:
    """Normalized mutual information (arithmetic mean normalization)."""
    t = contingency(true, pred)
    p = t / t.sum()
    pa, pb = p.sum(1), p.sum(0)
    ha = -(pa[pa > 0] * np.log(pa[pa > 0])).sum()
    hb = -(pb[pb > 0] * np.log(pb[pb > 0])).sum()
    if ha + hb == 0:
        return 1.0
    return float(2.0 * mutual_info(true, pred) / (ha + hb))


def purity(true: np.ndarray, pred: np.ndarray) -> float:
    t = contingency(true, pred)
    return float(t.max(axis=0).sum() / t.sum())


def best_match_acc(centroids_a: np.ndarray, labels_a: np.ndarray,
                   centroids_b: np.ndarray, labels_b: np.ndarray) -> dict:
    """Match cause centroids between two contexts by nearest-neighbor
    assignment (exact search over permutations; n_causes is small).

    centroids_*: (n_present, latent_dim); labels_*: cause id per row.
    Returns acc over causes present in BOTH contexts.
    """
    common = sorted(set(labels_a.tolist()) & set(labels_b.tolist()))
    if len(common) < 2:
        return {"match_acc": float("nan"), "n_common": len(common)}
    ia = [list(labels_a).index(c) for c in common]
    ib = {c: list(labels_b).index(c) for c in common}
    A = centroids_a[ia]                       # (n, H) indexed by common order
    B = np.stack([centroids_b[ib[c]] for c in common])
    # cosine distance matrix between A rows and B rows
    an = A / (np.linalg.norm(A, axis=1, keepdims=True) + 1e-12)
    bn = B / (np.linalg.norm(B, axis=1, keepdims=True) + 1e-12)
    sim = an @ bn.T
    n = len(common)
    best = -np.inf
    best_perm = None
    for perm in itertools.permutations(range(n)):
        s = sum(sim[i, perm[i]] for i in range(n))
        if s > best:
            best, best_perm = s, perm
    # greedy row-wise accuracy under best alignment is simply: fraction
    # of rows whose argmax column equals the identity (cause matched to
    # itself). Report both: optimal-assignment quality and the rate at
    # which the nearest centroid IS the same cause.
    nearest_same = float(np.mean([np.argmax(sim[i]) == i
                                  for i in range(n)]))
    return {"match_acc": nearest_same, "n_common": n,
            "align_score": float(best / n)}


def centroid_margin(centroids_a, labels_a, centroids_b, labels_b) -> dict:
    """same-cause cross-context similarity minus best other-cause sim."""
    common = sorted(set(labels_a.tolist()) & set(labels_b.tolist()))
    if len(common) < 2:
        return {"margin": float("nan")}
    ia = [list(labels_a).index(c) for c in common]
    ib = {c: list(labels_b).index(c) for c in common}
    A = centroids_a[ia]
    B = np.stack([centroids_b[ib[c]] for c in common])
    an = A / (np.linalg.norm(A, axis=1, keepdims=True) + 1e-12)
    bn = B / (np.linalg.norm(B, axis=1, keepdims=True) + 1e-12)
    sim = an @ bn.T
    n = len(common)
    margins = []
    for i in range(n):
        others = [sim[i, j] for j in range(n) if j != i]
        margins.append(float(sim[i, i] - max(others)))
    return {"margin": float(np.mean(margins)),
            "margin_min": float(np.min(margins))}
