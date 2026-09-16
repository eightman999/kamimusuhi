"""A3.1 §39-§43: single-cell electrophysiology benchmark suite.

Runs one isolated active node through the standard probes —
rest, step current, ramp, repeated pulse, hyperpolarising pulse —
and extracts the calibration metrics (§14/§43). Voltage traces and
channel state can be saved as artifacts (§41).

All currents are current *densities* (µA/cm²); converting to absolute
input resistance needs a cell area — carried per profile target,
never invented for an entity (§15).
"""
from __future__ import annotations

import numpy as np
import torch

from ..fba.torch_backend import TorchBackend


def sim_single(phys: dict, duration_ms: float = 200.0,
               i_ext: float = 0.0, dt: float = 0.1,
               trace: bool = False, seed: int = 0) -> TorchBackend:
    """One isolated active node, optional constant injection."""
    post = np.array([0]); pre = np.array([0])
    w = np.array([0.0]); esyn = np.array([0.0])
    mode = (phys or {}).get("runtime_mode", "active_fly_v1")
    b = TorchBackend(synthetic=False,
                     base_override=(1, post, pre, w, esyn),
                     runtime_mode=mode)
    b.initialize(phenotype={"physiology": phys}, batch_size=1,
                 seed=seed, device="cpu")
    if trace:
        b.enable_trace([0])
    if i_ext:
        b.set_current_injection({0: i_ext})
    b.run(duration_ms)
    return b


def _spike_times(tr_v, tr_t, thr=-20.0):
    v = np.asarray(tr_v); t = np.asarray(tr_t)
    on = (v[1:] >= thr) & (v[:-1] < thr)
    return t[1:][on]


def measure_rest(phys, duration=150.0, seed=0):
    b = sim_single(phys, duration, trace=True, seed=seed)
    tr = b.get_trace()[0]
    v = np.asarray(tr["v"])
    tail = v[len(v) // 2:]
    fail = b._num_failure is not None
    return {"rest_V": float(tail.mean()), "rest_sd": float(tail.std()),
            "spikes": int(b.spike_counts[0, 0]),
            "numerical_failure": fail,
            "trace": tr if fail else None}


def measure_rin_tau(phys, i_hyp=-2.0, pre_ms=60.0, pulse_ms=80.0,
                    seed=0):
    """Hyperpolarising pulse → Rin proxy (ΔV/I, density units) and
    membrane tau from the post-pulse recovery (63% rule)."""
    # baseline
    b = sim_single(phys, pre_ms, trace=True, seed=seed)
    v0 = float(np.asarray(b.get_trace()[0]["v"])[-20:].mean())
    # pulse — inject during the whole run; measure pre-steady dip
    b = sim_single(phys, pre_ms + pulse_ms, i_ext=i_hyp, trace=True,
                   seed=seed)
    tr = b.get_trace()[0]
    v = np.asarray(tr["v"]); t = np.asarray(tr["t"])
    during = v[t > pre_ms + pulse_ms * 0.7]
    dv = float(during.mean() - v0)
    rin_density = abs(dv / i_hyp)          # mV per µA/cm² = 1/g units
    # tau: after pulse end? we injected for whole run — use approach
    # to the new steady state from pulse onset instead
    seg = v[(t > pre_ms)]
    tgt = during.mean()
    x = np.abs(seg - tgt) / max(abs(seg[0] - tgt), 1e-9)
    k63 = int(np.argmax(x < 0.368)) if (x < 0.368).any() else len(x) - 1
    tau = float((t[(t > pre_ms)][k63] - pre_ms))
    return {"v0": v0, "dv": dv, "rin_density": rin_density,
            "tau_ms": tau}


def measure_rheobase(phys, lo=0.0, hi=200.0, iters=8,
                     duration=120.0, seed=0):
    """Bisection on step-current amplitude → first-spike threshold
    (current density)."""
    def fires(i):
        b = sim_single(phys, duration, i_ext=i, seed=seed)
        return int(b.spike_counts[0, 0]) > 0
    if not fires(hi):
        return None
    for _ in range(iters):
        mid = 0.5 * (lo + hi)
        if fires(mid):
            hi = mid
        else:
            lo = mid
    return float(hi)


def measure_fi(phys, amps, duration=200.0, seed=0):
    """F-I curve: constant injection → steady-state firing rate."""
    out = []
    for a in amps:
        b = sim_single(phys, duration, i_ext=a, trace=True, seed=seed)
        tr = b.get_trace()[0]
        st = _spike_times(tr["v"], tr["t"])
        st = st[st > duration * 0.25]      # skip onset transient
        hz = len(st) / (duration * 0.75 / 1000.0)
        out.append({"i_ext": float(a), "hz": float(hz)})
    return out


def spike_shape(phys, i_ext, duration=120.0, seed=0):
    """Width at half-height + AHP depth of the first spike."""
    b = sim_single(phys, duration, i_ext=i_ext, trace=True, seed=seed)
    tr = b.get_trace()[0]
    v = np.asarray(tr["v"]); t = np.asarray(tr["t"])
    st = _spike_times(tr["v"], tr["t"])
    if not len(st):
        return {"spike_width_ms": None, "ahp_mv": None}
    t0 = float(st[0])
    w = (t >= t0 - 2.0) & (t <= t0 + 8.0)
    seg_v, seg_t = v[w], t[w]
    if not len(seg_v):
        return {"spike_width_ms": None, "ahp_mv": None}
    base = float(v[t < t0 - 0.5].mean()) if (t < t0 - 0.5).any() \
        else float(seg_v[0])
    peak = float(seg_v.max())
    half = base + 0.5 * (peak - base)
    above = seg_t[seg_v >= half]
    width = float(above.max() - above.min()) if len(above) else None
    ahp = float(seg_v.min() - base)
    return {"spike_width_ms": width, "ahp_mv": ahp,
            "peak_mv": peak, "trace": tr}


def eval_profile(phys, target: dict, seed=0, duration=200.0,
                 amps=None) -> dict:
    """Full metric set for one profile. ``target`` carries the
    literature targets (§14); metrics are computed, then compared by
    calibrate.py's loss — fit vs held-out split lives there (§24)."""
    # current-density range spanning sub-rheobase → saturating drive
    # (fly-cell F-I, µA/cm²)
    amps = amps if amps is not None else [2, 4, 7, 12, 20, 35]
    m = {}
    r = measure_rest(phys, seed=seed)
    m["rest_V"] = r["rest_V"]
    m["rest_spikes"] = r["spikes"]
    m["rest_fail"] = r["numerical_failure"]
    rt = measure_rin_tau(phys, seed=seed)
    m["rin_density"] = rt["rin_density"]
    m["tau_ms"] = rt["tau_ms"]
    m["rheobase"] = measure_rheobase(phys, seed=seed)
    m["fi"] = measure_fi(phys, amps, seed=seed)
    m["shape"] = spike_shape(phys, i_ext=(m["rheobase"] or 20.0) * 1.5,
                             seed=seed)
    return m
