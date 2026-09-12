"""P0 evaluation metrics.

Primary:   detection_rate (task-relevant D events caught / decided events)
           mean_cost (observation cost per step)
           info_efficiency = detection_rate / mean_cost
           total_reward
Secondary: per-action fractions (attention allocation), per-channel surprise
           means, per-channel prediction error of the predictor, event counts.
"""

from __future__ import annotations

from typing import Dict, List

import numpy as np

from ..env import dynamics as dyn


def action_entropy(actions: np.ndarray) -> float:
    counts = np.bincount(actions, minlength=dyn.N_ACTIONS).astype(np.float64)
    p = counts / max(counts.sum(), 1)
    p = p[p > 0]
    return float(-np.sum(p * np.log(p)) / np.log(dyn.N_ACTIONS))


def episode_metrics(
    actions: np.ndarray,
    rewards: np.ndarray,
    costs: np.ndarray,
    n_detected: int,
    n_missed: int,
    surprise_mean: np.ndarray,      # (C,) mean e+d per channel over episode
    pred_err_mean: np.ndarray,      # (C,) mean raw |xhat - v| at obs times
    event_active_steps: int = 0,
) -> Dict:
    decided = n_detected + n_missed  # events resolved this episode (censored
                                     # in-flight events are excluded by the FSM)
    detection_rate = n_detected / decided if decided > 0 else float("nan")
    mean_cost = float(np.mean(costs)) if len(costs) else 0.0
    eff = detection_rate / mean_cost if (decided > 0 and mean_cost > 0) else (
        float("nan") if decided == 0 else 0.0
    )
    hist = np.bincount(actions, minlength=dyn.N_ACTIONS).astype(np.float64)
    hist /= max(hist.sum(), 1)
    return {
        "n_detected": int(n_detected),
        "n_missed": int(n_missed),
        "n_events_decided": int(decided),
        "detection_rate": float(detection_rate),
        "mean_cost": mean_cost,
        "info_efficiency": float(eff),
        "total_reward": float(np.sum(rewards)),
        "action_entropy": action_entropy(actions),
        "event_active_steps": int(event_active_steps),
        "attention": {dyn.ACTION_NAMES[i]: float(hist[i]) for i in range(dyn.N_ACTIONS)},
        "surprise_mean": {dyn.CHANNEL_NAMES[i]: float(surprise_mean[i]) for i in range(dyn.N_CHANNELS)},
        "pred_err_mean": {dyn.CHANNEL_NAMES[i]: float(pred_err_mean[i]) for i in range(dyn.N_CHANNELS)},
    }


def aggregate(episodes: List[Dict]) -> Dict:
    keys = ["detection_rate", "mean_cost", "info_efficiency", "total_reward",
            "action_entropy"]
    out: Dict = {"n_episodes": len(episodes)}
    for k in keys:
        vals = np.array([ep[k] for ep in episodes], dtype=np.float64)
        out[k] = {"mean": float(np.nanmean(vals)), "std": float(np.nanstd(vals))}
    out["n_detected"] = int(np.sum([ep["n_detected"] for ep in episodes]))
    out["n_missed"] = int(np.sum([ep["n_missed"] for ep in episodes]))
    # pooled detection rate across all episodes (more robust than mean-of-rates)
    dec = out["n_detected"] + out["n_missed"]
    out["detection_rate_pooled"] = float(out["n_detected"] / dec) if dec else float("nan")
    for group in ("attention", "surprise_mean", "pred_err_mean"):
        names = episodes[0][group].keys() if episodes else []
        out[group] = {
            n: float(np.mean([ep[group][n] for ep in episodes])) for n in names
        }
    return out
