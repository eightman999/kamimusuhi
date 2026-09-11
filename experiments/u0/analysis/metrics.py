"""U0 calibration / selective-prediction metrics.

All functions are pure numpy so tests can drive them with synthetic data.

Conventions:
    probs    (N, K) predicted class probabilities at the answered step
    y        (N,)   true labels
    conf     (N,)   scalar confidence used for ranking / ECE
    correct  (N,)   1{argmax probs == y} for answered episodes

Selective-prediction metrics are computed on the *answered* subset only;
coverage and abstention rate are always reported alongside.
"""

from __future__ import annotations

import numpy as np


def accuracy(correct: np.ndarray) -> float:
    return float(np.mean(correct)) if len(correct) else float("nan")


def nll(probs: np.ndarray, y: np.ndarray) -> float:
    p = np.clip(probs[np.arange(len(y)), y], 1e-12, 1.0)
    return float(-np.log(p).mean()) if len(y) else float("nan")


def brier(probs: np.ndarray, y: np.ndarray) -> float:
    onehot = np.zeros_like(probs)
    onehot[np.arange(len(y)), y] = 1.0
    return float(((probs - onehot) ** 2).sum(axis=1).mean()) \
        if len(y) else float("nan")


def ece(conf: np.ndarray, correct: np.ndarray, n_bins: int = 10) -> float:
    """Expected calibration error over equal-width confidence bins."""
    if len(conf) == 0:
        return float("nan")
    edges = np.linspace(0.0, 1.0, n_bins + 1)
    idx = np.clip(np.digitize(conf, edges[1:-1]), 0, n_bins - 1)
    out = 0.0
    for b in range(n_bins):
        m = idx == b
        if m.any():
            out += m.mean() * abs(correct[m].mean() - conf[m].mean())
    return float(out)


def ece_by_prob(probs: np.ndarray, y: np.ndarray, n_bins: int = 10) -> float:
    """Standard max-prob ECE."""
    if len(y) == 0:
        return float("nan")
    conf = probs.max(axis=1)
    correct = (probs.argmax(axis=1) == y).astype(float)
    return ece(conf, correct, n_bins)


def risk_coverage(conf: np.ndarray, correct: np.ndarray,
                  n_points: int = 21) -> dict:
    """Selective risk as a function of coverage (sort by conf desc)."""
    n = len(conf)
    if n == 0:
        return {"coverage": [], "risk": [], "aurc": float("nan")}
    order = np.argsort(-conf)
    corr_sorted = correct[order]
    coverages, risks = [], []
    for frac in np.linspace(0.0, 1.0, n_points):
        k = max(1, int(round(frac * n)))
        coverages.append(k / n)
        risks.append(1.0 - float(corr_sorted[:k].mean()))
    aurc = float(np.trapezoid(risks, coverages))
    return {"coverage": [float(c) for c in coverages],
            "risk": [float(r) for r in risks], "aurc": aurc}


def selective_accuracy(conf: np.ndarray, correct: np.ndarray,
                       coverage: float = 0.6) -> float:
    """Accuracy on the top-`coverage` most confident answered episodes."""
    n = len(conf)
    if n == 0:
        return float("nan")
    k = max(1, int(round(coverage * n)))
    order = np.argsort(-conf)
    return float(correct[order[:k]].mean())


def error_detection_auroc(conf: np.ndarray, correct: np.ndarray) -> float:
    """AUROC of confidence as a detector of *correctness* (1 = perfect)."""
    n = len(conf)
    if n == 0 or correct.sum() in (0, n):
        return float("nan")
    order = np.argsort(conf)
    ranks = np.empty(n)
    ranks[order] = np.arange(1, n + 1)
    # average ranks for ties
    _, inv, counts = np.unique(conf, return_inverse=True, return_counts=True)
    mean_ranks = np.zeros(len(counts))
    np.add.at(mean_ranks, inv, ranks)
    mean_ranks /= counts
    ranks = mean_ranks[inv]
    pos = correct.astype(bool)
    n_pos, n_neg = pos.sum(), (~pos).sum()
    return float((ranks[pos].sum() - n_pos * (n_pos + 1) / 2) / (n_pos * n_neg))


def episode_metrics(records: list[dict], n_bins: int = 10) -> dict:
    """Aggregate per-episode eval records into the headline metrics."""
    n = len(records)
    answered = [r for r in records if r["outcome"] == "answer"]
    probs = np.asarray([r["probs"] for r in answered], dtype=np.float64) \
        if answered else np.zeros((0, 4))
    y = np.asarray([r["y"] for r in answered], dtype=np.int64)
    correct = np.asarray([r["correct"] for r in answered], dtype=np.float64)
    conf = np.asarray([r["conf"] for r in answered], dtype=np.float64)
    rc = risk_coverage(conf, correct)
    out = {
        "episodes": n,
        "mean_reward": float(np.mean([r["reward"] for r in records])),
        "answer_rate": len(answered) / max(1, n),
        "abstain_rate": float(np.mean(
            [r["outcome"] in ("abstain", "forced_abstain") for r in records])),
        "mean_obs": float(np.mean([r["n_obs"] for r in records])),
        "mean_observe_cost": float(np.mean(
            [r["observe_cost"] for r in records])),
        "accuracy": accuracy(correct),
        "coverage": len(answered) / max(1, n),
        "nll": nll(probs, y),
        "brier": brier(probs, y),
        "ece_conf": ece(conf, correct, n_bins),
        "ece_maxprob": ece_by_prob(probs, y, n_bins),
        "aurc": rc["aurc"],
        "sel_acc_60": selective_accuracy(conf, correct, 0.6),
        "err_auroc": error_detection_auroc(conf, correct),
        "action_counts": np.zeros(6).tolist(),
    }
    counts = np.zeros(6)
    for r in records:
        counts += np.asarray(r["action_counts"][:6])
    out["action_counts"] = counts.tolist()
    return out


def per_regime(records: list[dict]) -> dict:
    """Metrics broken down by ground-truth regime (instrumentation)."""
    out = {}
    for reg in sorted({r["regime"] for r in records}):
        sub = [r for r in records if r["regime"] == reg]
        ans = [r for r in sub if r["outcome"] == "answer"]
        out[reg] = {
            "n": len(sub),
            "answer_rate": len(ans) / max(1, len(sub)),
            "abstain_rate": float(np.mean(
                [r["outcome"] in ("abstain", "forced_abstain")
                 for r in sub])),
            "mean_obs": float(np.mean([r["n_obs"] for r in sub])),
            "accuracy": float(np.mean([r["correct"] for r in ans]))
            if ans else float("nan"),
            "mean_reward": float(np.mean([r["reward"] for r in sub])),
            "mean_conf": float(np.mean(
                [r["conf"] for r in sub if r["conf"] is not None]))
            if any(r["conf"] is not None for r in sub) else float("nan"),
        }
    return out
