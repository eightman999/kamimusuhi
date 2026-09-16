"""A3.1 §14-§24, §57-§58: calibration targets + bounded fit.

Targets are *measurements*, parameters are *model knobs* — kept in
separate schemas (§15). Every target row carries species / sex /
stage / temperature / method / location / n / source (§16); adult
Drosophila rows are preferred and larval rows are flagged
``cross_stage_prior`` (§17-§18).

The optimizer is seeded random search inside physiological bounds
(§21). Fitted parameters are MODEL_INFERENCE with fit metadata
(§23) — fitting to a measurement never upgrades provenance.
"""
from __future__ import annotations

import json

import numpy as np

from .fly_overlay import FLY_PROFILES, FLY_PROFILES_V11
from .benchmark import (eval_profile, measure_rest, measure_rin_tau,
                        measure_rheobase, measure_fi, spike_shape)

CALIBRATION_VERSION = "calib-v1"

# ------------------------------------------------------------------ #
# literature targets (§16). These are *summarised* literature values   #
# for adult Drosophila central neurons; uncertainty kept in std/n.     #
# Sources are coarse citations — digitisation is future work.          #
# ------------------------------------------------------------------ #
TARGETS: dict[str, dict] = {
    "kenyon_cell": {
        "stage": "adult", "sex": "mixed", "temperature_C": 22,
        "method": "whole-cell patch", "location": "soma",
        "source": "adult KC physiology summaries (e.g. Gouwens & "
                  "Wilson 2009-class data)",
        "metrics": {
            "rest_V": {"mean": -65.0, "std": 6.0},
            "rin_density": {"mean": 4.0, "std": 2.0},   # 1/g units
            "tau_ms": {"mean": 12.0, "std": 6.0},
            "rheobase": {"mean": 8.0, "std": 6.0},      # µA/cm²
            "fi_slope_hz_per_uAcm2": {"mean": 1.5, "std": 1.0},
            "max_hz": {"mean": 60.0, "std": 30.0},
            "spike_width_ms": {"mean": 1.2, "std": 0.6},
            "ahp_mv": {"mean": -8.0, "std": 4.0},
        },
    },
    "projection_neuron": {
        "stage": "adult", "sex": "mixed", "temperature_C": 22,
        "method": "whole-cell patch", "location": "soma",
        "source": "adult antennal-lobe PN physiology summaries",
        "metrics": {
            "rest_V": {"mean": -58.0, "std": 6.0},
            "rin_density": {"mean": 2.5, "std": 1.2},
            "tau_ms": {"mean": 10.0, "std": 5.0},
            "rheobase": {"mean": 12.0, "std": 8.0},
            "fi_slope_hz_per_uAcm2": {"mean": 2.5, "std": 1.5},
            "max_hz": {"mean": 120.0, "std": 50.0},
            "spike_width_ms": {"mean": 1.0, "std": 0.5},
            "ahp_mv": {"mean": -10.0, "std": 5.0},
        },
    },
    "descending": {
        "stage": "adult", "sex": "mixed", "temperature_C": 22,
        "method": "whole-cell patch", "location": "soma",
        "source": "adult DN physiology summaries",
        "metrics": {
            "rest_V": {"mean": -58.0, "std": 6.0},
            "rin_density": {"mean": 2.0, "std": 1.0},
            "tau_ms": {"mean": 8.0, "std": 4.0},
            "rheobase": {"mean": 15.0, "std": 10.0},
            "fi_slope_hz_per_uAcm2": {"mean": 2.0, "std": 1.5},
            "max_hz": {"mean": 150.0, "std": 60.0},
            "spike_width_ms": {"mean": 1.0, "std": 0.5},
            "ahp_mv": {"mean": -9.0, "std": 5.0},
        },
    },
    "motor": {
        "stage": "adult", "sex": "mixed", "temperature_C": 22,
        "method": "patch (mixed adult/larval literature)",
        "location": "soma",
        "source": "MN physiology summaries — adult preferred; some "
                  "ranges cross-referenced from larval studies",
        "cross_stage_prior": True,      # §17 flag
        "metrics": {
            "rest_V": {"mean": -55.0, "std": 6.0},
            "rin_density": {"mean": 2.0, "std": 1.0},
            "tau_ms": {"mean": 8.0, "std": 4.0},
            "rheobase": {"mean": 15.0, "std": 10.0},
            "fi_slope_hz_per_uAcm2": {"mean": 3.0, "std": 2.0},
            "max_hz": {"mean": 180.0, "std": 80.0},
            "spike_width_ms": {"mean": 0.9, "std": 0.4},
            "ahp_mv": {"mean": -8.0, "std": 4.0},
        },
    },
}

#: physiological search bounds (§21) — units as in the overlay
BOUNDS = {
    "g_para": (10.0, 140.0),
    "g_shab": (2.0, 60.0),
    "g_shal": (0.0, 60.0),
    "g_shaker": (0.0, 60.0),
    "g_kca": (0.0, 30.0),          # A3.2 adaptation current
    "g_leak": (0.05, 1.0),
    "E_leak": (-75.0, -45.0),
    "Cm": (0.5, 2.0),
}

#: default loss weights (§22) — config-overridable
LOSS_W = {"rest_V": 1.0, "rin": 0.5, "tau": 0.5, "rheobase": 1.0,
          "fi_rmse": 1.0, "spike_width": 0.5, "ahp": 0.5}

#: §24 — which metrics the optimiser may see vs held-out validation
FIT_METRICS = ("rest_V", "rin", "tau", "rheobase", "fi_rmse")
VAL_METRICS = ("spike_width", "ahp")


def single_node_phys(profile_id: str, fitted: dict | None = None,
                     temperature_C: float = 25.0,
                     variant: str = "v1") -> dict:
    """One-node physiology block for a profile (soma only).
    ``variant`` "v1" | "v1_1" selects the channel-model set."""
    table = FLY_PROFILES if variant == "v1" else FLY_PROFILES_V11
    prof = table[profile_id]
    mem = dict(prof["membrane"])
    chans = {k: dict(v) for k, v in prof["channels"].items()}

    def _by_canon(canon):     # "para_Na" -> "para_Na[_v11]"
        return next((n for n in chans
                     if n == canon or n == canon + "_v11"), None)
    if fitted:
        gmap = {"g_para": "para_Na", "g_shab": "shab_K",
                "g_shal": "shal_K", "g_shaker": "shaker_K",
                "g_kca": "kca_K"}
        for k, v in fitted.items():
            if k in mem:
                mem[k] = float(v)
            elif k in gmap and _by_canon(gmap[k]) is not None:
                chans[_by_canon(gmap[k])]["g"] = float(v)
    channels = {}
    for name, c in chans.items():
        # single-node sim: inject the density wherever it would sit
        channels[name] = {"channel_model": name, "e_rev": c["e"],
                          "g_bar": [c["g"]],
                          "temperature_C": temperature_C}
    return {
        "runtime_mode": ("active_fly_v1" if variant == "v1"
                         else "active_fly_v1_1"),
        "active_idx": [0],
        "membrane": {"Cm": [mem["Cm"]], "g_leak": [mem["g_leak"]],
                     "E_leak": [mem["E_leak"]],
                     "V_rest": [mem["V_rest"]]},
        "channels": channels,
        "synapse": {"E_exc": 0.0, "E_inh": -80.0, "tau_syn": 5.0,
                    "weight_to_g": 0.5, "model": "receptor_aware_v0",
                    "unknown_sign_mode": "neutral"},
        "v_spike": -20.0, "stability": {"v_min": -110.0, "v_max": 90.0},
        "drive": {"g_exc": 1.0},
        "simulation_temperature_C": temperature_C,
    }


def _rel(a, b):
    return abs(a - b) / max(abs(b), 1e-9)


def _fi_rmse(fi, tgt):
    slope = tgt.get("fi_slope_hz_per_uAcm2", {}).get("mean")
    if slope is None:
        return 0.0
    errs = [(f["hz"] - slope * f["i_ext"]) ** 2 for f in fi]
    return float(np.sqrt(np.mean(errs)))


def calibrate_profile(profile_id: str, n_iter: int = 16, seed: int = 0,
                      temperature_C: float = 25.0,
                      weights: dict | None = None,
                      variant: str = "v1") -> dict:
    """Seeded bounded random search → fitted params + fit/val split
    metrics (§20-§24)."""
    tgt = TARGETS.get(profile_id)
    if tgt is None:
        return {"profile_id": profile_id, "status": "no_targets"}
    w = {**LOSS_W, **(weights or {})}
    rng = np.random.default_rng(seed)
    base = single_node_phys(profile_id,
                            temperature_C=temperature_C,
                            variant=variant)

    def loss_of(params):
        phys = single_node_phys(profile_id, fitted=params,
                                temperature_C=temperature_C,
                                variant=variant)
        r = measure_rest(phys, duration=120.0, seed=seed)
        if r["numerical_failure"]:
            return 1e6, {"fail": True}
        rt = measure_rin_tau(phys, seed=seed)
        rheo = measure_rheobase(phys, hi=150.0, iters=6,
                                duration=90.0, seed=seed)
        fi = measure_fi(phys, [5.0, 20.0, 60.0], duration=140.0,
                        seed=seed)
        errs = {
            "rest_V": _rel(r["rest_V"], tgt["metrics"]["rest_V"]["mean"]),
            "rin": _rel(rt["rin_density"],
                        tgt["metrics"]["rin_density"]["mean"]),
            "tau": _rel(rt["tau_ms"], tgt["metrics"]["tau_ms"]["mean"]),
            "rheobase": (_rel(rheo, tgt["metrics"]["rheobase"]["mean"])
                         if rheo is not None else 1.0),
            "fi_rmse": _fi_rmse(fi, tgt["metrics"]) / 50.0,
        }
        loss = sum(w[k] * errs[k] for k in FIT_METRICS)
        # §56: spontaneous firing at rest is a hard fail
        if r["spikes"] > 0:
            loss += 100.0
        return loss, {"metrics": errs, "rheobase": rheo, "fi": fi,
                      "rest_V": r["rest_V"]}

    best = None
    for _ in range(n_iter):
        params = {k: float(rng.uniform(*BOUNDS[k])) for k in BOUNDS}
        loss, det = loss_of(params)
        if best is None or loss < best[0]:
            best = (loss, params, det)

    loss, params, det = best
    # held-out validation metrics (§24): measured, not fitted
    phys = single_node_phys(profile_id, fitted=params,
                            temperature_C=temperature_C,
                            variant=variant)
    sh = spike_shape(phys, i_ext=max((det.get("rheobase") or 20.0)
                                     * 1.5, 5.0), seed=seed)
    val = {
        "spike_width_err": (
            _rel(sh["spike_width_ms"],
                 tgt["metrics"]["spike_width_ms"]["mean"])
            if sh["spike_width_ms"] else None),
        "ahp_err": (
            _rel(sh["ahp_mv"], tgt["metrics"]["ahp_mv"]["mean"])
            if sh["ahp_mv"] else None),
    }
    fitted_out = dict(params)
    return {
        "profile_id": profile_id, "status": "fitted",
        "optimizer": {"method": "seeded_random_search",
                      "n_iter": n_iter, "bounds": BOUNDS,
                      "weights": w, "seed": seed,
                      "variant": variant},
        "fitted_params": fitted_out,
        "final_loss": float(loss),
        "fit_metrics": det.get("metrics"),
        "validation_metrics": val,
        "provenance": "MODEL_INFERENCE",          # §23
        "fit_target_set": FIT_METRICS,
        "validation_set": VAL_METRICS,
        "source_measurements": tgt["source"],
        "cross_stage_prior": tgt.get("cross_stage_prior", False),
    }
