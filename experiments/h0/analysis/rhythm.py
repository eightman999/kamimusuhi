"""Rhythm / periodicity analysis (spec section 15).

The point is NOT to require periodicity — it is to check whether any
behavioral structure is state-dependent rather than a hard-coded clock.
"""

from __future__ import annotations

from typing import Dict

import numpy as np

from ..env import dynamics as dyn


def action_transition_matrix(actions: np.ndarray) -> np.ndarray:
    """P(a_{t+1} | a_t) over the action sequence."""
    M = np.zeros((dyn.N_ACTIONS, dyn.N_ACTIONS))
    for a, b in zip(actions[:-1], actions[1:]):
        M[a, b] += 1
    row = M.sum(axis=1, keepdims=True)
    return M / np.maximum(row, 1)


def dwell_times(actions: np.ndarray) -> Dict[str, float]:
    """Mean consecutive-run length per action."""
    out = {}
    for a in range(dyn.N_ACTIONS):
        runs, cur = [], 0
        for x in actions:
            if x == a:
                cur += 1
            elif cur:
                runs.append(cur)
                cur = 0
        if cur:
            runs.append(cur)
        out[dyn.ACTION_NAMES[a]] = float(np.mean(runs)) if runs else 0.0
    return out


def action_autocorrelation(actions: np.ndarray, max_lag: int = 200) -> np.ndarray:
    """Autocorrelation of each action indicator, averaged over actions."""
    T = len(actions)
    acs = []
    for a in range(dyn.N_ACTIONS):
        x = (actions == a).astype(np.float64)
        x = x - x.mean()
        denom = np.sum(x * x)
        if denom == 0:
            acs.append(np.zeros(max_lag))
            continue
        ac = np.array(
            [np.sum(x[: T - l] * x[l:]) / denom for l in range(max_lag)]
        )
        acs.append(ac)
    return np.mean(acs, axis=0)


def dominant_period(actions: np.ndarray, max_lag: int = 200) -> Dict:
    """Dominant period from the action PSD (excluding DC)."""
    ac = action_autocorrelation(actions, max_lag)
    # FFT of the autocorrelation gives the PSD
    psd = np.abs(np.fft.rfft(ac))
    freqs = np.fft.rfftfreq(len(ac), d=1.0)
    if len(psd) <= 1:
        return {"period": None, "strength": 0.0}
    idx = int(np.argmax(psd[1:]) + 1)
    period = float(1.0 / freqs[idx]) if freqs[idx] > 0 else None
    strength = float(psd[idx] / (psd[1:].mean() + 1e-12))
    return {"period": period, "strength": strength, "psd": psd.tolist(),
            "freqs": freqs.tolist()}


def analyze_episode(trace: Dict) -> Dict:
    """Rhythm summary for one episode trace (from evaluate --traces)."""
    actions = np.array(trace["action"])
    internals = np.array(trace["internal"])
    return {
        "transition_matrix": action_transition_matrix(actions).tolist(),
        "dwell_times": dwell_times(actions),
        "dominant_period": dominant_period(actions),
        "action_counts": {
            dyn.ACTION_NAMES[a]: int((actions == a).sum())
            for a in range(dyn.N_ACTIONS)
        },
        "internal_ranges": {
            dyn.INTERNAL_NAMES[i]: [float(internals[:, i].min()),
                                    float(internals[:, i].max())]
            for i in range(dyn.N_INTERNAL)
        },
    }
