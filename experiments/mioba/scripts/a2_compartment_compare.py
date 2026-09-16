"""A2 — point vs reduced-compartment runtime on the same circuit (§19).

Selects a real BANC subcircuit (driven afferents + their direct
partners + descending cells), fetches compartment-labelled split SWCs
for those entities, reduces each skeleton to runtime compartments, and
runs the identical drive through both graphs via the shared
TorchBackend event engine. Reports spikes / rate / propagated edges /
wall time / determinism for both modes (§19).

Usage:
    python -m experiments.mioba.scripts.a2_compartment_compare \
        --store experiments/mioba/datasets/banc_888/store_v2 \
        --n-circuit 240 --duration-ms 300
"""
from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

import numpy as np
import pandas as pd

from ..anatomy import schema as A
from ..anatomy import compartments as C
from ..anatomy import morphology as M


def _pick_circuit(ent, conn, n_circuit: int, seed: int):
    """A connected demo circuit: afferent drivers + descending cells +
    their strongest mutual partners, deterministic under ``seed``."""
    rng = np.random.default_rng(seed)
    aff = ent.loc[ent["flow_class"] == "afferent", "entity_idx"].to_numpy()
    drivers = sorted(rng.choice(aff, size=min(32, len(aff)),
                                replace=False).tolist())
    desc = ent.loc[ent["super_class"] == "descending",
                   "entity_idx"].to_numpy()
    targets = sorted(rng.choice(desc, size=min(32, len(desc)),
                                replace=False).tolist())
    core = set(drivers) | set(targets)
    # 1-hop partners of the core, strongest connections first
    part = conn[conn["pre_idx"].isin(core) | conn["post_idx"].isin(core)]
    part = part.sort_values("anatomical_count", ascending=False)
    ring = set(part["pre_idx"]).union(part["post_idx"]) - core
    circuit = sorted(core | set(list(ring)[: max(0, n_circuit - len(core))]))
    return circuit, drivers, targets


def _run(backend, drive_idx, drive_hz, duration_ms, seed, replicates):
    from ..fba.replicates import replicate_seeds
    backend.initialize({"artificial_organs": [], "attachments": [],
                        "n_extra_neurons": 0, "params": {}},
                       batch_size=replicates, seed=seed, device="cpu",
                       replicate_seeds=replicate_seeds(seed, replicates))
    backend.set_inputs({"rates_hz": {str(i): drive_hz
                                     for i in drive_idx}})
    out = backend.run(duration_ms)
    s = backend.get_state_summary()
    return {"spikes": s["per_batch_spike_counts"],
            "rate_hz": s["per_batch_mean_rate_hz"],
            "edge_events": (s.get("resource") or {}).get(
                "active_propagated_edges"),
            "wall_s": out.get("wall_s")}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--store", required=True)
    ap.add_argument("--n-circuit", type=int, default=240)
    ap.add_argument("--duration-ms", type=float, default=300.0)
    ap.add_argument("--replicates", type=int, default=2)
    ap.add_argument("--seed", type=int, default=20260916)
    ap.add_argument("--drive-hz", type=float, default=30.0)
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    manifest, ent, conn = A.load_store_v2(args.store)
    circuit, drivers, targets = _pick_circuit(ent, conn,
                                              args.n_circuit, args.seed)
    print(f"[a2] circuit: {len(circuit)} entities "
          f"({len(drivers)} afferent drivers, {len(targets)} descending)")

    # fetch labelled split SWCs for the circuit (per-file public URLs)
    swc_dir = Path("data/cache/banc_888/split_swc")
    rids = [str(ent.loc[ent["entity_idx"] == i, "root_id"].iloc[0])
            for i in circuit]
    n_got = M.fetch_split_swcs(rids, swc_dir)
    print(f"[a2] split SWCs available for {n_got}/{len(rids)} entities")

    # reductions for entities with labelled skeletons
    e2r = {int(e): str(r) for e, r in zip(ent["entity_idx"], ent["root_id"])}
    reductions = {}
    for ei in circuit:
        p = swc_dir / f"{e2r[ei]}_split.swc"
        if p.is_file() and p.stat().st_size > 0:
            nodes = M.parse_swc(p)
            reductions[ei] = C.reduce_entity(ei, nodes)
    n_labeled = sum(1 for r in reductions.values()
                    if len(r.compartments) > 1)
    print(f"[a2] reductions: {len(reductions)} entities, "
          f"{n_labeled} multi-compartment")

    sub = conn[conn["pre_idx"].isin(circuit)
               & conn["post_idx"].isin(circuit)].copy()
    print(f"[a2] intra-circuit edges: {len(sub)}")
    split = A.load_layer(args.store, "connections_split")
    cset = set(circuit)
    split_sub = split[split["pre_idx"].isin(cset)
                      & split["post_idx"].isin(cset)].copy()
    print(f"[a2] intra-circuit split edges: {len(split_sub)}")

    from ..fba.torch_backend import TorchBackend
    results = {}
    # --- point mode: one node per entity
    pmap = {e: i for i, e in enumerate(circuit)}
    pw = sub["weight"].to_numpy()
    post = sub["post_idx"].map(pmap).to_numpy()
    pre = sub["pre_idx"].map(pmap).to_numpy()
    b_pt = TorchBackend(synthetic=False,
                        base_override=(len(circuit), post, pre, pw))
    dr_pt = [pmap[d] for d in drivers if d in pmap]
    results["point"] = _run(b_pt, dr_pt, args.drive_hz,
                            args.duration_ms, args.seed, args.replicates)

    # --- reduced mode: compartment-expanded
    (n_nodes, rpost, rpre, rw, coup, node_rows, red_manifest) = \
        C.compile_reduced_graph(sub, reductions)
    soma_node = {r["entity_idx"]: r["runtime_idx"] for r in node_rows
                 if r["compartment"] == "SOMA"}
    dr_rd = [soma_node[d] for d in drivers if d in soma_node]
    b_rd = TorchBackend(synthetic=False,
                        base_override=(n_nodes, rpost, rpre, rw),
                        voltage_coupling=coup)
    results["reduced"] = _run(b_rd, dr_rd, args.drive_hz,
                              args.duration_ms, args.seed,
                              args.replicates)
    results["reduced"]["n_runtime_nodes"] = n_nodes

    # --- A2.1: reduce-v1-split-synapse — per-compartment edges from
    # connections_split, never collapsed onto a dominant label
    (n1, p1, q1, w1, coup1, rows1, man1, audit1) = \
        C.compile_reduced_graph_v2(split_sub, reductions)
    soma1 = {r["entity_idx"]: r["runtime_idx"] for r in rows1
             if r["compartment"] == "SOMA"}
    dr1 = [soma1[d] for d in drivers if d in soma1]
    b_v1 = TorchBackend(synthetic=False,
                        base_override=(n1, p1, q1, w1),
                        voltage_coupling=coup1)
    results["reduced_v1"] = _run(b_v1, dr1, args.drive_hz,
                                 args.duration_ms, args.seed,
                                 args.replicates)
    results["reduced_v1"]["n_runtime_nodes"] = n1
    results["reduced_v1"]["n_runtime_edges"] = int(len(w1))
    by_post = audit1.groupby("post_compartment").size().to_dict()
    results["reduced_v1"]["edges_by_post_compartment"] = \
        {str(k): int(v) for k, v in by_post.items()}

    # determinism: rerun reduced
    b_rd2 = TorchBackend(synthetic=False,
                         base_override=(n_nodes, rpost, rpre, rw),
                         voltage_coupling=coup)
    again = _run(b_rd2, dr_rd, args.drive_hz, args.duration_ms,
                 args.seed, args.replicates)
    deterministic = again["spikes"] == results["reduced"]["spikes"]
    b_v12 = TorchBackend(synthetic=False,
                         base_override=(n1, p1, q1, w1),
                         voltage_coupling=coup1)
    again_v1 = _run(b_v12, dr1, args.drive_hz, args.duration_ms,
                    args.seed, args.replicates)
    deterministic_v1 = \
        again_v1["spikes"] == results["reduced_v1"]["spikes"]

    report = {
        "kind": "A2 point-vs-reduced comparison",
        "store": args.store,
        "dataset": manifest.get("dataset"),
        "circuit": {"n_entities": len(circuit),
                    "n_intra_edges": int(len(sub)),
                    "n_entities_with_labels": n_labeled,
                    "n_runtime_nodes_reduced": n_nodes,
                    "drivers": drivers, "targets": targets},
        "reduction_manifest": red_manifest,
        "reduction_manifest_v1": man1,
        "point": results["point"], "reduced": results["reduced"],
        "reduced_v1": results["reduced_v1"],
        "deterministic_reduced": deterministic,
        "deterministic_reduced_v1": deterministic_v1,
        "notes": [
            "compartment labels: upstream split SWC "
            "(RAW_EM_DERIVED); reduction is reduce-v0",
            "coupling + soma->axon weights are MODEL_INFERENCE "
            "parameters, not measurements",
            "point mode drives entity SOMA nodes; reduced drives the "
            "same entities' SOMA compartments",
        ],
    }
    out = Path(args.out or "/tmp/a2_compare.json")
    out.write_text(json.dumps(report, indent=1, default=str))
    print(f"[a2] point  : spikes={results['point']['spikes']} "
          f"edges={results['point']['edge_events']} "
          f"wall={results['point']['wall_s']:.1f}s")
    print(f"[a2] reduced: spikes={results['reduced']['spikes']} "
          f"edges={results['reduced']['edge_events']} "
          f"wall={results['reduced']['wall_s']:.1f}s "
          f"nodes={n_nodes}")
    v1 = results["reduced_v1"]
    print(f"[a2] reduced-v1: spikes={v1['spikes']} "
          f"edges={v1['edge_events']} runtime_edges={v1['n_runtime_edges']} "
          f"wall={v1['wall_s']:.1f}s nodes={n1} "
          f"fallback={man1['split_edges_fallback_to_soma']}/"
          f"{man1['split_edges_total']}")
    print(f"[a2] determinism: v0={'PASS' if deterministic else 'FAIL'} "
          f"v1={'PASS' if deterministic_v1 else 'FAIL'}")
    print(f"[a2] report -> {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
