"""A3.2 — physiology hardening + graft→AIS/output causal path
(A3.2 §3-§19).

active_fly_v1_1 = flychan-v1_1 (slower Para kinetics → wider spikes)
+ kca_K_v11 KCa-lite adaptation current (MODEL_INFERENCE). A3.1
runtime/overlay stay frozen; this script runs both as the comparison
arm (§11 table).

Causal acceptance (§16-§17): full vs efferent_disabled must differ in
AIS spike count OR host output count, direction-consistent across
paired replicates, deterministic — not just dendritic ΔV.
"""
from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

import numpy as np

from ..anatomy import compartments as C
from ..physio.calibrate import TARGETS, calibrate_profile, single_node_phys
from ..physio.benchmark import eval_profile
from ..physio.fly_compiler import AIS, compile_reduced_graph_v4
from ..physio.fly_overlay import (build_fly_overlay, load_fly_overlay,
                                  resolve_fly_entity_params)
from .a3_active_graft import _run
from .a31_fly_calibration import CALIBRATE_PROFILES, FIT_UNITS
from .g01_compartment_graft import (_build_circuit, _graft_phen,
                                    matched_targets)


def _run32(base, coup, phen, drive_idx, drive_hz, duration_ms, seed,
           replicates, mode, ais_nodes=(), trace_nodes=()):
    """_run + per-AIS-node spike counts + optional V trace (§14-15)."""
    from ..fba.torch_backend import TorchBackend
    from ..fba.replicates import replicate_seeds
    b = TorchBackend(synthetic=False, base_override=base,
                     voltage_coupling=coup, runtime_mode=mode)
    p = {"artificial_organs": [], "attachments": [],
         "n_extra_neurons": 0, "params": {}}
    if phen:
        p.update(phen)
    b.initialize(p, batch_size=replicates, seed=seed, device="cpu",
                 replicate_seeds=replicate_seeds(seed, replicates))
    if trace_nodes:
        b.enable_trace(trace_nodes)
    b.set_inputs({"rates_hz": {str(i): drive_hz for i in drive_idx}})
    t0 = time.perf_counter()
    out = b.run(duration_ms)
    wall = time.perf_counter() - t0
    ais_spikes = (b.spike_counts[:, list(ais_nodes)].sum(1).tolist()
                  if ais_nodes else [0] * replicates)
    return {
        "wall_s": wall,
        "host_spikes": b.spike_counts[:, :b.n_base].sum(1).tolist(),
        "graft_spikes": (b.spike_counts[:, b.n_base:].sum(1).tolist()
                         if b.n > b.n_base else [0] * replicates),
        "ais_spikes": ais_spikes,
        "trace": b.get_trace(),
        "numerical_failure": out.get("numerical_failure"),
    }


def _compare_variants(profiles, seed, temperature_C, fitted_v11):
    """§11: A3.1 vs A3.2 metric table per profile."""
    table = {}
    for pid in profiles:
        row = {}
        for tag, variant, fitted in (
                ("v1_1", "v1_1", (fitted_v11 or {}).get(pid)),
                ("v1", "v1", None)):
            phys = single_node_phys(pid, fitted=fitted,
                                    variant=variant,
                                    temperature_C=temperature_C)
            m = eval_profile(phys, TARGETS.get(pid) or {}, seed=seed)
            fi = m["fi"]
            slope = TARGETS.get(pid, {}).get("metrics", {}).get(
                "fi_slope_hz_per_uAcm2", {}).get("mean")
            fi_rmse = (float(np.sqrt(np.mean(
                [(f["hz"] - slope * f["i_ext"]) ** 2
                 for f in fi]))) if slope else None)
            row[tag] = {
                "rest_V": round(m["rest_V"], 1),
                "rest_spikes": m["rest_spikes"],
                "rin_density": round(m["rin_density"], 2),
                "tau_ms": round(m["tau_ms"], 1),
                "rheobase": m["rheobase"],
                "fi_rmse": (round(fi_rmse, 1)
                            if fi_rmse is not None else None),
                "fi_hz": [f["hz"] for f in fi],
                "fi_amps": [f["i_ext"] for f in fi],
                "max_hz": max(f["hz"] for f in fi),
                "spike_width_ms": m["shape"]["spike_width_ms"],
                "ahp_mv": m["shape"]["ahp_mv"],
                "numerical_failure": m["rest_fail"],
            }
        table[pid] = row
    return table


def _ablation_v11(seed):
    """§12: para / shab / kca off on the v1_1 PN single cell."""
    base = single_node_phys("projection_neuron", variant="v1_1")
    out = {"i_ext": 25.0}
    for name in (None, "para_Na_v11", "shab_K_v11", "kca_K_v11"):
        phys = json.loads(json.dumps(base))
        tag = "full" if name is None else name + "_off"
        if name:
            phys["channels"][name]["g_bar"] = [0.0]
        from ..physio.benchmark import sim_single, _spike_times
        b = sim_single(phys, 400.0, i_ext=25.0, trace=True)
        tr = b.get_trace()[0]
        st = _spike_times(tr["v"], tr["t"])
        v = np.asarray(tr["v"])
        out[tag] = {"spikes": int(b.spike_counts[0, 0]),
                    "hz_after_100ms": float(
                        len(st[st > 100]) / 0.3),
                    "min_after_peak": float(v[np.argmax(v):].min())
                        if len(v) else None,
                    "fail": b._num_failure}
    out["para_abolished"] = out["para_Na_v11_off"]["spikes"] \
        <= out["full"]["spikes"] * 0.05
    out["kca_rate_changed"] = (
        out["kca_K_v11_off"]["hz_after_100ms"]
        > out["full"]["hz_after_100ms"] * 1.5)
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--store", required=True)
    ap.add_argument("--n-circuit", type=int, default=240)
    ap.add_argument("--duration-ms", type=float, default=300.0)
    ap.add_argument("--replicates", type=int, default=8)
    ap.add_argument("--seed", type=int, default=20260916)
    ap.add_argument("--drive-hz", type=float, default=30.0)
    ap.add_argument("--drive-g", type=float, default=1.5)
    ap.add_argument("--weight-in", type=float, default=30.0)
    ap.add_argument("--weight-out", type=float, default=200.0)
    ap.add_argument("--graft-p", type=float, default=1.0)
    ap.add_argument("--weight-to-g", type=float, default=0.02)
    ap.add_argument("--unknown-sign-mode", default="neutral")
    ap.add_argument("--calibrate-iters", type=int, default=10)
    # §17: acceptance floor is config, not a hidden constant —
    # effect must exceed this noise floor AND be direction-
    # consistent across every paired replicate
    ap.add_argument("--causal-noise-floor", type=float, default=1.0,
                    help="min |delta| in AIS spikes or host output "
                         "count, per replicate")
    ap.add_argument("--temperature-C", type=float, default=25.0)
    ap.add_argument("--overlay-out", default=None)
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    t0 = time.time()
    swc_dir = Path("data/cache/banc_888/split_swc")
    (manifest, ent, circuit, drivers, targets, reductions,
     split_sub) = _build_circuit(args.store, args.n_circuit,
                                 args.seed, swc_dir)

    # ---- calibrate v1_1 (>=3 profiles) -----------------------------
    calib, fitted = [], {}
    if args.calibrate_iters:
        for pid in CALIBRATE_PROFILES:
            r = calibrate_profile(pid, n_iter=args.calibrate_iters,
                                  seed=args.seed,
                                  temperature_C=args.temperature_C,
                                  variant="v1_1")
            calib.append(r)
            if r.get("status") == "fitted":
                fitted[pid] = r["fitted_params"]
            print(f"[a32] calib {pid}: status={r['status']} "
                  f"loss={r.get('final_loss')}")

    fitted_overlay = {pid: {k: {"value": v, "unit": FIT_UNITS[k],
                                "source": "calib-v1_1 fit",
                                "fit": {"seed": args.seed,
                                        "provenance":
                                        "MODEL_INFERENCE"}}
                            for k, v in fp.items()}
                      for pid, fp in fitted.items()}

    ov_dir = Path(args.overlay_out or "/tmp/a32_overlay")
    build_fly_overlay(ent[ent["entity_idx"].isin(circuit)], manifest,
                      ov_dir, fitted=fitted_overlay or None,
                      variant="v1_1")
    ov = load_fly_overlay(ov_dir, manifest)
    entity_params, receptor_maps = resolve_fly_entity_params(ov)
    nt_map = {int(r.entity_idx): str(getattr(r, "nt_top", "") or "")
              for r in ent.itertuples(index=False)}

    (n4, post, pre, w, esyn, coup4, rows4, man4, audit4, phys4) = \
        compile_reduced_graph_v4(
            split_sub, reductions, entity_params, receptor_maps,
            nt_map=nt_map, unknown_sign_mode=args.unknown_sign_mode,
            weight_to_g=args.weight_to_g, drive_g=args.drive_g,
            temperature_C=args.temperature_C)
    assert phys4["runtime_mode"] == "active_fly_v1_1"

    matched, comp_of = matched_targets(rows4, targets)
    soma_t = [comp_of[e]["SOMA"] for e in matched]
    dend_t = [comp_of[e]["DENDRITE_DIST"] for e in matched]
    ais_t = [comp_of[e][AIS] for e in matched if AIS in comp_of[e]]
    in_src = [comp_of[d]["SOMA"] for d in drivers if d in comp_of]
    # §14: per-target telemetry — one representative target's
    # dend/soma/AIS traced for the latency analysis (§15)
    probe = matched[0]
    trace_nodes = [comp_of[probe]["DENDRITE_DIST"],
                   comp_of[probe]["SOMA"]]
    if AIS in comp_of[probe]:
        trace_nodes.append(comp_of[probe][AIS])
    print(f"[a32] {len(circuit)} entities -> {n4} nodes "
          f"({man4['ais']['count']} AIS), {len(w)} edges; "
          f"matched {len(matched)}; probe entity {probe}")

    # ---- §11: A3.1 vs A3.2 single-cell table -----------------------
    cmp_table = _compare_variants(
        CALIBRATE_PROFILES, args.seed, args.temperature_C, fitted)

    # ---- §12: channel ablation on v1_1 -----------------------------
    ablation = _ablation_v11(args.seed)

    # ---- §13-§16: graft causal hardening ---------------------------
    base4 = (n4, post, pre, w, esyn)
    mode = "active_fly_v1_1"
    phen_full = {"physiology": phys4, **_graft_phen(
        10, in_src, dend_t, args.weight_in, args.weight_out, args.graft_p)}
    runs = {}
    runs["full"] = _run32(base4, coup4, phen_full, in_src,
                          args.drive_hz, args.duration_ms, args.seed,
                          args.replicates, mode, ais_nodes=ais_t,
                          trace_nodes=trace_nodes)
    for name, kw in (("efferent_disabled", {"enable_out": False}),
                     ("afferent_disabled", {"enable_in": False})):
        runs[name] = _run32(
            base4, coup4,
            {"physiology": phys4, **_graft_phen(
                10, in_src, dend_t, args.weight_in, args.weight_out,
                args.graft_p, **kw)},
            in_src, args.drive_hz, args.duration_ms, args.seed,
            args.replicates, mode, ais_nodes=ais_t,
            trace_nodes=trace_nodes)
    runs["graft_disabled"] = _run32(
        base4, coup4, {"physiology": phys4}, in_src, args.drive_hz,
        args.duration_ms, args.seed, args.replicates, mode,
        ais_nodes=ais_t)

    # §16-17: AIS spikes OR host output must differ, direction-
    # consistent across all paired replicates, effect > noise floor
    def _paired_effect(full_v, off_v):
        diffs = [f - o for f, o in zip(full_v, off_v)]
        consistent = all(d > args.causal_noise_floor for d in diffs) \
            or all(d < -args.causal_noise_floor for d in diffs)
        return diffs, consistent
    ais_d, ais_ok = _paired_effect(runs["full"]["ais_spikes"],
                                   runs["efferent_disabled"]
                                   ["ais_spikes"])
    out_d, out_ok = _paired_effect(runs["full"]["host_spikes"],
                                   runs["efferent_disabled"]
                                   ["host_spikes"])
    causal = ais_ok or out_ok

    again = _run32(base4, coup4, phen_full, in_src, args.drive_hz,
                   args.duration_ms, args.seed, args.replicates, mode,
                   ais_nodes=ais_t)
    deterministic = (again["host_spikes"] == runs["full"]["host_spikes"]
                     and again["ais_spikes"] == runs["full"]["ais_spikes"]
                     and again["graft_spikes"] ==
                     runs["full"]["graft_spikes"])

    # §15 latency: trace first-crossing times on the probe chain
    lat = {}
    tr = runs["full"]["trace"]
    for name, nd in zip(("dend", "soma", "ais"), trace_nodes):
        rec = tr.get(nd)
        if not rec:
            continue
        v = np.asarray(rec["v"]); t = np.asarray(rec["t"])
        pk = int(np.argmax(v))
        first = float(t[v > -20][0]) if (v > -20).any() else None
        lat[name] = {"first_over_-20": first,
                     "t_peak": float(t[pk]),
                     "v_peak": float(v[pk]),
                     "v_end": float(v[-5:].mean())}
    fail_count = sum(1 for r in runs.values()
                     if r["numerical_failure"])

    report = {
        "kind": "A3.2 physiology hardening + causal chain on G0.1",
        "run_manifest": {
            "anatomy_manifest_hash": manifest.get("manifest_hash"),
            "physiology_overlay": ov["manifest"],
            "reduction_algorithm": man4["algorithm"],
            "runtime_mode": phys4["runtime_mode"],
            "channel_model_versions": "flychan-v1_1",
            "unknown_sign_mode": args.unknown_sign_mode,
            "temperature_C": args.temperature_C,
            "causal_noise_floor": args.causal_noise_floor,
        },
        "comparison_v1_vs_v11": cmp_table,
        "channel_ablation_v11": ablation,
        "graft_conditions": {k: {kk: r[kk] for kk in
                                 ("host_spikes", "graft_spikes",
                                  "ais_spikes", "numerical_failure")}
                             for k, r in runs.items()},
        "causal": {"ais_spike_deltas": ais_d,
                   "host_output_deltas": out_d,
                   "ais_consistent": ais_ok, "output_consistent":
                   out_ok, "accepted": causal},
        "probe_latency": lat,
        "deterministic": deterministic,
        "calibration_v11": calib,
        "receptor_audit": man4["synapse_effect"],
        "numerical_failure_count": fail_count,
        "wall_s_total": time.time() - t0,
    }
    out = Path(args.out or "/tmp/a32.json")
    out.write_text(json.dumps(report, indent=1, default=str))
    print(f"[a32] causal={causal} (ais={ais_ok} out={out_ok}) "
          f"det={deterministic} fails={fail_count}")
    print(f"[a32] report -> {out}")
    ok = (causal and deterministic and fail_count == 0
          and ablation["para_abolished"])
    print(f"[a32] {'PASS' if ok else 'CHECK FAILED'}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
