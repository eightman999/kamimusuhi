"""H0 evaluation metrics (spec section 12).

Primary:   homeostatic_error, survival_time, stable_fraction
Secondary: action_entropy, state_action_mi, switching_rate, recovery_time
"""

from __future__ import annotations

from typing import Dict, List, Optional

import numpy as np

from ..env import dynamics as dyn


def action_entropy(actions: np.ndarray) -> float:
    """Normalized entropy of the action distribution (0 = collapsed, 1 = uniform)."""
    counts = np.bincount(actions, minlength=dyn.N_ACTIONS).astype(np.float64)
    p = counts / max(counts.sum(), 1)
    p = p[p > 0]
    h = -np.sum(p * np.log(p))
    return float(h / np.log(dyn.N_ACTIONS))


def switching_rate(actions: np.ndarray) -> float:
    """Fraction of adjacent steps where the action changed."""
    if len(actions) < 2:
        return 0.0
    return float(np.mean(actions[1:] != actions[:-1]))


def state_action_mi(internals: np.ndarray, actions: np.ndarray, bins: int = 4) -> Dict[str, float]:
    """Mutual information between each (binned) internal variable and action.

    internals: (T, N_INTERNAL), actions: (T,). Returns per-variable MI in bits.
    """
    T = min(len(internals), len(actions))
    actions = actions[:T]
    out = {}
    for i, name in enumerate(dyn.INTERNAL_NAMES):
        x = np.clip(internals[:T, i] * bins, 0, bins - 1).astype(int)
        joint = np.zeros((bins, dyn.N_ACTIONS))
        for xi, ai in zip(x, actions):
            joint[xi, ai] += 1
        joint /= max(joint.sum(), 1)
        px = joint.sum(axis=1, keepdims=True)
        pa = joint.sum(axis=0, keepdims=True)
        with np.errstate(divide="ignore", invalid="ignore"):
            term = joint * np.log2(joint / (px @ pa))
        mi = float(np.nansum(term))
        out[name] = mi
    return out


def conditional_action_shift(internals: np.ndarray, actions: np.ndarray) -> Dict[str, float]:
    """Max change in any action's frequency when an internal var is low vs high.

    For each internal variable, split steps at its median and compute the
    largest per-action frequency difference between the two halves. Robust to
    narrow-band states where MI is limited by low marginal entropy.
    """
    T = min(len(internals), len(actions))
    actions = actions[:T]
    out = {}
    for i, name in enumerate(dyn.INTERNAL_NAMES):
        x = internals[:T, i]
        med = np.median(x)
        lo_mask, hi_mask = x <= med, x > med
        if lo_mask.sum() < 5 or hi_mask.sum() < 5:
            out[name] = 0.0
            continue
        shift = 0.0
        for a in range(dyn.N_ACTIONS):
            p_lo = np.mean(actions[lo_mask] == a)
            p_hi = np.mean(actions[hi_mask] == a)
            shift = max(shift, abs(p_lo - p_hi))
        out[name] = float(shift)
    return out


def recovery_time(stable: np.ndarray) -> float:
    """Mean number of steps to re-enter the stable region after leaving it."""
    times: List[int] = []
    cur = 0
    in_unstable = False
    for s in stable:
        if s:
            if in_unstable:
                times.append(cur)
            in_unstable = False
            cur = 0
        else:
            in_unstable = True
            cur += 1
    if not times:
        return 0.0 if not in_unstable else float("nan")
    return float(np.mean(times))


def episode_metrics(
    internals: np.ndarray,
    actions: np.ndarray,
    rewards: np.ndarray,
    survived_steps: int,
    episode_length: int,
    death_cause: Optional[str] = None,
) -> Dict:
    """Compute all metrics for one episode.

    internals: (T, N_INTERNAL) recorded *before* each step.
    """
    errors = np.array([dyn.homeostatic_error(s) for s in internals])
    stable = np.array([dyn.stable_mask(s) for s in internals])
    mi = state_action_mi(internals, actions)
    # Full-episode error: steps after death count at the final (death-state)
    # error, so a policy cannot look good by dying early in a good state.
    if len(errors) and survived_steps < episode_length:
        full = np.concatenate(
            [errors, np.full(episode_length - survived_steps, errors[-1])]
        )
    else:
        full = errors
    return {
        "homeostatic_error": float(np.mean(errors)) if len(errors) else float("nan"),
        "homeostatic_error_full": float(np.mean(full)) if len(full) else float("nan"),
        "survival_time": int(survived_steps),
        "survival_fraction": float(survived_steps / episode_length),
        "died": death_cause is not None,
        "death_cause": death_cause,
        "stable_fraction": float(np.mean(stable)) if len(stable) else 0.0,
        "action_entropy": action_entropy(actions),
        "switching_rate": switching_rate(actions),
        "recovery_time": recovery_time(stable),
        "state_action_mi": mi,
        "action_shift": conditional_action_shift(internals, actions),
        "total_reward": float(np.sum(rewards)),
    }


def aggregate(episodes: List[Dict]) -> Dict:
    """Aggregate per-episode metric dicts into mean/std summaries."""
    keys = [
        "homeostatic_error",
        "homeostatic_error_full",
        "survival_time",
        "survival_fraction",
        "stable_fraction",
        "action_entropy",
        "switching_rate",
        "recovery_time",
        "total_reward",
    ]
    out: Dict = {"n_episodes": len(episodes)}
    for k in keys:
        vals = np.array([ep[k] for ep in episodes], dtype=np.float64)
        vals = vals[~np.isnan(vals)]
        if len(vals):
            out[k] = {"mean": float(np.mean(vals)), "std": float(np.std(vals))}
        else:
            out[k] = {"mean": float("nan"), "std": float("nan")}
    out["death_rate"] = float(np.mean([ep["died"] for ep in episodes]))
    mi_names = dyn.INTERNAL_NAMES
    out["state_action_mi"] = {
        n: float(np.mean([ep["state_action_mi"][n] for ep in episodes]))
        for n in mi_names
    }
    out["action_shift"] = {
        n: float(np.mean([ep["action_shift"][n] for ep in episodes]))
        for n in mi_names
    }
    return out
