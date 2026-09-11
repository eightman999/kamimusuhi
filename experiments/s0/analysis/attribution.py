"""Action-sensitivity attribution: which obs dims does the MODEL treat
as action-caused? Compared against ground-truth cause labels this gives
the causal attribution accuracy (S0 agency metric)."""

from __future__ import annotations

import numpy as np
import torch


def action_sensitivity(model, obs: torch.Tensor, act: torch.Tensor,
                       n_actions: int) -> np.ndarray:
    """Per-dim std of predictions when the action input is varied over
    all actions at each timestep, averaged over time/episodes.

    obs: (E,T,D) eval episodes, act: (E,T) actual actions (only the last
    action in each fed window is perturbed — for MLP each step is
    independent; for GRU we perturb every position's action which
    measures the model's overall reliance on the action channel).
    """
    if not model.use_action:
        return np.zeros(model.obs_dim)
    e, t, d = obs.shape
    with torch.no_grad():
        preds = []
        for a in range(n_actions):
            aa = torch.full_like(act, a)
            p, _, _ = model(obs, aa)
            preds.append(p)
        p = torch.stack(preds)                    # (A,E,T,D)
    return p.std(dim=0).mean(dim=(0, 1)).cpu().numpy()   # (D,)


def last_action_sensitivity(model, obs: torch.Tensor, act: torch.Tensor,
                            n_actions: int, window: int = 16) -> np.ndarray:
    """Like action_sensitivity but only the LAST action of each fed
    window is replaced; prediction taken at the last step. This isolates
    the immediate effect of the current action (closer to the env's
    ground-truth action_component)."""
    if not model.use_action:
        return np.zeros(model.obs_dim)
    e, t, d = obs.shape
    w = min(window, t)
    o = obs[:, -w:]
    with torch.no_grad():
        preds = []
        for a in range(n_actions):
            aa = act[:, -w:].clone()
            aa[:, -1] = a
            p, _, _ = model(o, aa)
            preds.append(p[:, -1])
        p = torch.stack(preds)                    # (A,E,D)
    return p.std(dim=0).mean(dim=0).cpu().numpy()  # (D,)


def auc_score(labels_pos: np.ndarray, scores: np.ndarray) -> float:
    """Rank-based AUC of `scores` for binary `labels_pos`."""
    order = np.argsort(scores)
    ranks = np.empty_like(order, dtype=np.float64)
    ranks[order] = np.arange(1, len(scores) + 1)
    pos = labels_pos.astype(bool)
    n_pos, n_neg = pos.sum(), (~pos).sum()
    if n_pos == 0 or n_neg == 0:
        return float("nan")
    return float((ranks[pos].sum() - n_pos * (n_pos + 1) / 2)
                 / (n_pos * n_neg))


def attribution_auc(sens: np.ndarray, cause_labels: np.ndarray) -> dict:
    """AUC of model sensitivity for predicting true action-dependence."""
    from env.dynamics import CauseLabels
    affected = np.isin(cause_labels, [CauseLabels.SELF, CauseLabels.MIXED])
    pure_self = cause_labels == CauseLabels.SELF
    return {
        "auc_self_or_mix": auc_score(affected, sens),
        "auc_pure_self": auc_score(pure_self, sens),
        "sensitivity": sens.tolist(),
        "labels": cause_labels.tolist(),
    }
