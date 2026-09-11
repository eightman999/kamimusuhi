"""Evaluation metrics for X0 (numpy).

Token-level primitives used by evaluate.py:

  retrieval(sim, partner)      top-1 / MRR over candidate tokens
  matching_accuracy            optimal assignment between two modalities'
                               token sets (exact for <=8 events)
  false_binding                conflict-scene stats: is the argmax partner
                               a *co-timed different-cause* token?
  summarise / cohens_d / sign_test   multi-seed statistics
"""

from __future__ import annotations

import itertools
from typing import Dict, Optional

import numpy as np


def retrieval(sim: np.ndarray, partner: np.ndarray) -> Dict[str, float]:
    """sim: (nq, nc) similarity; partner: (nq,) index of same-event token or
    -1.  Returns top1 accuracy and MRR over queries that have a partner."""
    valid = partner >= 0
    n = int(valid.sum())
    if n == 0:
        return {"top1": float("nan"), "mrr": float("nan"), "n": 0}
    order = np.argsort(-sim, axis=1)               # descending
    ranks = np.empty_like(order)
    rows = np.arange(sim.shape[0])[:, None]
    ranks[rows, order] = np.arange(sim.shape[1])[None, :]
    pr = ranks[np.arange(sim.shape[0]), np.clip(partner, 0, sim.shape[1] - 1)]
    pr = pr[valid]
    return {"top1": float((pr == 0).mean()), "mrr": float((1.0 / (pr + 1)).mean()),
            "n": n}


def _best_assignment(S: np.ndarray) -> np.ndarray:
    """Return col index per row maximizing sum; exact for <=8 rows."""
    n_r, n_c = S.shape
    if n_r == 0 or n_c == 0:
        return np.full(n_r, -1)
    if n_r <= 8 and n_c <= 8:
        best, best_v = None, -np.inf
        for perm in itertools.permutations(range(n_c)):
            if len(perm) < n_r:
                continue
            v = sum(S[i, perm[i]] for i in range(n_r))
            if v > best_v:
                best_v, best = v, perm
        return np.asarray(best[:n_r])
    # greedy fallback
    S = S.copy()
    out = np.full(n_r, -1)
    used = np.zeros(n_c, bool)
    for _ in range(n_r):
        i, j = np.unravel_index(np.argmax(S), S.shape)
        out[i] = j
        used[j] = True
        S[i, :] = -np.inf
        S[:, j] = -np.inf
    return out


def matching_accuracy(sim: np.ndarray, ev_a: np.ndarray,
                      ev_b: np.ndarray) -> float:
    """Fraction of events (present in both modalities) that the optimal
    assignment pairs correctly.  sim is token-level; ev_* map tokens to
    event ids."""
    ev_to_i = {int(e): i for i, e in enumerate(ev_a)}
    ev_to_j = {int(e): j for j, e in enumerate(ev_b)}
    common = sorted(set(ev_to_i) & set(ev_to_j))
    if not common:
        return float("nan")
    S = np.asarray([[sim[ev_to_i[e], ev_to_j[f]] for f in common]
                    for e in common])
    assign = _best_assignment(S)
    correct = sum(int(assign[r] >= 0 and common[assign[r]] == common[r])
                  for r in range(len(common)))
    return correct / len(common)


def false_binding(sim: np.ndarray, times_a: np.ndarray, times_b: np.ndarray,
                  ev_a: np.ndarray, ev_b: np.ndarray,
                  partner: np.ndarray) -> Dict[str, float]:
    """Conflict-scene statistics.

    cotimed_wrong : fraction of valid queries whose top-1 candidate is a
                    different-event token at |Δt| <= |Δt_true| (the timing
                    shortcut answer)
    top1          : fraction whose top-1 is the true partner
    margin        : mean s(true) - s(best cotimed wrong), over queries that
                    have a cotimed wrong candidate
    n_distracted  : queries having >=1 cotimed (Δt<=Δt_true) wrong candidate
    """
    top_wrong = 0
    top_correct = 0
    valid = int((partner >= 0).sum())
    margins = []
    n_distracted = 0
    for i in range(sim.shape[0]):
        if partner[i] < 0:
            continue
        j_true = int(partner[i])
        dt_true = abs(int(times_a[i]) - int(times_b[j_true]))
        j_best = int(np.argmax(sim[i]))
        if j_best == j_true:
            top_correct += 1
        else:
            dt_best = abs(int(times_a[i]) - int(times_b[j_best]))
            if dt_best <= dt_true and ev_b[j_best] != ev_a[i]:
                top_wrong += 1
        wrong = [j for j in range(sim.shape[1])
                 if ev_b[j] != ev_a[i]
                 and abs(int(times_a[i]) - int(times_b[j])) <= dt_true]
        if wrong:
            n_distracted += 1
            margins.append(sim[i, j_true] - max(sim[i, j] for j in wrong))
    return {
        "cotimed_wrong": float(top_wrong / max(1, valid)),
        "top1": float(top_correct / max(1, valid)) if valid else float("nan"),
        "margin": float(np.mean(margins)) if margins else float("nan"),
        "n_distracted": n_distracted, "n": valid}


# ----------------------------------------------------------------------
# multi-seed statistics
# ----------------------------------------------------------------------
def summarise(values, n_boot: int = 2000, seed: int = 0) -> Dict[str, float]:
    v = np.asarray([x for x in values if x is not None
                    and not (isinstance(x, float) and np.isnan(x))], float)
    if len(v) == 0:
        return {"n": 0, "mean": float("nan"), "sd": float("nan"),
                "median": float("nan"), "ci95": [float("nan")] * 2}
    rng = np.random.default_rng(seed)
    boot = rng.choice(v, (n_boot, len(v))).mean(1) if len(v) > 1 else v
    return {"n": int(len(v)), "mean": float(v.mean()),
            "sd": float(v.std(ddof=1)) if len(v) > 1 else 0.0,
            "median": float(np.median(v)),
            "ci95": [float(np.percentile(boot, 2.5)),
                     float(np.percentile(boot, 97.5))]}


def cohens_d(a, b) -> float:
    a, b = np.asarray(a, float), np.asarray(b, float)
    na, nb = len(a), len(b)
    if na < 2 or nb < 2:
        return float("nan")
    sp = np.sqrt(((na - 1) * a.var(ddof=1) + (nb - 1) * b.var(ddof=1))
                 / (na + nb - 2))
    return float((a.mean() - b.mean()) / (sp + 1e-12))


def sign_test_p(a, b, n_boot: int = 20000, seed: int = 0) -> Dict[str, float]:
    """Two-sided sign test on paired differences via binomial sampling."""
    d = np.asarray(a, float) - np.asarray(b, float)
    d = d[d != 0]
    n = len(d)
    if n == 0:
        return {"wins": 0, "n": 0, "p": float("nan")}
    wins = int((d > 0).sum())
    rng = np.random.default_rng(seed)
    p_one = float((rng.binomial(n, 0.5, n_boot) >= wins).mean())
    return {"wins": wins, "n": n, "p": float(min(1.0, 2 * min(p_one, 1 - p_one)))}
