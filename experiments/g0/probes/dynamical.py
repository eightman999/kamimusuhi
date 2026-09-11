"""Dynamical-signature features + segment-level probes.

The latent causes are distinguished by *dynamics* (decay, per-action
response, oscillation, spiking), not by instantaneous appearance. These
hand-crafted, permutation/sign-robust features extract exactly that
signature from a window of a vector stream — used two ways:

  * as an oracle-ish HEADROOM reference when applied to the env's
    canonical signal (and a hard baseline when applied to raw obs), and
  * as a nonlinear/dynamical probe on each representation's latents, so
    "a linear probe lacked power" cannot explain a negative result.

All features are invariant to permutation and sign flips of the input
dims (they are means over dims, norms, correlations, spectra), so the
same extractor transfers across sensor contexts.
"""

from __future__ import annotations

from typing import Dict, Optional

import numpy as np

from ..env.dynamics import N_ACTIONS

FEAT_NAMES = (
    "n_mean", "n_std", "dn_mean", "dn_std",
    "ac1", "ac2", "ac4", "ac8",            # per-dim obs autocorr
    "dac1",                                # per-dim delta autocorr
    "nac1", "nac2", "nac4",                # norm autocorr
    "spec_lo", "spec_mid", "spec_hi", "spec_vhi", "spec_peak",
    "resp_APP", "resp_WIT", "resp_TAP", "resp_NOOP",
    "spike_max", "spike_frac", "xcorr", "level_std",
)


def dyn_features(x: np.ndarray, a: np.ndarray,
                 next_x: Optional[np.ndarray] = None) -> np.ndarray:
    """Dynamical signature of one segment window.

    x      (L, D) stream (obs, canonical signal, or model latents)
    a      (L,) actions taken at each position
    next_x (L, D) optional: post-action successor of x. When given,
    deltas are x->next_x (driven by a[t]); else x[t]->x[t+1] (driven
    by a[t+1]).
    """
    x = np.asarray(x, dtype=np.float64)
    L, D = x.shape
    n = np.linalg.norm(x, axis=1)
    if next_x is not None:
        d = np.asarray(next_x, dtype=np.float64) - x
        drive = a
    else:
        d = np.diff(x, axis=0)
        drive = a[1:]
    dn = np.linalg.norm(d, axis=1)
    f = [float(n.mean()), float(n.std()), float(dn.mean()),
         float(dn.std())]

    # per-dim autocorrelation (sign- and permutation-invariant)
    xc = x - x.mean(0)
    sd = x.std(0) + 1e-9
    for lag in (1, 2, 4, 8):
        if L > lag + 2:
            ac = (xc[lag:] * xc[:-lag]).mean(0) / (sd * sd)
            f.append(float(np.clip(ac.mean(), -1, 1)))
        else:
            f.append(0.0)

    # delta autocorrelation (oscillation -> negative lag-1)
    if d.shape[0] > 2:
        dc = d - d.mean(0)
        dd = d.std(0) + 1e-9
        f.append(float(np.clip(
            ((dc[1:] * dc[:-1]).mean(0) / (dd * dd)).mean(), -1, 1)))
    else:
        f.append(0.0)

    # autocorrelation of the stream norm (envelope dynamics)
    nn = (n - n.mean()) / (n.std() + 1e-9)
    for lag in (1, 2, 4):
        f.append(float(np.clip((nn[lag:] * nn[:-lag]).mean(), -1, 1))
                 if L > lag + 2 else 0.0)

    # spectrum of the norm (OSC causes a mid-band peak)
    if L >= 8 and n.std() > 1e-9:
        spec = np.abs(np.fft.rfft(n - n.mean()))
        freqs = np.fft.rfftfreq(L)
        tot = spec[1:].sum() + 1e-9
        f += [
            float(spec[(freqs > 0) & (freqs < 0.04)].sum() / tot),
            float(spec[(freqs >= 0.04) & (freqs < 0.12)].sum() / tot),
            float(spec[(freqs >= 0.12) & (freqs < 0.30)].sum() / tot),
            float(spec[freqs >= 0.30].sum() / tot),
            float(freqs[1 + np.argmax(spec[1:])]),
        ]
    else:
        f += [0.0] * 5

    # per-action delta response (action-conditional drive strength)
    m = dn.mean() + 1e-9
    for ai in range(N_ACTIONS):
        mask = drive == ai
        f.append(float(dn[mask].mean() / m) if mask.any() else 1.0)

    # spiking / heavy-tail stats
    med = np.median(dn) + 1e-9
    f.append(float(dn.max() / med))
    f.append(float((dn > 2.5 * med).mean()))

    # mean sign-change rate across dims (sign-invariant)
    f.append(float((x[:-1] * x[1:] < 0).mean()) if L > 1 else 0.0)

    # level spread across dims of the mean vector (|.| -> sign-safe)
    f.append(float(np.abs(x.mean(0)).std()))
    return np.asarray(f)


def dynseg_dataset(X: np.ndarray, act: np.ndarray, ds: dict,
                   warmup: int, tail: float = 0.5,
                   next_X: Optional[np.ndarray] = None) -> Dict:
    """Pool dynamical features over the tail of each single-cause
    segment -> one feature vector per segment (a "concept" descriptor).

    Same masking policy as analysis.metrics.segment_means. X is
    (E,T,D) obs / canonical / latents; act (E,T); ds carries seg_id,
    cause_a, cause_b, ctx."""
    E, T, D = X.shape
    seg = ds["seg_id"][:, warmup:]
    ca = ds["cause_a"][:, warmup:]
    cb = ds["cause_b"][:, warmup:]
    ctx = ds["ctx"][:, warmup:] if "ctx" in ds else np.broadcast_to(
        ds["ctx_id"][:, None], (E, T - warmup))
    Xw = X[:, warmup:, :]
    Aw = act[:, warmup:]
    Nw = next_X[:, warmup:, :] if next_X is not None else None
    out_z, out_c, out_x, out_e = [], [], [], []
    for e in range(E):
        for s in np.unique(seg[e]):
            m = seg[e] == s
            if cb[e][m][0] >= 0:
                continue                      # single-cause segments only
            idx = np.where(m)[0]
            n0 = idx[0] + max(1, int(len(idx) * (1 - tail)))
            idx = idx[idx >= n0]
            if len(idx) < 3:
                continue                      # need >=3 pts for dyn feats
            out_z.append(dyn_features(
                Xw[e][idx], Aw[e][idx],
                Nw[e][idx] if Nw is not None else None))
            out_c.append(int(ca[e][m][0]))
            out_x.append(int(ctx[e][idx[0]]))
            out_e.append(e)
    if not out_z:
        return {"Z": np.zeros((0, len(FEAT_NAMES))),
                "cause": np.zeros(0, np.int64),
                "ctx": np.zeros(0, np.int64),
                "ep": np.zeros(0, np.int64)}
    return {"Z": np.stack(out_z), "cause": np.asarray(out_c),
            "ctx": np.asarray(out_x), "ep": np.asarray(out_e)}
