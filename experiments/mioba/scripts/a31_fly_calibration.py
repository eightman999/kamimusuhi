"""A3.1 — Drosophila physiological calibration on the G0.1 circuit
(A3.1 §45-§60).

Same 240-entity host, same matched targets, same paired seeds as
G0.1/A3. What changes is physiology only:

    reduce-v3-ais-fly   AIS runtime nodes + receptor-aware sign
    active_fly_v1       Para/Shab/(Shaker|Shal) channels, Q10
    fly overlay v1      per-entity profile class + receptor map
    calib-v1            bounded seeded fit -> CALIBRATION_MANIFEST

Outputs: PHYSIOLOGY_FIDELITY v2, CALIBRATION_MANIFEST, receptor audit,
AIS causal telemetry, single-cell suite, channel ablation, report.
"""
from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

import numpy as np

from ..anatomy import compartments as C
from ..anatomy.provenance import Provenance
from ..physio.calibrate import (BOUNDS, TARGETS, calibrate_profile,
                                single_node_phys)
from ..physio.benchmark import (eval_profile, measure_fi,
                                measure_rest, sim_single)
from ..physio.compiler import compile_reduced_graph_v3
from ..physio.fly_compiler import AIS, compile_reduced_graph_v4
from ..physio.fly_overlay import (FLY_PROFILES, build_fly_overlay,
                                  load_fly_overlay,
                                  resolve_fly_entity_params)
from ..physio.overlay import (build_overlay, load_overlay,
                              resolve_entity_params)
from .a3_active_graft import _run
from .g01_compartment_graft import (_any_nonzero_delta, _build_circuit,
                                    _graft_phen, _tel_delta,
                                    matched_targets)

#: §60: at least three profile families carry quantitative fits
CALIBRATE_PROFILES = ("kenyon_cell", "projection_neuron",
                      "descending", "motor")
FIT_UNITS = {"g_para": "mS/cm2", "g_shab": "mS/cm2",
             "g_shal": "mS/cm2", "g_shaker": "mS/cm2",
             "g_leak": "mS/cm2", "E_leak": "mV", "Cm": "uF/cm2"}


def _fidelity_v2(params, receptors, phys, audit, n_nodes, manifest,
                 calib_results):
    """§57 PHYSIOLOGY_FIDELITY v2."""
    prov_count: dict[str, int] = {}
    ch_entities: dict[str, int] = {}
    prof_count: dict[str, int] = {}
    for ep in params.values():
        prof_count[ep["profile_id"]] = \
            prof_count.get(ep["profile_id"], 0) + 1
        for rec in ep["membrane"].values():
            p = str(rec["provenance"])
            prov_count[p] = prov_count.get(p, 0) + 1
        for ch in ep["channels"]:
            prov_count[str(ch["provenance"])] = \
                prov_count.get(str(ch["provenance"]), 0) + 1
            ch_entities[ch["channel"]] = \
                ch_entities.get(ch["channel"], 0) + 1
    n_rec_unknown = sum(1 for r in receptors.values()
                        if set(r) <= {"acetylcholine", "gaba"})
    syn = manifest["synapse_effect"]
    return {
        "fidelity_version": 2,
        "entities_total": len(params),
        "entities_active_mode": len(params),
        "entities_passive_mode": 0,
        "compartments_active": len(phys["active_idx"]),
        "compartments_passive": n_nodes - len(phys["active_idx"]),
        "entities_with_fly_profile": sum(
            v for k, v in prof_count.items() if k != "generic_fly"),
        "entities_generic_fly_fallback": prof_count.get(
            "generic_fly", 0),
        "entities_unknown_profile": 0,
        "profile_class_counts": prof_count,
        "entities_with_Para": ch_entities.get("para_Na", 0),
        "entities_with_Shab": ch_entities.get("shab_K", 0),
        "entities_with_Shaker": ch_entities.get("shaker_K", 0),
        "entities_with_Shal": ch_entities.get("shal_K", 0),
        "AIS_entities": manifest["ais"]["count"],
        "AIS_model_inferred": manifest["ais"]["count"],
        "receptor_profiles_resolved": len(receptors) - n_rec_unknown,
        "receptor_profiles_unknown": n_rec_unknown,
        "calibration_targets_total": sum(
            len(t["metrics"]) for t in TARGETS.values()),
        "calibration_targets_adult": sum(
            len(t["metrics"]) for t in TARGETS.values()
            if not t.get("cross_stage_prior")),
        "calibration_targets_cross_stage": sum(
            len(t["metrics"]) for t in TARGETS.values()
            if t.get("cross_stage_prior")),
        "profiles_fitted": [r["profile_id"] for r in calib_results
                            if r.get("status") == "fitted"],
        "parameters_direct": prov_count.get("DIRECT_MEASUREMENT", 0),
        "parameters_cell_type":
            prov_count.get("CELL_TYPE_MEASUREMENT", 0),
        "parameters_literature":
            prov_count.get("LITERATURE_PRIOR", 0),
        "parameters_fitted": sum(
            len(r.get("fitted_params") or {})
            for r in calib_results if r.get("status") == "fitted"),
        "parameters_model_inference":
            prov_count.get("MODEL_INFERENCE", 0),
        "parameters_generic_fallback":
            prov_count.get("GENERIC_FALLBACK", 0),
        "parameters_unknown": prov_count.get("UNKNOWN", 0),
        "channels_by_type": ch_entities,
        "synapses_conductance_based": syn["synapses_emitted"],
        "synapses_weight_only": 0,
        "synapses_receptor_resolved": syn["synapses_receptor_resolved"],
        "synapses_unknown_sign": syn["synapses_unknown_sign"],
        "synapses_using_prior": syn["synapses_using_prior"],
        "synapses_dropped_strict": syn["synapses_dropped_strict"],
    }


def _channel_ablation(seed: int) -> dict:
    """§25-§26: single-cell ablation on the projection_neuron
    profile — Para off must strongly reduce/abolish spikes; Shab off
    must change repolarisation/AHP."""
    base = single_node_phys("projection_neuron")
    # above rheobase but below depolarisation block, so a spike's
    # undershoot can actually reach back toward rest (§56 check)
    i_inj = 25.0
    full = sim_single(base, 150.0, i_ext=i_inj, trace=True, seed=seed)
    out = {"i_ext": i_inj,
           "full_spikes": int(full.spike_counts[0, 0]),
           "full_fail": full._num_failure}
    v_full = np.asarray(full.get_trace()[0]["v"])
    out["full_min_after_peak"] = float(v_full[
        np.argmax(v_full):].min())

    for name in ("para_Na", "shab_K"):
        phys = json.loads(json.dumps(base))
        phys["channels"][name]["g_bar"] = [0.0]
        b = sim_single(phys, 150.0, i_ext=i_inj, trace=True, seed=seed)
        tr = b.get_trace()[0]
        v = np.asarray(tr["v"])
        out[name + "_off"] = {
            "spikes": int(b.spike_counts[0, 0]),
            "fail": b._num_failure,
            "min_after_peak": float(v[np.argmax(v):].min())
                if len(v) else None,
        }
    out["para_abolished"] = out["para_Na_off"]["spikes"] \
        < out["full_spikes"] * 0.2
    out["shab_repol_changed"] = (
        out["shab_K_off"]["min_after_peak"]
        > out["full_min_after_peak"] + 3.0)
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
    ap.add_argument("--weight-out", type=float, default=80.0)
    ap.add_argument("--weight-to-g", type=float, default=0.02)
    ap.add_argument("--unknown-sign-mode", default="neutral",
                    choices=("neutral", "prior", "strict", "drop"))
    ap.add_argument("--calibrate-iters", type=int, default=8)
    ap.add_argument("--temperature-C", type=float, default=25.0)
    ap.add_argument("--overlay-out", default=None)
    ap.add_argument("--fidelity-out", default=None)
    ap.add_argument("--calibration-out", default=None)
    ap.add_argument("--trace-out", default=None)
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    t0 = time.time()
    swc_dir = Path("data/cache/banc_888/split_swc")
    (manifest, ent, circuit, drivers, targets, reductions,
     split_sub) = _build_circuit(args.store, args.n_circuit,
                                 args.seed, swc_dir)

    # ---- §16-§24: bounded calibration of >=3 profiles --------------
    calib = []
    fitted_overlay = {}
    if args.calibrate_iters:
        for pid in CALIBRATE_PROFILES:
            r = calibrate_profile(pid, n_iter=args.calibrate_iters,
                                  seed=args.seed,
                                  temperature_C=args.temperature_C)
            calib.append(r)
            if r.get("status") == "fitted":
                fitted_overlay[pid] = {
                    k: {"value": v, "unit": FIT_UNITS[k],
                        "source": "calib-v1 fit to " + r[
                            "source_measurements"],
                        "fit": {"loss": r["final_loss"], "seed":
                                args.seed,
                                "provenance": "MODEL_INFERENCE"}}
                    for k, v in r["fitted_params"].items()}
            print(f"[a31] calib {pid}: status={r['status']} "
                  f"loss={r.get('final_loss')}")

    # ---- fly overlay on THIS circuit's entities --------------------
    ov_dir = Path(args.overlay_out or "/tmp/a31_fly_overlay")
    build_fly_overlay(ent[ent["entity_idx"].isin(circuit)], manifest,
                      ov_dir, fitted=fitted_overlay or None)
    ov = load_fly_overlay(ov_dir, manifest)
    entity_params, receptor_maps = resolve_fly_entity_params(ov)
    nt_map = {int(r.entity_idx): str(getattr(r, "nt_top", "") or "")
              for r in ent.itertuples(index=False)}

    # ---- compile: v4 fly graph (+ v3 generic for the A3 compare) ---
    (n4, post, pre, w, esyn, coup4, rows4, man4, audit4, phys4) = \
        compile_reduced_graph_v4(
            split_sub, reductions, entity_params, receptor_maps,
            nt_map=nt_map, unknown_sign_mode=args.unknown_sign_mode,
            weight_to_g=args.weight_to_g, drive_g=args.drive_g,
            temperature_C=args.temperature_C)

    # A3 generic comparison on the SAME anatomy (v3 compiler, generic
    # overlay) — §45
    ov3_dir = Path("/tmp/a31_generic_overlay")
    build_overlay(ent[ent["entity_idx"].isin(circuit)], manifest,
                  ov3_dir)
    ov3 = load_overlay(ov3_dir, manifest)
    ep3 = resolve_entity_params(ov3)
    (n3, p3, r3, w3, e3, coup3, rows3, man3, audit3, phys3) = \
        compile_reduced_graph_v3(split_sub, reductions, ep3,
                                 nt_map=nt_map,
                                 weight_to_g=args.weight_to_g,
                                 drive_g=args.drive_g)
    (n2, post2, pre2, w2, coup2, rows2, man2, _a2) = \
        C.compile_reduced_graph_v2(split_sub, reductions)

    matched, comp_of = matched_targets(rows4, targets)
    soma_t = [comp_of[e]["SOMA"] for e in matched]
    dend_t = [comp_of[e]["DENDRITE_DIST"] for e in matched]
    ais_t = [comp_of[e][AIS] for e in matched if AIS in comp_of[e]]
    in_src = [comp_of[d]["SOMA"] for d in drivers if d in comp_of]
    tel_nodes = dend_t + soma_t + ais_t     # §49: dend+soma+AIS
    # v4 inserts AIS nodes → its node indices diverge from the v2
    # (passive) map; the L2 benchmark needs v2-indexed drive/tel
    _, comp_of2 = matched_targets(rows2, targets)
    soma_t2 = [comp_of2[e]["SOMA"] for e in matched]
    dend_t2 = [comp_of2[e]["DENDRITE_DIST"] for e in matched]
    in_src2 = [comp_of2[d]["SOMA"] for d in drivers if d in comp_of2]
    tel_nodes2 = dend_t2 + soma_t2
    print(f"[a31] circuit {len(circuit)} entities -> {n4} nodes "
          f"({len(ais_t)} matched AIS), {len(w)} edges; "
          f"matched {len(matched)}")
    if not matched:
        print("[a31] FAIL: no matched targets")
        return 1

    # ---- §54 benchmark: L2 passive / A3 generic / A3.1 fly ---------
    conn_sub = split_sub.groupby(["pre_idx", "post_idx"],
                                 as_index=False)["anatomical_count"]\
        .sum().rename(columns={"anatomical_count": "weight"})
    n1, p1, r1, w1, c1, rows1, man1 = C.compile_reduced_graph(
        conn_sub, {})
    soma1 = {r["entity_idx"]: r["runtime_idx"] for r in rows1}
    dr1 = [soma1[d] for d in drivers if d in soma1]
    soma_t3 = [next(r["runtime_idx"] for r in rows3
                    if r["entity_idx"] == e
                    and r["compartment"] == "SOMA") for e in matched]
    in_src3 = [next(r["runtime_idx"] for r in rows3
                    if r["entity_idx"] == d
                    and r["compartment"] == "SOMA")
               for d in drivers if any(
                   r["entity_idx"] == d and r["compartment"] == "SOMA"
                   for r in rows3)]

    bench = {}
    for name, base, coup, dr, mode, ph, tel in (
            ("L2_passive", (n2, post2, pre2, w2), coup2, in_src2,
             "passive_lif", None, tel_nodes2),
            ("A3_generic", (n3, p3, r3, w3, e3), coup3, in_src3,
             "active_hh_v0", {"physiology": phys3},
             soma_t3 + [next(r["runtime_idx"] for r in rows3
                             if r["entity_idx"] == e
                             and r["compartment"] == "DENDRITE_DIST")
                        for e in matched]),
            ("A31_fly", (n4, post, pre, w, esyn), coup4,
             in_src, "active_fly_v1", {"physiology": phys4},
             tel_nodes)):
        res = _run(base, coup, ph, dr, args.drive_hz,
                   args.duration_ms, args.seed, args.replicates, mode,
                   tel)
        bench[name] = {
            "wall_s": res["wall_s"],
            "steps_per_s": res["steps"] / max(res["wall_s"], 1e-9),
            "spikes_per_s": (sum(res["host_spikes"])
                             / max(res["wall_s"], 1e-9)),
            "host_spikes": res["host_spikes"],
            "state_mem_mb": res["state_mem_mb"],
            "n_nodes": base[0], "n_edges": int(len(base[3])),
            "numerical_failure": res["numerical_failure"],
        }
        print(f"[a31] {name}: wall={res['wall_s']:.2f}s "
              f"spikes={sum(res['host_spikes'])} "
              f"fail={res['numerical_failure']}")

    # ---- §45-§50: graft on the fly host, paired ablation -----------
    base4 = (n4, post, pre, w, esyn)
    phen_full = {"physiology": phys4, **_graft_phen(
        10, in_src, dend_t, args.weight_in, args.weight_out, 0.5)}
    graft = {}
    graft["full"] = _run(base4, coup4, phen_full, in_src,
                         args.drive_hz, args.duration_ms, args.seed,
                         args.replicates, "active_fly_v1", tel_nodes)
    for name, kw in (("efferent_disabled", {"enable_out": False}),
                     ("afferent_disabled", {"enable_in": False})):
        graft[name] = _run(
            base4, coup4,
            {"physiology": phys4, **_graft_phen(
                10, in_src, dend_t, args.weight_in, args.weight_out,
                0.5, **kw)},
            in_src, args.drive_hz, args.duration_ms, args.seed,
            args.replicates, "active_fly_v1", tel_nodes)
    graft["graft_disabled"] = _run(
        base4, coup4, {"physiology": phys4}, in_src, args.drive_hz,
        args.duration_ms, args.seed, args.replicates, "active_fly_v1",
        tel_nodes)

    deltas = _tel_delta(graft["full"]["telemetry"],
                        graft["efferent_disabled"]["telemetry"])
    causal = _any_nonzero_delta(deltas)
    again = _run(base4, coup4, phen_full, in_src, args.drive_hz,
                 args.duration_ms, args.seed, args.replicates,
                 "active_fly_v1", tel_nodes)
    deterministic = (
        again["host_spikes"] == graft["full"]["host_spikes"]
        and again["graft_spikes"] == graft["full"]["graft_spikes"])

    # AIS telemetry split (§49): which telemetry rows are AIS nodes
    n_tel_split = len(dend_t) + len(soma_t)
    tel_full = graft["full"]["telemetry"] or {"nodes": []}
    ais_tel = [nd for nd in tel_full["nodes"]
               if nd["runtime_idx"] in set(ais_t)]

    # ---- §39-§43: single-cell suite + §25 channel ablation ---------
    suite = {}
    for pid in ("kenyon_cell", "projection_neuron", "motor"):
        phys1 = single_node_phys(
            pid, fitted=(calib and next(
                (c.get("fitted_params") for c in calib
                 if c["profile_id"] == pid
                 and c.get("status") == "fitted"), None)),
            temperature_C=args.temperature_C)
        suite[pid] = eval_profile(phys1, TARGETS.get(pid) or {},
                                  seed=args.seed)
    ablation = _channel_ablation(args.seed)

    # §56 model-validity checks
    profile_sigs = {pid: (round(s["rest_V"], 1),
                          round(s["rheobase"] or -1, 1))
                    for pid, s in suite.items()}
    validity = {
        "no_rest_spontaneous_firing": all(
            s["rest_spikes"] == 0 for s in suite.values()),
        "no_rest_numerical_failure": not any(
            s["rest_fail"] for s in suite.values()),
        "profiles_not_identical": len(set(profile_sigs.values())) > 1,
        "para_removal_effect": bool(ablation["para_abolished"]),
        "repolarisation_present": bool(
            ablation["full_min_after_peak"] < -55.0),
    }

    # ---- §57-§58 manifests -----------------------------------------
    fidelity = _fidelity_v2(entity_params, receptor_maps, phys4,
                            audit4, n4, man4, calib)
    fidelity["numerical_failure_count"] = sum(
        1 for r in (*bench.values(), *graft.values())
        if r.get("numerical_failure"))
    if args.fidelity_out:
        Path(args.fidelity_out).write_text(json.dumps(fidelity,
                                                    indent=1))

    calib_manifest = {
        "calibration_version": "calib-v1",
        "source_papers": {pid: TARGETS[pid]["source"]
                          for pid in TARGETS},
        "profile_targets": TARGETS,
        "optimizer": (calib[0]["optimizer"] if calib else None),
        "parameter_bounds": BOUNDS,
        "results": calib,
    }
    if args.calibration_out:
        Path(args.calibration_out).write_text(
            json.dumps(calib_manifest, indent=1, default=str))

    # §41: representative voltage trace artifact (kenyon_cell)
    if args.trace_out:
        phys1 = single_node_phys(
            "kenyon_cell",
            fitted=(fitted_overlay.get("kenyon_cell") and {
                k: v["value"] for k, v in
                fitted_overlay["kenyon_cell"].items()}),
            temperature_C=args.temperature_C)
        b = sim_single(phys1, 200.0, i_ext=40.0, trace=True,
                       seed=args.seed)
        Path(args.trace_out).write_text(
            json.dumps(b.get_trace(), indent=0))

    report = {
        "kind": "A3.1 Drosophila physiological calibration on G0.1",
        "run_manifest": {
            "anatomy_manifest_hash": manifest.get("manifest_hash"),
            "physiology_overlay": ov["manifest"],
            "reduction_algorithm": man4["algorithm"],
            "runtime_fidelity": "L3+",
            "runtime_mode": "active_fly_v1",
            "dt": phys4["integration"]["dt"],
            "temperature_C": args.temperature_C,
            "channel_model_versions": "flychan-v1",
            "synapse_model_version": "receptor_aware_v0",
            "unknown_sign_mode": args.unknown_sign_mode,
        },
        "matched_target_entities": matched,
        "benchmark": bench,
        "graft_conditions": {k: {kk: r[kk] for kk in
                                 ("host_spikes", "graft_spikes",
                                  "numerical_failure")}
                             for k, r in graft.items()},
        "graft_telemetry_deltas": deltas,
        "ais_telemetry_nodes": len(ais_tel),
        "graft_causal": {
            "host_to_graft": all(
                s > 0 for s in graft["full"]["graft_spikes"]),
            "graft_to_target_compartment": bool(causal),
        },
        "deterministic": deterministic,
        "single_cell_suite": {p: {k: v for k, v in s.items()
                                  if k != "trace"}
                              for p, s in suite.items()},
        "channel_ablation": ablation,
        "model_validity": validity,
        "receptor_audit": man4["synapse_effect"],
        "ais_manifest": {"count": man4["ais"]["count"],
                         "sample": man4["ais"]["rows"][:5]},
        "fidelity": fidelity,
        "calibration": calib,
        "wall_s_total": time.time() - t0,
    }
    out = Path(args.out or "/tmp/a31.json")
    out.write_text(json.dumps(report, indent=1, default=str))
    print(f"[a31] graft causal={causal} det={deterministic} "
          f"fail_count={fidelity['numerical_failure_count']}")
    print(f"[a31] validity={validity}")
    print(f"[a31] report -> {out}")
    ok = (causal and deterministic
          and fidelity["numerical_failure_count"] == 0
          and all(validity.values()))
    print(f"[a31] {'PASS' if ok else 'CHECK FAILED'}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
