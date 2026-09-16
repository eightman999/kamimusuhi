"""A3 — L1/L2/L3 fidelity comparison + graft re-validation on the
G0.1 circuit (A3 §26–§35, §46–§47).

Same host circuit, matched targets, paired seeds as G0.1:

    L1  point-LIF (compile_reduced_graph, no reductions)
    L2  passive reduced compartment (reduce-v1-split-synapse)
    L3  active reduced compartment (reduce-v2-biophysical +
        active_hh_v0 runtime + physiology overlay)

Graft (10 LIF neurons, unchanged) is re-attached to the active host
(§33): graft->DENDRITE_DIST, paired full vs efferent_disabled +
graft_disabled + afferent_disabled (§35).
"""
from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

import numpy as np

from ..anatomy import schema as A
from ..anatomy import compartments as C
from ..anatomy.provenance import Provenance
from ..physio.compiler import compile_reduced_graph_v3
from ..physio.overlay import (build_overlay, load_overlay,
                              resolve_entity_params)
from .g01_compartment_graft import (_any_nonzero_delta, _build_circuit,
                                    _graft_phen, _tel_delta,
                                    matched_targets)


def _run(base, coup, phen, drive_idx, drive_hz, duration_ms, seed,
         replicates, mode="passive_lif", tel_nodes=()):
    from ..fba.torch_backend import TorchBackend
    from ..fba.replicates import replicate_seeds
    ov = base if len(base) == 4 else base[:4]
    b = TorchBackend(synthetic=False, base_override=base,
                     voltage_coupling=coup, runtime_mode=mode)
    p = {"artificial_organs": [], "attachments": [],
         "n_extra_neurons": 0, "params": {}}
    if phen:
        p.update(phen)
    b.initialize(p, batch_size=replicates, seed=seed, device="cpu",
                 replicate_seeds=replicate_seeds(seed, replicates))
    if tel_nodes:
        b.set_telemetry(tel_nodes)
    b.set_inputs({"rates_hz": {str(i): drive_hz for i in drive_idx}})
    t0 = time.perf_counter()
    out = b.run(duration_ms)
    wall = time.perf_counter() - t0
    # analytic state memory (§46): runtime state tensors + edge tables.
    # ru_maxrss is a cumulative process max and cannot attribute deltas.
    state_bytes = sum(t.numel() * t.element_size() for t in
                      (b.v, b.g, b.delay_buf, b.refrac, b.spikes))
    if b._active_idx is not None:
        state_bytes += sum(t.numel() * t.element_size() for t in
                           (b.g_exc, b.g_inh, b.delay_buf_inh))
        state_bytes += sum(s.numel() * s.element_size()
                           for s in b._ch_state)
        state_bytes += sum(g.numel() * g.element_size()
                           for g in b._ch_gbar)
    edge_bytes = b.nnz * 12          # coo post/pre/val fp32+int64
    n_extra = b.n - b.n_base
    return {
        "state_mem_mb": (state_bytes + edge_bytes) / 1e6,
        "host_spikes": b.spike_counts[:, :b.n_base].sum(1).tolist(),
        "graft_spikes": (b.spike_counts[:, b.n_base:].sum(1).tolist()
                         if n_extra else [0] * replicates),
        "spikes_total": b.spike_counts.sum(1).tolist(),
        "wall_s": wall,
        "steps": int(duration_ms / b.params["dt"]),
        "telemetry": b.get_telemetry(),
        "numerical_failure": out.get("numerical_failure"),
    }


def _fidelity(params, phys, audit, n_nodes):
    """§42: PHYSIOLOGY_FIDELITY.json payload."""
    prov_count: dict[str, int] = {}
    for ep in params.values():
        for rec in ep["membrane"].values():
            p = str(rec["provenance"])
            prov_count[p] = prov_count.get(p, 0) + 1
        for ch in ep["channels"]:
            p = str(ch["provenance"])
            prov_count[p] = prov_count.get(p, 0) + 1
    ch_by = {}
    for ep in params.values():
        for ch in ep["channels"]:
            ch_by[ch["channel"]] = ch_by.get(ch["channel"], 0) + 1
    return {
        "entities_total": len(params),
        "entities_active_mode": len(params),
        "entities_passive_mode": 0,
        "compartments_active": len(phys["active_idx"]),
        "compartments_passive": n_nodes - len(phys["active_idx"]),
        "parameters_direct_measurement":
            prov_count.get("DIRECT_MEASUREMENT", 0),
        "parameters_cell_type_measurement":
            prov_count.get("CELL_TYPE_MEASUREMENT", 0),
        "parameters_literature_prior":
            prov_count.get("LITERATURE_PRIOR", 0),
        "parameters_model_inference":
            prov_count.get("MODEL_INFERENCE", 0),
        "parameters_generic_fallback":
            prov_count.get("GENERIC_FALLBACK", 0),
        "parameters_unknown": prov_count.get("UNKNOWN", 0),
        "channels_by_type": ch_by,
        "synapses_conductance_based": int(len(audit)),
        "synapses_weight_only": 0,
        "edges_unknown_nt": int((audit["esyn_provenance"]
                                 == "UNKNOWN").sum()),
    }


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
    ap.add_argument("--overlay-out", default=None)
    ap.add_argument("--fidelity-out", default=None)
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    t0 = time.time()
    swc_dir = Path("data/cache/banc_888/split_swc")
    (manifest, ent, circuit, drivers, targets, reductions,
     split_sub) = _build_circuit(args.store, args.n_circuit,
                                 args.seed, swc_dir)

    # physiology overlay for THIS circuit's entities only (§5)
    ov_dir = Path(args.overlay_out or "/tmp/a3_overlay")
    build_overlay(ent[ent["entity_idx"].isin(circuit)], manifest,
                  ov_dir)
    ov = load_overlay(ov_dir, manifest)
    entity_params = resolve_entity_params(ov)
    nt_map = {int(r.entity_idx): str(getattr(r, "nt_top", "") or "")
              for r in ent.itertuples(index=False)}

    # L3 graph
    (n3, post, pre, w, esyn, coup3, rows3, man3, audit3, phys) = \
        compile_reduced_graph_v3(split_sub, reductions, entity_params,
                                 nt_map=nt_map,
                                 weight_to_g=args.weight_to_g,
                                 drive_g=args.drive_g)
    # L2 graph (identical node allocation → same runtime indices)
    (n2, post2, pre2, w2, coup2, rows2, man2, _a2) = \
        C.compile_reduced_graph_v2(split_sub, reductions)
    assert rows2 == rows3, "v2/v3 node maps diverged"

    matched, comp_of = matched_targets(rows3, targets)
    soma_t = [comp_of[e]["SOMA"] for e in matched]
    dend_t = [comp_of[e]["DENDRITE_DIST"] for e in matched]
    in_src = [comp_of[d]["SOMA"] for d in drivers if d in comp_of]
    tel_nodes = dend_t + soma_t
    print(f"[a3] circuit {len(circuit)} entities -> {n3} nodes, "
          f"{len(w)} split edges; matched targets {len(matched)}")
    if not matched:
        print("[a3] FAIL: no matched targets")
        return 1

    # ---- §46 benchmark: L1 / L2 / L3, host only --------------------
    conn_sub = split_sub.groupby(["pre_idx", "post_idx"],
                                 as_index=False)["anatomical_count"]\
        .sum().rename(columns={"anatomical_count": "weight"})
    n1, p1, r1, w1, c1, rows1, man1 = C.compile_reduced_graph(
        conn_sub, {})
    soma1 = {r["entity_idx"]: r["runtime_idx"] for r in rows1}
    dr1 = [soma1[d] for d in drivers if d in soma1]
    dr23 = in_src

    bench = {}
    for name, base, coup, dr, mode, ph, tel in (
            ("L1_point", (n1, p1, r1, w1), c1, dr1, "passive_lif",
             None, ()),
            ("L2_passive", (n2, post2, pre2, w2), coup2, dr23,
             "passive_lif", None, tel_nodes),
            ("L3_active", (n3, post, pre, w, esyn), coup3, dr23,
             "active_hh_v0", {"physiology": phys}, tel_nodes)):
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
        print(f"[a3] {name}: wall={res['wall_s']:.2f}s "
              f"spikes={sum(res['host_spikes'])} "
              f"fail={res['numerical_failure']}")

    # whole-CNS estimate (§47): scale by entity count ratio
    n_ent_total = 204257
    scale = n_ent_total / max(len(circuit), 1)
    est = {"entities_circuit": len(circuit),
           "entities_whole_cns": n_ent_total,
           "note": "linear extrapolation — ESTIMATE ONLY",
           "per_level": {k: {
               "est_nodes": int(b["n_nodes"] * scale),
               "est_mem_mb": round(b["state_mem_mb"] * scale, 1),
               "est_step_cost_s_per_100ms":
                   round(b["wall_s"] * scale /
                         (args.duration_ms / 100.0), 2)}
               for k, b in bench.items()}}

    # ---- §33-35: graft on the L3 active host ------------------------
    base3 = (n3, post, pre, w, esyn)
    phen_full = {"physiology": phys, **_graft_phen(
        10, in_src, dend_t, args.weight_in, args.weight_out, 0.5)}
    graft = {}
    graft["full"] = _run(base3, coup3, phen_full, dr23, args.drive_hz,
                         args.duration_ms, args.seed, args.replicates,
                         "active_hh_v0", tel_nodes)
    graft["efferent_disabled"] = _run(
        base3, coup3,
        {"physiology": phys, **_graft_phen(
            10, in_src, dend_t, args.weight_in, args.weight_out, 0.5,
            enable_out=False)},
        dr23, args.drive_hz, args.duration_ms, args.seed,
        args.replicates, "active_hh_v0", tel_nodes)
    graft["afferent_disabled"] = _run(
        base3, coup3,
        {"physiology": phys, **_graft_phen(
            10, in_src, dend_t, args.weight_in, args.weight_out, 0.5,
            enable_in=False)},
        dr23, args.drive_hz, args.duration_ms, args.seed,
        args.replicates, "active_hh_v0", tel_nodes)
    graft["graft_disabled"] = _run(
        base3, coup3, {"physiology": phys}, dr23, args.drive_hz,
        args.duration_ms, args.seed, args.replicates, "active_hh_v0",
        tel_nodes)

    deltas = _tel_delta(graft["full"]["telemetry"],
                        graft["efferent_disabled"]["telemetry"])
    causal = _any_nonzero_delta(deltas)
    # paired determinism re-run
    again = _run(base3, coup3, phen_full, dr23, args.drive_hz,
                 args.duration_ms, args.seed, args.replicates,
                 "active_hh_v0", tel_nodes)
    deterministic = (
        again["host_spikes"] == graft["full"]["host_spikes"]
        and again["graft_spikes"] == graft["full"]["graft_spikes"])

    fidelity = _fidelity(entity_params, phys, audit3, n3)
    fidelity["numerical_failure_count"] = sum(
        1 for r in (*bench.values(), *graft.values())
        if r.get("numerical_failure"))
    if args.fidelity_out:
        Path(args.fidelity_out).write_text(json.dumps(fidelity,
                                                    indent=1))

    report = {
        "kind": "A3 active biophysics on G0.1 circuit",
        "runtime_fidelity": {"L1": "point-LIF", "L2": "passive reduced",
                             "L3": "active reduced"},
        "run_manifest": {
            "anatomy_manifest_hash": manifest.get("manifest_hash"),
            "physiology_overlay_hash":
                (ov["manifest"].get("files") or {}),
            "reduction_algorithm": man3["algorithm"],
            "runtime_fidelity": "L3",
            "dt": phys["integration"]["dt"],
            "temperature_C": phys["simulation_temperature_C"],
            "channel_model_versions": "chan-v0",
            "synapse_model_version": "conductance_v0",
            "unknown_nt_mode": phys["synapse"]["unknown_nt_mode"],
        },
        "matched_target_entities": matched,
        "matched_target_count": len(matched),
        "benchmark": bench,
        "whole_cns_estimate": est,
        "graft_conditions": {k: {kk: r[kk] for kk in
                                 ("host_spikes", "graft_spikes",
                                  "numerical_failure")}
                             for k, r in graft.items()},
        "graft_telemetry_deltas": deltas,
        "graft_causal": {
            "host_to_graft": all(s > 0 for s in
                                 graft["full"]["graft_spikes"]),
            "graft_to_target_compartment": bool(causal),
        },
        "deterministic": deterministic,
        "fidelity": fidelity,
        "v3_manifest": {k: man3[k] for k in
                        ("algorithm", "mapping_mode", "esyn_model",
                         "edges_unknown_nt", "fallback_fraction",
                         "runtime_edges_out", "coupling_model")},
        "wall_s_total": time.time() - t0,
    }
    out = Path(args.out or "/tmp/a3.json")
    out.write_text(json.dumps(report, indent=1, default=str))
    print(f"[a3] graft causal={causal} det={deterministic} "
          f"fail_count={fidelity['numerical_failure_count']}")
    print(f"[a3] report -> {out}")
    ok = causal and deterministic and \
        fidelity["numerical_failure_count"] == 0
    print(f"[a3] {'PASS' if ok else 'CHECK FAILED'}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
