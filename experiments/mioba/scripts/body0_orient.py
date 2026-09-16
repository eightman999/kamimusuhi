"""Body0 — closed sensorimotor loop on real BANC anatomy
(Body0 §20-§56).

    env → bilateral sensory port (afferent/sensory super_class,
          side-split, CURATED_ANNOTATION) → BANC CNS
          (active_fly_v1_1) → descending/motor port (side-split) →
          2D unicycle body → env

Conditions (§35-§38): oracle baseline, host_only, host+graft,
graft efferent/afferent disabled, sensory ablation, motor ablation.
Paired seeds + deterministic rerun. No sensor→body or graft→body
shortcut — every path flows through the audited ports (§45).

Artifacts (§55): body0_report.json, episode_traces/*.jsonl,
BODY_FIDELITY.json, port_manifest.json.
"""
from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

from ..anatomy import compartments as C
from ..anatomy import morphology as M
from ..anatomy import schema as A
from ..body.body0 import (BODY0_VERSION, Environment,
                          pick_sensorimotor_circuit, port_manifest,
                          run_episode, run_oracle, select_ports)
from ..fba.replicates import replicate_seeds
from ..fba.torch_backend import TorchBackend
from ..physio.fly_compiler import AIS, compile_reduced_graph_v4
from ..physio.fly_overlay import (build_fly_overlay, load_fly_overlay,
                                  resolve_fly_entity_params)
from .g01_compartment_graft import (_graft_phen, matched_targets)


def _build_sm_circuit(store: str, n_circuit: int, seed: int,
                    swc_dir: Path, depth: int, n_motor: int):
    """Like _build_circuit but the entity set is connectivity-informed
    (pick_sensorimotor_circuit) so real sensory→descending paths are
    present — a random 240-cell subgraph had none (Body0 §28)."""
    manifest, ent, conn = A.load_store_v2(store)
    split = A.load_layer(store, "connections_split")
    picked = pick_sensorimotor_circuit(ent, conn, n_circuit, seed,
                                       depth=depth, n_motor=n_motor)
    circuit = picked["circuit"]
    rids = [str(ent.loc[ent["entity_idx"] == i, "root_id"].iloc[0])
            for i in circuit]
    M.fetch_split_swcs(rids, swc_dir)
    e2r = {int(e): str(r) for e, r in zip(ent["entity_idx"],
                                          ent["root_id"])}
    reductions = {}
    for ei in circuit:
        p = swc_dir / f"{e2r[ei]}_split.swc"
        if p.is_file() and p.stat().st_size > 0:
            reductions[ei] = C.reduce_entity(ei, M.parse_swc(p))
    cset = set(circuit)
    split_sub = split[split["pre_idx"].isin(cset)
                      & split["post_idx"].isin(cset)].copy()
    return (manifest, ent, conn, circuit, picked, reductions,
            split_sub)


def _make_backend(base, coup, phen, mode, seed):
    b = TorchBackend(synthetic=False, base_override=base,
                     voltage_coupling=coup, runtime_mode=mode)
    p = {"artificial_organs": [], "attachments": [],
         "n_extra_neurons": 0, "params": {}}
    if phen:
        p.update(phen)
    b.initialize(p, batch_size=1, seed=seed, device="cpu",
                 replicate_seeds=replicate_seeds(seed, 1))
    return b


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--store", required=True)
    ap.add_argument("--n-circuit", type=int, default=240)
    ap.add_argument("--episode-s", type=float, default=4.0)
    ap.add_argument("--tick-ms", type=float, default=20.0)
    ap.add_argument("--seeds", default="20260916,20260917")
    ap.add_argument("--initial-heading", type=float, default=1.2)
    ap.add_argument("--sensory-gain-hz", type=float, default=200.0)
    ap.add_argument("--sensory-base-hz", type=float, default=5.0)
    ap.add_argument("--k-ang", type=float, default=0.05)
    ap.add_argument("--k-lin", type=float, default=0.02)
    ap.add_argument("--decode-window-s", type=float, default=0.1)
    ap.add_argument("--weight-in", type=float, default=30.0)
    ap.add_argument("--weight-out", type=float, default=200.0)
    ap.add_argument("--graft-p", type=float, default=1.0)
    ap.add_argument("--weight-to-g", type=float, default=5.0,
                    help="synapse-count→conductance scale; the "
                         "calibrated network needs ~5 for "
                         "measurable sensory→motor conduction "
                         "(MODEL_INFERENCE free parameter)")
    ap.add_argument("--drive-g", type=float, default=1.5)
    ap.add_argument("--unknown-sign-mode", default="neutral")
    ap.add_argument("--temperature-C", type=float, default=25.0)
    ap.add_argument("--path-depth", type=int, default=3)
    ap.add_argument("--n-motor", type=int, default=8)
    ap.add_argument("--calib-report", default=None,
                    help="a32_report.json — reuse its fitted_params "
                         "so the body0 circuit gets the calibrated "
                         "v1_1 densities (per-profile, not "
                         "per-entity)")
    ap.add_argument("--out-dir", default="docs/experiments/body0")
    args = ap.parse_args()

    t0 = time.time()
    seeds = [int(s) for s in args.seeds.split(",")]
    swc_dir = Path("data/cache/banc_888/split_swc")
    (manifest, ent, conn, circuit, picked, reductions,
     split_sub) = _build_sm_circuit(args.store, args.n_circuit,
                                    seeds[0], swc_dir,
                                    args.path_depth, args.n_motor)
    drivers = picked["drivers"]
    targets = picked["targets"]

    fitted_overlay = None
    if args.calib_report:
        from .a31_fly_calibration import FIT_UNITS
        rep = json.loads(Path(args.calib_report).read_text())
        fitted_overlay = {
            c["profile_id"]: {k: {"value": v, "unit": FIT_UNITS[k],
                               "source": "a32 calib-v1_1 fit",
                               "fit": {"seed": seeds[0],
                                       "provenance":
                                       "MODEL_INFERENCE"}}
                           for k, v in c["fitted_params"].items()}
            for c in rep["calibration_v11"]
            if c.get("status") == "fitted"}
    ov_dir = Path(args.out_dir) / "fly_overlay_v11"
    build_fly_overlay(ent[ent["entity_idx"].isin(circuit)],
                      manifest, ov_dir, fitted=fitted_overlay,
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
    base4 = (n4, post, pre, w, esyn)
    mode = phys4["runtime_mode"]

    matched, comp_of = matched_targets(rows4, targets)
    dend_t = [comp_of[e]["DENDRITE_DIST"] for e in matched]
    in_src = [comp_of[d]["SOMA"] for d in drivers if d in comp_of]
    ports = select_ports(ent, circuit, comp_of, ais_name=AIS,
                         picked=picked)
    pman = port_manifest(ports)
    pman["circuit_selection"] = picked["selection"]
    pman["n_path_entities"] = picked["n_path_entities"]
    pman["graft_entities"] = ["graft:g001 (10 LIF neurons)"]
    pman["artificial_links"] = ["g001:in0 host->graft",
                                "g001:out0 graft->host"]

    # §45 audit: the sensory→motor path must exist in the *runtime*
    # graph, not just the entity-level annotation graph
    import numpy as np
    from collections import deque
    adj = [[] for _ in range(n4)]
    for s_, d_ in zip(pre, post):
        adj[int(s_)].append(int(d_))
    # intra-entity compartment coupling lives in `coup4`, not the
    # synaptic edge list — signal must travel SOMA→AIS→axon to reach
    # the next entity, so include coupling both directions
    crow, ccol, _cv = coup4
    for a_, b_ in zip(crow.tolist(), ccol.tolist()):
        adj[int(a_)].append(int(b_))
        adj[int(b_)].append(int(a_))

    def _reach(src):
        seen = np.zeros(n4, bool)
        q = deque(src)
        for s_ in src:
            seen[s_] = True
        while q:
            u = q.popleft()
            for v in adj[u]:
                if not seen[v]:
                    seen[v] = True
                    q.append(v)
        return seen

    rL = _reach(ports["sensory_left"])
    rR = _reach(ports["sensory_right"])
    reach_audit = {
        "sensL_to_motL": int(rL[ports["motor_left"]].sum())
        if ports["motor_left"] else 0,
        "sensL_to_motR": int(rL[ports["motor_right"]].sum())
        if ports["motor_right"] else 0,
        "sensR_to_motL": int(rR[ports["motor_left"]].sum())
        if ports["motor_left"] else 0,
        "sensR_to_motR": int(rR[ports["motor_right"]].sum())
        if ports["motor_right"] else 0,
    }
    pman["runtime_reachability"] = reach_audit
    print(f"[body0] runtime reach: {reach_audit}")
    print(f"[body0] ports: sensL={len(ports['sensory_left'])} "
          f"sensR={len(ports['sensory_right'])} "
          f"motL={len(ports['motor_left'])} "
          f"motR={len(ports['motor_right'])}")

    env = Environment(target=(10.0, 0.0), boundary_r=30.0,
                      relocations=((2.0, (0.0, 10.0)),))
    graft = _graft_phen(10, in_src, dend_t, args.weight_in,
                        args.weight_out, args.graft_p)

    out_dir = Path(args.out_dir)
    (out_dir / "episode_traces").mkdir(parents=True, exist_ok=True)

    def cns_episode(phen, seed, **kw):
        from ..body.body0 import measure_motor_baseline
        b = _make_backend(base4, coup4, phen, mode, seed)
        bl, br = measure_motor_baseline(b, ports)
        r = run_episode(b, ports, env, args.episode_s,
                        tick_ms=args.tick_ms,
                        sensory_gain_hz=args.sensory_gain_hz,
                        sensory_base_hz=args.sensory_base_hz,
                        k_ang=args.k_ang, k_lin=args.k_lin,
                        decode_window_s=args.decode_window_s,
                        motor_baseline=(bl, br),
                        initial_heading=args.initial_heading, **kw)
        r["motor_baseline_hz"] = (bl, br)
        return r

    results = {"oracle": run_oracle(env, args.episode_s,
                                    tick_ms=args.tick_ms,
                                    initial_heading=
                                    args.initial_heading)}
    conds = {
        "host_only": ({"physiology": phys4}, {}),
        "host_graft": ({"physiology": phys4, **graft}, {}),
        "graft_efferent_disabled": (
            {"physiology": phys4, **_graft_phen(
                10, in_src, dend_t, args.weight_in, args.weight_out,
                args.graft_p, enable_out=False)}, {}),
        "graft_afferent_disabled": (
            {"physiology": phys4, **_graft_phen(
                10, in_src, dend_t, args.weight_in, args.weight_out,
                args.graft_p, enable_in=False)}, {}),
        "sensory_ablation": ({"physiology": phys4, **graft},
                             {"sensory_ablation": True}),
        "motor_ablation": ({"physiology": phys4, **graft},
                           {"motor_ablation": True}),
    }
    per_seed = {}
    for name, (phen, kw) in conds.items():
        per_seed[name] = {}
        for s in seeds:
            r = cns_episode(phen, s, **kw)
            per_seed[name][str(s)] = r["metrics"]
            if r["trace"]:
                tp = (out_dir / "episode_traces"
                      / f"{name}_s{s}.jsonl")
                tp.write_text("\n".join(
                    json.dumps({k: v[i] for k, v in
                                r["trace"].items()})
                    for i in range(len(r["trace"]["t"]))))
            print(f"[body0] {name} seed={s}: "
                  f"err={r['metrics']['mean_abs_heading_err_rad_tail']:.3f} "
                  f"motion@{r['metrics']['first_motion_latency_s']}")
        results[name] = per_seed[name]

    # §33: determinism — rerun host_graft with seed[0], compare
    again = cns_episode({"physiology": phys4, **graft}, seeds[0])
    det_ref = per_seed["host_graft"][str(seeds[0])]
    deterministic = (again["metrics"] == det_ref)

    # §42-43 causal checks: ablations must change behavior
    def _changed(cond, base="host_graft"):
        diffs = []
        for s in seeds:
            a = per_seed[cond][str(s)]["mean_abs_heading_err_rad_tail"]
            b = per_seed[base][str(s)]["mean_abs_heading_err_rad_tail"]
            diffs.append(a - b)
        return diffs, all(d > 0.01 for d in diffs) or \
            all(d < -0.01 for d in diffs)

    sens_d, sens_changed = _changed("sensory_ablation")
    mot_d, mot_changed = _changed("motor_ablation")
    # motor ablation: body must stop responding
    mot_motion_lost = all(
        per_seed["motor_ablation"][str(s)]["first_motion_latency_s"]
        is None for s in seeds)

    report = {
        "kind": "Body0 closed sensorimotor loop (BANC host + G0.1 "
                "graft on active_fly_v1_1)",
        "version": BODY0_VERSION,
        "run_manifest": {
            "anatomy_manifest_hash": manifest.get("manifest_hash"),
            "runtime_mode": mode,
            "reduction_algorithm": man4["algorithm"],
            "n_entities": len(circuit),
            "n_nodes": n4,
            "episode_s": args.episode_s,
            "tick_ms": args.tick_ms,
            "seeds": seeds,
            # §51: which entities are L3-active vs passive
            "l3_active_nodes": len(phys4["active_idx"]),
            "l2_passive_nodes": n4 - len(phys4["active_idx"]),
        },
        "port_manifest": pman,
        "environment": {"target": env.target, "boundary_r":
                        env.boundary_r, "relocations":
                        env.relocations},
        "conditions": results,
        "causality": {
            "sensory_ablation_deltas": sens_d,
            "sensory_ablation_changes_behavior": sens_changed,
            "motor_ablation_deltas": mot_d,
            "motor_ablation_changes_behavior": mot_changed,
            "motor_ablation_motion_lost": mot_motion_lost,
        },
        "deterministic": deterministic,
        "wall_s_total": time.time() - t0,
    }
    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / "port_manifest.json").write_text(
        json.dumps(pman, indent=1))
    (out_dir / "BODY_FIDELITY.json").write_text(json.dumps({
        "body_model": "MODEL_INFERENCE",
        "actuator_decode": "MODEL_INFERENCE",
        "sensory_transduction": "MODEL_INFERENCE",
        "host_sensory_identity": "CURATED_ANNOTATION",
        "host_motor_identity": "CURATED_ANNOTATION",
        "biomechanics": "simplified MODEL_INFERENCE",
        "environment": "MODEL_INFERENCE",
        "host_anatomy": "DIRECT_MEASUREMENT (BANC, immutable)",
        "host_physiology": "flychan-v1_1 (kinetics "
                           "LITERATURE_PRIOR; densities "
                           "MODEL_INFERENCE fit)",
        "graft": "ARTIFICIAL_GRAFT",
    }, indent=1))
    rp = out_dir / "body0_report.json"
    rp.write_text(json.dumps(report, indent=1, default=str))
    loop_ok = (det_ref["first_motion_latency_s"] is not None
               and det_ref["first_motor_spike_s"] is not None)
    paths_ok = (reach_audit["sensL_to_motL"] > 0
                and reach_audit["sensR_to_motR"] > 0)
    ok = (deterministic and loop_ok and paths_ok
          and mot_motion_lost and (sens_changed or mot_changed))
    print(f"[body0] paths_ok={paths_ok}")
    print(f"[body0] loop_closed={loop_ok} det={deterministic} "
          f"sens_abl={sens_changed} mot_abl={mot_changed}/"
          f"motion_lost={mot_motion_lost}")
    print(f"[body0] report -> {rp}")
    print(f"[body0] {'PASS' if ok else 'CHECK FAILED'}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
