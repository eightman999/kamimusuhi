"""Metric computation for O0.

All functions take:
    pred  -- dict with pos [T,B] (normalized x_hat), exist [T,B]
             (probability), same [T,B] (probability)
    data  -- the dict returned by env.dynamics.generate_batch
    p     -- EnvParams (for world_len rescaling)

Positions are compared in normalized units (x / world_len); we also
report world units (x_err_world = pos_mae * world_len).
"""

from __future__ import annotations

import math
from typing import Optional

import numpy as np

from ..env import dynamics as dyn


def json_safe(obj):
    """Recursively convert a metrics container to strict-JSON-safe data:
    non-finite floats (NaN/inf, e.g. empty-mask means) become ``None`` and
    numpy scalars become Python scalars.  Lets ``json.dumps`` run with
    ``allow_nan=False`` so genuinely broken values still fail loudly."""
    if isinstance(obj, dict):
        return {k: json_safe(v) for k, v in obj.items()}
    if isinstance(obj, (list, tuple)):
        return [json_safe(v) for v in obj]
    if isinstance(obj, (bool, np.bool_)):
        return bool(obj)
    if isinstance(obj, np.integer):
        return int(obj)
    if isinstance(obj, (float, np.floating)):
        return float(obj) if math.isfinite(obj) else None
    return obj


def _masked_mean(values, mask):
    m = np.asarray(mask, dtype=bool)
    if not m.any():
        return float("nan")
    return float(np.mean(np.asarray(values)[m]))


def first_bout_masks(data):
    """Mask of the invisibility window: from first occlusion until
    reappearance, or until episode end if the object never comes back
    (absorbed or still hidden).  For absorbed episodes the window thus
    includes the post-death steps on which a world model must answer
    "it no longer exists"."""
    T, B = data["occluded"].shape
    t = np.arange(T)[:, None]
    end = np.where(data["t_reapp"] > 0, data["t_reapp"], T)
    return (data["t_occl"][None, :] >= 0) & (t >= data["t_occl"][None, :]) & (t < end[None, :])


def compute_metrics(pred, data, p: dyn.EnvParams,
                    reset_steps: Optional[np.ndarray] = None) -> dict:
    pos_err = np.abs(pred["pos"] - data["pos"])          # normalized
    exist_true = data["exist"]
    exist_pred = (pred["exist"] > 0.5).astype(np.float64)
    same_true = data["same"]
    same_pred = (pred["same"] > 0.5).astype(np.float64)

    occ = data["occluded"]                  # physically inside & alive
    vis = data["visible"]
    bout = first_bout_masks(data)           # invisibility window (incl. dead)
    hidden = bout & ~vis                    # should-be-tracked window
    hidden_alive = hidden & (exist_true > 0.5)   # pos error only while it exists

    m = {}
    m["pos_mae_visible"] = _masked_mean(pos_err, vis)
    m["pos_mae_occluded"] = _masked_mean(pos_err, hidden_alive)
    m["pos_mae_occluded_world"] = m["pos_mae_occluded"] * p.world_len

    # --- decomposition (reviewer C1): the aggregate mixes episode types.
    # The canonical claim is position tracking on *persistent* occlusion
    # bouts of the trained length; gone/long-bout episodes measure
    # different skills (boundary-absorption correction, long integration).
    gone_ep = data["gone"].astype(bool)[None, :]          # geometry flag
    bl = data["bout_len"].astype(np.float64)[None, :]
    m["pos_mae_occ_persistent"] = _masked_mean(
        pos_err, hidden_alive & ~gone_ep)
    m["pos_mae_occ_gone"] = _masked_mean(
        pos_err, hidden_alive & gone_ep)
    m["pos_mae_occ_le16"] = _masked_mean(
        pos_err, hidden_alive & (bl <= 16))
    m["pos_mae_occ_gt16"] = _masked_mean(
        pos_err, hidden_alive & (bl > 16))
    # the strict headline: persistent AND realized bout within trained
    # range  -- "did it track through an ordinary occlusion?"
    m["pos_mae_occ_persist_le16"] = _masked_mean(
        pos_err, hidden_alive & ~gone_ep & (bl <= 16))
    # per-episode spread -> a proper CI over the eval batch (episodes are
    # the independent unit, not steps)
    T, B = occ.shape
    ep_err = np.full(B, np.nan)
    for b in range(B):
        mb = hidden_alive[:, b]
        if mb.any():
            ep_err[b] = pos_err[mb, b].mean()
    n_ep = int(np.isfinite(ep_err).sum())
    m["pos_mae_occluded_ep_std"] = (
        float(np.nanstd(ep_err, ddof=1)) if n_ep > 1 else float("nan"))
    m["pos_mae_occluded_ci95"] = (
        float(1.96 * np.nanstd(ep_err, ddof=1) / np.sqrt(n_ep))
        if n_ep > 1 else float("nan"))
    m["n_pos_episodes"] = n_ep

    # --- decoy-takeover steps (target channel reports a wrong object) ---
    tk = data.get("decoy")
    if tk is not None and tk.any():
        m["frac_decoy_episodes"] = float(tk.any(axis=0).mean())
        m["n_decoy_steps"] = int(tk.sum())
        m["pos_mae_decoy"] = _masked_mean(pos_err, tk & (exist_true > 0.5))
        m["id_acc_decoy"] = _masked_mean(
            (same_pred == same_true).astype(np.float64), tk)

    # --- post-intervention-only degradation (reviewer minor): the
    # window-averaged MAE dilutes the effect; measure hidden error
    # strictly after the reset/noise step ---
    t = np.arange(T)[:, None]
    if reset_steps is not None:
        rs = np.asarray(reset_steps)[None, :]
        post_alive = hidden_alive & (t > rs) & (rs >= 0)
        m["pos_mae_post_reset"] = _masked_mean(pos_err, post_alive)
        post_hid = hidden & (t > rs) & (rs >= 0)
        m["exist_acc_post_reset"] = _masked_mean(
            (exist_pred == exist_true).astype(np.float64), post_hid)
        m["n_post_reset"] = int(post_alive.sum())
    # forecast at the moment of reappearance: error on the last hidden
    # step before each episode's reappearance
    T, B = occ.shape
    t = np.arange(T)[:, None]
    last_hidden = (
        (t == (data["t_reapp"] - 1)[None, :])
        & (data["t_reapp"][None, :] > 0)
        & (exist_true > 0.5)
    )
    m["pos_err_pre_reappear"] = _masked_mean(pos_err, last_hidden)
    # existence accuracy inside the invisibility window and overall
    m["exist_acc_hidden"] = _masked_mean(
        (exist_pred == exist_true).astype(np.float64), hidden
    )
    m["exist_acc_all"] = float((exist_pred == exist_true).mean())
    # existence accuracy in the hidden window of absorbed (vanished)
    # episodes only: does the model notice the object left the world?
    absorbed_eps = data["absorbed"].astype(bool)
    if absorbed_eps.any():
        m["exist_acc_absorbed"] = _masked_mean(
            (exist_pred == exist_true)[:, absorbed_eps].astype(np.float64),
            hidden[:, absorbed_eps],
        )
    else:
        m["exist_acc_absorbed"] = float("nan")
    # identity accuracy over post-occlusion visible steps
    m["id_acc"] = _masked_mean(
        (same_pred == same_true).astype(np.float64), data["id_mask"]
    )
    # split id accuracy by episode outcome
    swapped_eps = data["swapped"].astype(bool)
    if swapped_eps.any():
        m["id_acc_swapped"] = _masked_mean(
            (same_pred == same_true)[:, swapped_eps].astype(np.float64),
            data["id_mask"][:, swapped_eps],
        )
    if (~swapped_eps).any():
        m["id_acc_intact"] = _masked_mean(
            (same_pred == same_true)[:, ~swapped_eps].astype(np.float64),
            data["id_mask"][:, ~swapped_eps],
        )
    # error growth vs occlusion step index k (1..bout_len)
    kidx = t - data["t_occl"][None, :] + 1
    max_k = int(max(1, np.nanmax(data["bout_len"])))
    curve, curve_n = [], []
    for k in range(1, min(max_k, 64) + 1):
        mask = hidden_alive & (kidx == k)
        curve.append(_masked_mean(pos_err, mask))
        curve_n.append(int(mask.sum()))
    m["err_vs_occstep"] = curve
    m["err_vs_occstep_n"] = curve_n
    # error vs planned bout length
    m["mean_bout_len"] = float(data["bout_len"][data["bout_len"] > 0].mean())
    m["frac_occluded"] = float((data["t_occl"] >= 0).mean())
    m["frac_reappeared"] = float((data["t_reapp"] > 0).mean())
    m["frac_absorbed"] = float(data["absorbed"].mean())
    m["n_episodes"] = int(B)
    return m


def curve_by_boutlen(pred, data, bins=(4, 8, 12, 16, 24, 32, 48, 96)):
    """Mean hidden-window position error binned by bout length."""
    pos_err = np.abs(pred["pos"] - data["pos"])
    bout = first_bout_masks(data)
    hidden = bout & ~data["visible"] & (data["exist"] > 0.5)
    out = []
    bl = data["bout_len"]
    for lo, hi in zip(bins[:-1], bins[1:]):
        sel = (bl > lo) & (bl <= hi)
        if sel.any():
            mask = hidden[:, sel]
            out.append(
                {
                    "range": f"{lo}-{hi}",
                    "pos_mae": _masked_mean(pos_err[:, sel], mask),
                    "n_episodes": int(sel.sum()),
                }
            )
    return out
