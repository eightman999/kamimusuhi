"""Seed-level descriptive statistics; no telemetry-point pseudo-replication."""
from __future__ import annotations

import math
import numpy as np


def exact_sign_test(differences, tolerance: float = 1e-12) -> float:
    values = np.asarray(differences, dtype=float)
    values = values[np.abs(values) > tolerance]
    if not len(values):
        return 1.0
    positive = int(np.sum(values > 0))
    tail = min(positive, len(values) - positive)
    return min(1.0, 2 * sum(math.comb(len(values), k) for k in range(tail + 1)) / 2 ** len(values))


def describe(values, *, bootstrap_seed: int = 1847, resamples: int = 20000) -> dict:
    x = np.asarray(values, dtype=np.float64)
    if x.ndim != 1 or not len(x) or not np.isfinite(x).all():
        raise ValueError("statistics require a nonempty finite vector of independent seed means")
    rng = np.random.default_rng(bootstrap_seed)
    means = x[rng.integers(0, len(x), size=(resamples, len(x)))].mean(1)
    return {"n_seeds": len(x), "mean": float(x.mean()),
            "sd": float(x.std(ddof=1)) if len(x) > 1 else None,
            "median": float(np.median(x)), "ci95": np.quantile(means, [.025, .975]).tolist(),
            "unit": "training_seed", "bootstrap_resamples": resamples,
            "bootstrap_seed": bootstrap_seed, "values": x.tolist()}


def paired_comparison(left, right) -> dict:
    a, b = np.asarray(left, dtype=float), np.asarray(right, dtype=float)
    if a.shape != b.shape:
        raise ValueError("paired arrays must have identical seed order and length")
    result = describe(a - b)
    result["mean_difference"] = result["mean"]
    result["exact_sign_p"] = exact_sign_test(a - b)
    result["non_tied_seeds"] = int(np.sum(np.abs(a - b) > 1e-12))
    result["pass"] = bool(len(a) >= 8 and result["mean"] > 0 and
                          result["ci95"][0] > 0 and result["exact_sign_p"] <= .05)
    result["multiplicity"] = "unadjusted; prespecified primary comparisons require conjunction"
    return result
