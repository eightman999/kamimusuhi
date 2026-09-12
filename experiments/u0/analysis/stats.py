"""Small descriptive-statistics helpers (mean/sd/median/CI, Cohen's d)."""

from __future__ import annotations

import numpy as np


def describe(vals) -> dict:
    a = np.asarray([v for v in vals if v is not None], dtype=float)
    a = a[~np.isnan(a)]
    if len(a) == 0:
        return {"n": 0, "mean": float("nan"), "sd": float("nan"),
                "median": float("nan"), "ci95": float("nan")}
    sd = float(a.std(ddof=1)) if len(a) > 1 else 0.0
    return {"n": int(len(a)), "mean": float(a.mean()), "sd": sd,
            "median": float(np.median(a)),
            "ci95": float(1.96 * sd / np.sqrt(len(a))) if len(a) else 0.0}


def cohens_d(a, b) -> float:
    a = np.asarray(a, dtype=float)
    b = np.asarray(b, dtype=float)
    a = a[~np.isnan(a)]
    b = b[~np.isnan(b)]
    if len(a) < 2 or len(b) < 2:
        return float("nan")
    sp = np.sqrt((a.var(ddof=1) + b.var(ddof=1)) / 2)
    return float((a.mean() - b.mean()) / (sp + 1e-12))


def sign_consistent(vals, direction: str = "negative") -> bool:
    """All non-nan values share the expected sign."""
    a = np.asarray([v for v in vals if v is not None], dtype=float)
    a = a[~np.isnan(a)]
    if len(a) == 0:
        return False
    return bool(np.all(a < 0)) if direction == "negative" \
        else bool(np.all(a > 0))
