"""G0 evaluation metrics operating on encoded latents.

All functions take a representation's encoded latents Z (E,T,H) plus the
dataset dict (which carries eval-only ground truth: cause_a/cause_b,
ctx_id, seg_id, switch, x_a, x_b). No sklearn — everything numpy/torch.
"""

from __future__ import annotations

from typing import Dict, Optional, Tuple

import numpy as np

from ..env.dynamics import CAUSE_TABLE, N_ACTIONS, N_CAUSES, NOOP, TAP

EPS = 1e-12


def auc_score(labels_pos: np.ndarray, scores: np.ndarray) -> float:
    order = np.argsort(scores)
    ranks = np.empty_like(order, dtype=np.float64)
    ranks[order] = np.arange(1, len(scores) + 1)
    pos = labels_pos.astype(bool)
    n_pos, n_neg = pos.sum(), (~pos).sum()
    if n_pos == 0 or n_neg == 0:
        return float("nan")
    return float((ranks[pos].sum() - n_pos * (n_pos + 1) / 2)
                 / (n_pos * n_neg))


def best_action_labels(cause_a: np.ndarray) -> np.ndarray:
    """Per-step class: which action has the largest |response| for the
    active cause (NOOP if the cause barely responds)."""
    tab = np.zeros((N_CAUSES, N_ACTIONS))
    for c, spec in enumerate(CAUSE_TABLE):
        # |per-action drive|; TAP additionally triggers spikes for PULSE
        tab[c] = np.abs(spec.resp)
        tab[c, TAP] += spec.tap_spike_p * spec.spike_amp
        if tab[c].max() < 0.05:
            tab[c, NOOP] = 1.0  # NEUTRAL -> "no action matters"
    return tab[cause_a].argmax(axis=1)


def flat_steps(ds: dict, warmup: int) -> dict:
    """Flatten (E,T,...) eval data into per-step arrays, dropping the
    first `warmup` steps of each episode."""
    E, T = ds["actions"].shape
    keep = np.arange(T) >= warmup
    sl = np.s_[:, keep]
    cause_a = ds["cause_a"][sl].ravel()
    cause_b = ds["cause_b"][sl].ravel()
    switch_next = np.zeros((E, T - warmup), dtype=bool)
    sw = ds["switch"]
    switch_next[:, :-1] = sw[:, warmup + 1:]
    ctx = ds["ctx"][sl].ravel() if "ctx" in ds else np.repeat(
        ds["ctx_id"], T - warmup)
    # steps since the current segment started (position within segment)
    seg = ds["seg_id"][:, warmup:]
    seg_pos = np.zeros((E, T - warmup), dtype=np.int64)
    for e in range(E):
        first = {}
        for t in range(T - warmup):
            s = seg[e, t]
            if s not in first:
                first[s] = t
            seg_pos[e, t] = t - first[s]
    return {
        "cause_a": cause_a,
        "cause_b": cause_b,
        "single": cause_b < 0,
        "ctx": ctx,
        "x_a": ds["x_a"][sl].ravel(),
        "best_action": best_action_labels(cause_a),
        "switch_next": switch_next.ravel(),
        "episode": np.repeat(np.arange(E), T - warmup),
        "seg_id": ds["seg_id"][sl].ravel(),
        "seg_pos": seg_pos.ravel(),
        "step_of_ep": np.tile(np.arange(T - warmup), E),
    }


def flat_latents(Z: np.ndarray, warmup: int) -> np.ndarray:
    E, T, H = Z.shape
    return Z[:, warmup:, :].reshape(-1, H)


def segment_means(Z: np.ndarray, ds: dict, warmup: int,
                  tail: float = 0.5) -> dict:
    """Pool latents over the tail of each single-cause segment ->
    segment-level concept vectors.

    Returns dict: Z (n_seg,H), cause (n_seg,), ctx (n_seg,),
    ep (n_seg,)."""
    E, T, H = Z.shape
    Zw = Z[:, warmup:, :]
    seg = ds["seg_id"][:, warmup:]
    ca = ds["cause_a"][:, warmup:]
    cb = ds["cause_b"][:, warmup:]
    ctx = ds["ctx"][:, warmup:] if "ctx" in ds else np.broadcast_to(
        ds["ctx_id"][:, None], (E, T - warmup))
    out_z, out_c, out_x, out_e = [], [], [], []
    for e in range(E):
        for s in np.unique(seg[e]):
            m = seg[e] == s
            if cb[e][m][0] >= 0:
                continue  # skip pair segments for cause-id metrics
            idx = np.where(m)[0]
            n0 = idx[0] + max(1, int(len(idx) * (1 - tail)))
            idx = idx[idx >= n0]
            if len(idx) < 2:
                continue
            out_z.append(Zw[e][idx].mean(0))
            out_c.append(int(ca[e][m][0]))
            out_x.append(int(ctx[e][idx[0]]))
            out_e.append(e)
    if not out_z:
        return {"Z": np.zeros((0, H)), "cause": np.zeros(0, np.int64),
                "ctx": np.zeros(0, np.int64), "ep": np.zeros(0, np.int64)}
    return {"Z": np.stack(out_z), "cause": np.asarray(out_c),
            "ctx": np.asarray(out_x), "ep": np.asarray(out_e)}


def ep_demean(X: np.ndarray, group: np.ndarray) -> np.ndarray:
    """Subtract the per-group (episode or episode-x-context) mean.
    Removes the slowly-varying nuisance block (e.g. a recurrent model's
    estimate of the current context) from latents."""
    Xd = X.copy()
    for g in np.unique(group):
        m = group == g
        Xd[m] = X[m] - X[m].mean(0)
    return Xd


def episode_split(E: int) -> Tuple[np.ndarray, np.ndarray]:
    """Deterministic episode-level train/test split (first/second half)."""
    first = np.zeros(E, dtype=bool)
    first[: E // 2] = True
    return first, ~first


def cause_centroids(Z_flat: np.ndarray, labels: np.ndarray,
                    ctx: np.ndarray, ctx_id: int,
                    n_causes: int = N_CAUSES) -> Tuple[np.ndarray, np.ndarray]:
    """Per-cause latent centroids within one context.
    Returns (centroids (n_present,H), cause_ids (n_present,))."""
    m = (ctx == ctx_id) & (labels >= 0)
    cents, ids = [], []
    for c in range(n_causes):
        mc = m & (labels == c)
        if mc.sum() >= 2:
            cents.append(Z_flat[mc].mean(0))
            ids.append(c)
    if not cents:
        return np.zeros((0, Z_flat.shape[1])), np.zeros(0, dtype=np.int64)
    return np.stack(cents), np.asarray(ids)


def obs_space_prototypes(env_cfg, ctx_id: int, env_seed: int) -> np.ndarray:
    """Expected obs direction per cause in a context: C_k @ W @ v_c
    (normalized). Used for latent-intervention analysis."""
    from ..env.dynamics import make_dynamics_params
    p = make_dynamics_params(env_cfg, seed=env_seed)
    proto = p.contexts[ctx_id] @ p.w @ p.v_table.T  # (D, n_causes)
    return proto / (np.linalg.norm(proto, axis=0, keepdims=True) + EPS)


def shuffle_episode_steps(obs: np.ndarray, act: np.ndarray,
                          seed: int, ret_perm: bool = False):
    """Destroy temporal order while preserving obs-action pairing and
    per-episode marginals. Kills all dynamical cues.

    With ret_perm=True also returns perms (E,T): position t of the
    shuffled episode e holds original step perms[e, t] — needed to build
    label-aligned shuffled targets."""
    rng = np.random.default_rng(seed)
    E, T, D = obs.shape
    o, a = obs.copy(), act.copy()
    perms = np.zeros((E, T), dtype=np.int64)
    for e in range(E):
        p = rng.permutation(T)
        perms[e] = p
        o[e] = o[e][p]
        a[e] = a[e][p]
    if ret_perm:
        return o, a, perms
    return o, a


def dropout_dims(obs: np.ndarray, frac: float, seed: int) -> np.ndarray:
    """Zero a fixed random subset of sensor dims (sensor dropout)."""
    rng = np.random.default_rng(seed)
    n_drop = int(round(obs.shape[-1] * frac))
    dims = rng.choice(obs.shape[-1], n_drop, replace=False)
    o = obs.copy()
    o[..., dims] = 0.0
    return o
