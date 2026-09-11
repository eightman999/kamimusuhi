"""Multi-seed summary statistics for U0 (mean/std/median/CI/effect size)."""

from __future__ import annotations

import numpy as np


def describe(values, n_boot: int = 2000, seed: int = 0) -> dict:
    v = np.asarray(list(values), dtype=np.float64)
    v = v[~np.isnan(v)]
    if len(v) == 0:
        return {"n": 0, "mean": float("nan"), "sd": float("nan"),
                "median": float("nan"), "ci95": [float("nan"), float("nan")]}
    rng = np.random.default_rng(seed)
    boot = rng.choice(v, (n_boot, len(v))).mean(axis=1)
    return {"n": int(len(v)),
            "mean": float(v.mean()),
            "sd": float(v.std(ddof=1)) if len(v) > 1 else 0.0,
            "median": float(np.median(v)),
            "ci95": [float(np.percentile(boot, 2.5)),
                     float(np.percentile(boot, 97.5))]}


def cohens_d(a, b) -> float:
    """Effect size of mean(a) - mean(b) with pooled SD."""
    a = np.asarray(a, dtype=np.float64)
    b = np.asarray(b, dtype=np.float64)
    a, b = a[~np.isnan(a)], b[~np.isnan(b)]
    if len(a) < 2 or len(b) < 2:
        return float("nan")
    sp = np.sqrt((a.var(ddof=1) * (len(a) - 1) +
                  b.var(ddof=1) * (len(b) - 1)) / (len(a) + len(b) - 2))
    if sp < 1e-12:
        return 0.0
    return float((a.mean() - b.mean()) / sp)


def summarize_by_subject(rows: list[dict], metric: str = "mean_reward") -> dict:
    """rows: list of {'subject': ..., metric: ...} -> per-subject stats."""
    out = {}
    subjects = sorted({r["subject"] for r in rows})
    for s in subjects:
        vals = [r[metric] for r in rows if r["subject"] == s]
        out[s] = describe(vals)
    return out
