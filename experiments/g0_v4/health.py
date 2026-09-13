"""Representation-collapse health metrics (v4 spec §19).

For every trained / untrained rep we record:

  * per-dim variance (min / mean)         — dead/collapsed dims
  * effective rank of the latent covariance (participation ratio)
  * pairwise cosine distribution (mean / std / |mean|)
  * feature-covariance off-diagonal RMS    — redundancy

A collapsed run (effective rank ~1, near-constant latents) is marked
in the report, not silently averaged into conclusions.
"""

from __future__ import annotations

import numpy as np

EPS = 1e-12


def representation_health(Z: np.ndarray, max_points: int = 4096,
                          seed: int = 0) -> dict:
    """Z: (E,T,H) or (N,H) latents -> health dict."""
    X = np.asarray(Z, dtype=np.float64).reshape(-1, Z.shape[-1])
    if X.shape[0] > max_points:
        idx = np.random.default_rng(seed).choice(
            X.shape[0], max_points, replace=False)
        X = X[idx]
    n, d = X.shape
    if n < 4:
        return {"collapsed": True, "n": int(n)}

    var = X.var(0)
    Xc = X - X.mean(0)
    cov = (Xc.T @ Xc) / max(n - 1, 1)
    eig = np.linalg.eigvalsh(cov)
    eig = np.clip(eig, 0.0, None)
    s1, s2 = eig.sum(), (eig ** 2).sum()
    eff_rank = float(s1 * s1 / s2) if s2 > EPS else 0.0

    # off-diagonal covariance RMS (redundancy)
    off = cov - np.diag(np.diag(cov))
    off_rms = float(np.sqrt((off ** 2).mean()))

    # pairwise cosine on a subsample
    m = min(n, 1024)
    Xs = X[:m]
    norms = np.linalg.norm(Xs, axis=1, keepdims=True)
    Xn = Xs / np.maximum(norms, EPS)
    sim = Xn @ Xn.T
    iu = np.triu_indices(m, k=1)
    cos = sim[iu]

    # collapse heuristic: rank ~1 or mean per-dim variance ~0
    collapsed = bool(eff_rank < 1.5 or var.mean() < 1e-8)
    return {
        "n": int(n), "dim": int(d),
        "var_min": float(var.min()), "var_mean": float(var.mean()),
        "effective_rank": eff_rank,
        "cov_offdiag_rms": off_rms,
        "cos_mean": float(cos.mean()), "cos_std": float(cos.std()),
        "cos_abs_mean": float(np.abs(cos).mean()),
        "norm_mean": float(norms.mean()),
        "collapsed": collapsed,
    }
