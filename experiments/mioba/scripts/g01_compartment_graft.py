"""G0.1 — compartment-aware graft on a real anatomical circuit (§26/§27).

Same 10-neuron graft under three conditions on the *same* reduced
host circuit:

    A  host only
    B  host + neuron-level graft        (links land on entity SOMA)
    C  host + compartment-aware graft   (out-link lands on DENDRITE_DIST
                                          compartments; in-link reads
                                          SOMA output — spikes emit
                                          from soma in this model)

Condition C is then ablated four ways under identical seeds:
    full / graft disabled / afferent(in-link) disabled /
    efferent(out-link) disabled
so host→graft, graft-internal and graft→host contributions can be
separated causally (§27).

Graft↔host synapses are ARTIFICIAL_GRAFT provenance throughout (§25);
the host graph is read-only (§24).
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
from ..anatomy import morphology as M
from ..graft.schema import GraftSpec
from ..graft.build import compile_grafts
from .a2_compartment_compare import _pick_circuit


def _build_circuit(store: str, n_circuit: int, seed: int,
                   swc_dir: Path):
    manifest, ent, conn = A.load_store_v2(store)
    circuit, drivers, targets = _pick_circuit(ent, conn, n_circuit, seed)
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
    sub = conn[conn["pre_idx"].isin(circuit)
               & conn["post_idx"].isin(circuit)].copy()
    return manifest, ent, conn, circuit, drivers, targets, \
        reductions, sub


def _run_host(base_override, coup, phenotype_extra, drive_idx, drive_hz,
              duration_ms, seed, replicates):
    from ..fba.torch_backend import TorchBackend
    from ..fba.replicates import replicate_seeds
    b = TorchBackend(synthetic=False, base_override=base_override,
                     voltage_coupling=coup)
    phen = {"artificial_organs": [], "attachments": [],
            "n_extra_neurons": 0, "params": {}}
    if phenotype_extra:
        phen.update(phenotype_extra)
    b.initialize(phen, batch_size=replicates, seed=seed, device="cpu",
                 replicate_seeds=replicate_seeds(seed, replicates))
    b.set_inputs({"rates_hz": {str(i): drive_hz for i in drive_idx}})
    out = b.run(duration_ms)
    s = b.get_state_summary()
    n_extra = b.n - b.n_base
    return {
        "spikes_total": s["per_batch_spike_counts"],
        "rate_hz": s["per_batch_mean_rate_hz"],
        "graft_spikes": (b.spike_counts[:, b.n_base:].sum(1).tolist()
                         if n_extra else [0] * replicates),
        "edge_events": (s.get("resource") or {}).get(
            "active_propagated_edges"),
        "graft_edge_events": getattr(b, "_act_edges_extra", None),
        "wall_s": out.get("wall_s"),
    }


def _graft_phen(graft_nodes: int, in_src: list[int], out_tgt: list[int],
                weight_in: float, weight_out: float, p: float,
                enable_in=True, enable_out=True) -> dict:
    """Direct phenotype construction on runtime-node indices: the graft
    block is appended after the host's n_base compartment nodes."""
    organs = [{"organ_id": "graft:g001", "size": graft_nodes,
               "kind": "graft", "internal_p": 0.15,
               "neuron_model": "lif"}]
    atts = []
    if enable_in:
        atts.append({"attachment_id": "g001:in0",
                     "direction": "forward", "weight_scale": weight_in,
                     "p": p, "source": "host:g001:in0",
                     "source_idx": in_src,
                     "target": "graft:g001",
                     "connection_provenance": "ARTIFICIAL_GRAFT"})
    if enable_out:
        atts.append({"attachment_id": "g001:out0",
                     "direction": "forward", "weight_scale": weight_out,
                     "p": p, "source": "graft:g001",
                     "target": "host:g001:out0", "target_idx": out_tgt,
                     "connection_provenance": "ARTIFICIAL_GRAFT"})
    return {"artificial_organs": organs, "attachments": atts,
            "n_extra_neurons": graft_nodes}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--store", required=True)
    ap.add_argument("--n-circuit", type=int, default=240)
    ap.add_argument("--duration-ms", type=float, default=300.0)
    ap.add_argument("--replicates", type=int, default=4)
    ap.add_argument("--seed", type=int, default=20260916)
    ap.add_argument("--drive-hz", type=float, default=30.0)
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    t0 = time.time()
    swc_dir = Path("data/cache/banc_888/split_swc")
    (manifest, ent, conn, circuit, drivers, targets, reductions,
     sub) = _build_circuit(args.store, args.n_circuit, args.seed,
                           swc_dir)
    print(f"[g01] circuit: {len(circuit)} entities, {len(sub)} intra "
          f"edges, {len(drivers)} drivers / {len(targets)} descending")

    (n_nodes, rpost, rpre, rw, coup, node_rows, red_manifest) = \
        C.compile_reduced_graph(sub, reductions)
    soma = {r["entity_idx"]: r["runtime_idx"] for r in node_rows
            if r["compartment"] == "SOMA"}
    ddist = {r["entity_idx"]: r["runtime_idx"] for r in node_rows
             if r["compartment"] == "DENDRITE_DIST"}
    dr = [soma[d] for d in drivers if d in soma]
    print(f"[g01] reduced host: {n_nodes} runtime nodes "
          f"({len(red_manifest['rows'])} compartment-expanded entities)")

    in_src = [soma[d] for d in drivers if d in soma]          # emit site
    out_soma = [soma[t] for t in targets if t in soma]        # B targets
    out_dend = [ddist[t] for t in targets if t in ddist]      # C targets
    print(f"[g01] targets: B soma={len(out_soma)} "
          f"C dendrite_dist={len(out_dend)}")

    res = {}
    base = (n_nodes, rpost, rpre, rw)
    # A: host only
    res["A_host_only"] = _run_host(base, coup, None, dr, args.drive_hz,
                                   args.duration_ms, args.seed,
                                   args.replicates)
    # B: neuron-level graft (entity semantics -> SOMA nodes)
    phen_b = _graft_phen(10, in_src, out_soma, 30.0, 80.0, 0.5)
    res["B_neuron_graft"] = _run_host(base, coup, phen_b, dr,
                                      args.drive_hz, args.duration_ms,
                                      args.seed, args.replicates)
    # C: compartment-aware graft (DENDRITE_DIST landing sites)
    phen_c = _graft_phen(10, in_src, out_dend, 30.0, 80.0, 0.5)
    res["C_compartment_graft"] = _run_host(base, coup, phen_c, dr,
                                           args.drive_hz,
                                           args.duration_ms, args.seed,
                                           args.replicates)
    print(f"[g01] A={res['A_host_only']['spikes_total']} "
          f"B={res['B_neuron_graft']['spikes_total']} "
          f"C={res['C_compartment_graft']['spikes_total']} "
          f"(B graft {res['B_neuron_graft']['graft_spikes']}, "
          f"C graft {res['C_compartment_graft']['graft_spikes']})")

    # §27 ablation on C
    abl = {}
    abl["full"] = res["C_compartment_graft"]
    abl["graft_disabled"] = _run_host(base, coup, None, dr,
                                      args.drive_hz, args.duration_ms,
                                      args.seed, args.replicates)
    abl["afferent_disabled"] = _run_host(
        base, coup, _graft_phen(10, in_src, out_dend, 30.0, 80.0, 0.5,
                                enable_in=False),
        dr, args.drive_hz, args.duration_ms, args.seed, args.replicates)
    abl["efferent_disabled"] = _run_host(
        base, coup, _graft_phen(10, in_src, out_dend, 30.0, 80.0, 0.5,
                                enable_out=False),
        dr, args.drive_hz, args.duration_ms, args.seed, args.replicates)

    # determinism on C
    again = _run_host(base, coup, phen_c, dr, args.drive_hz,
                      args.duration_ms, args.seed, args.replicates)
    deterministic = (again["spikes_total"]
                     == res["C_compartment_graft"]["spikes_total"])

    deltas = {k: [c - a for c, a in
                  zip(abl["full"]["spikes_total"], v["spikes_total"])]
              for k, v in abl.items() if k != "full"}
    report = {
        "kind": "G0.1 compartment-aware graft",
        "store": args.store, "dataset": manifest.get("dataset"),
        "seed": args.seed, "paired": True,
        "circuit": {"n_entities": len(circuit),
                    "n_runtime_nodes": n_nodes,
                    "n_intra_edges": int(len(sub)),
                    "drivers": drivers, "targets": targets,
                    "out_target_compartments": {
                        "B": ["SOMA"] * len(out_soma),
                        "C": ["DENDRITE_DIST"] * len(out_dend)}},
        "graft": {"n_neurons": 10,
                  "synapse_provenance": "ARTIFICIAL_GRAFT",
                  "weights": {"in": 30.0, "out": 80.0},
                  "weight_provenance": "MODEL_INFERENCE"},
        "conditions": res, "ablation": abl,
        "ablation_deltas_spikes": deltas,
        "deterministic": deterministic,
        "reduction_manifest_summary": {
            "entities_expanded": red_manifest["entities_expanded"],
            "algorithm": red_manifest["algorithm"],
            "coupling_model": red_manifest["coupling_model"]},
        "wall_s_total": time.time() - t0,
    }
    out = Path(args.out or "/tmp/g01.json")
    out.write_text(json.dumps(report, indent=1, default=str))
    print(f"[g01] determinism: {'PASS' if deterministic else 'FAIL'}")
    print(f"[g01] ablation Δspikes: "
          f"{ {k: v for k, v in deltas.items()} }")
    print(f"[g01] report -> {out}")
    ok = deterministic and res["B_neuron_graft"]["graft_spikes"][0] > 0 \
        and res["C_compartment_graft"]["graft_spikes"][0] > 0
    print(f"[g01] {'PASS' if ok else 'CHECK FAILED'}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
