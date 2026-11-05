"""G0.1 — compartment-aware graft, causal validation (§12–§27, A2.1).

Runs on the reduce-v1-split-synapse runtime graph (split synapses are
never collapsed to a dominant compartment). Conditions:

    A  host only
    B  host + graft landing on SOMA            (same entity set as C)
    C  host + graft landing on DENDRITE_DIST   (same entity set as B)

§13–§15: the target entity set is IDENTICAL in B and C — only entities
provably carrying BOTH SOMA and DENDRITE_DIST are eligible
(matched_target_entities, asserted equal or FAIL). B and C differ only
in the efferent landing compartment. UNKNOWN / point-fallback entities
are never targets (§12).

§18–§21: graft→host causality is measured at the TARGET COMPARTMENT —
input event count, synaptic input sum, membrane voltage mean/peak —
compared full vs efferent_disabled. A non-zero paired delta on any of
them demonstrates the causal effect; soma/host-spike changes are
recorded but not required.

§23: ablations on C: full / graft_disabled / afferent_disabled /
efferent_disabled / internal_recurrence_disabled.
"""
from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path


from ..anatomy import schema as A
from ..anatomy import compartments as C
from ..anatomy import morphology as M
from .a2_compartment_compare import _pick_circuit


def _build_circuit(store: str, n_circuit: int, seed: int,
                   swc_dir: Path):
    manifest, ent, conn = A.load_store_v2(store)
    split = A.load_layer(store, "connections_split")
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
    cset = set(circuit)
    split_sub = split[split["pre_idx"].isin(cset)
                      & split["post_idx"].isin(cset)].copy()
    return (manifest, ent, circuit, drivers, targets, reductions,
            split_sub)


def matched_targets(node_rows, target_entities):
    """§13: entities having BOTH a SOMA and a DENDRITE_DIST runtime
    node — the only legal matched-target set for B vs C."""
    comp_of = {}
    for r in node_rows:
        comp_of.setdefault(r["entity_idx"], {})[r["compartment"]] = \
            r["runtime_idx"]
    out = [e for e in target_entities
           if "SOMA" in comp_of.get(e, {})
           and "DENDRITE_DIST" in comp_of.get(e, {})]
    return sorted(out), comp_of


def _run_host(base, coup, phen, drive_idx, drive_hz, duration_ms,
              seed, replicates, tel_nodes=()):
    from ..fba.torch_backend import TorchBackend
    from ..fba.replicates import replicate_seeds
    b = TorchBackend(synthetic=False, base_override=base,
                     voltage_coupling=coup)
    p = {"artificial_organs": [], "attachments": [],
         "n_extra_neurons": 0, "params": {}}
    if phen:
        p.update(phen)
    b.initialize(p, batch_size=replicates, seed=seed, device="cpu",
                 replicate_seeds=replicate_seeds(seed, replicates))
    if tel_nodes:
        b.set_telemetry(tel_nodes)
    b.set_inputs({"rates_hz": {str(i): drive_hz for i in drive_idx}})
    out = b.run(duration_ms)
    s = b.get_state_summary()
    n_extra = b.n - b.n_base
    host_spikes = b.spike_counts[:, :b.n_base].sum(1).tolist()
    return {
        "spikes_total": s["per_batch_spike_counts"],
        "host_spikes": host_spikes,
        "graft_spikes": (b.spike_counts[:, b.n_base:].sum(1).tolist()
                         if n_extra else [0] * replicates),
        "edge_events": (s.get("resource") or {}).get(
            "active_propagated_edges"),
        "graft_edge_events": getattr(b, "_act_edges_extra", None),
        "telemetry": b.get_telemetry(),
        "wall_s": out.get("wall_s"),
    }


def _graft_phen(graft_nodes, in_src, out_tgt, weight_in, weight_out,
                p, internal_p=0.15, enable_in=True, enable_out=True):
    organs = [{"organ_id": "graft:g001", "size": graft_nodes,
               "kind": "graft", "internal_p": internal_p,
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


def _tel_delta(full, ablated):
    """§20: per-target-compartment deltas between two telemetry sets."""
    if not full or not ablated:
        return None
    by_idx = {n["runtime_idx"]: n for n in ablated["nodes"]}
    deltas = []
    for n in full["nodes"]:
        m = by_idx.get(n["runtime_idx"])
        if m is None:
            continue
        deltas.append({
            "runtime_idx": n["runtime_idx"],
            "d_input_event_count": [a - b for a, b in zip(
                n["input_event_count"], m["input_event_count"])],
            "d_synaptic_input": [a - b for a, b in zip(
                n["synaptic_input_sum"], m["synaptic_input_sum"])],
            "d_voltage_mean": [a - b for a, b in zip(
                n["membrane_voltage_mean"], m["membrane_voltage_mean"])],
            "d_voltage_peak": [a - b for a, b in zip(
                n["membrane_voltage_peak"], m["membrane_voltage_peak"])],
        })
    return deltas


def _any_nonzero_delta(deltas):
    for d in deltas or []:
        for k in ("d_input_event_count", "d_synaptic_input",
                  "d_voltage_mean", "d_voltage_peak"):
            if any(abs(x) > 1e-9 for x in d[k]):
                return True
    return False


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--store", required=True)
    ap.add_argument("--n-circuit", type=int, default=240)
    ap.add_argument("--duration-ms", type=float, default=300.0)
    ap.add_argument("--replicates", type=int, default=8)
    ap.add_argument("--seed", type=int, default=20260916)
    ap.add_argument("--drive-hz", type=float, default=30.0)
    ap.add_argument("--weight-in", type=float, default=30.0)
    ap.add_argument("--weight-out", type=float, default=80.0)
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    t0 = time.time()
    swc_dir = Path("data/cache/banc_888/split_swc")
    (manifest, ent, circuit, drivers, targets, reductions,
     split_sub) = _build_circuit(args.store, args.n_circuit, args.seed,
                                 swc_dir)
    print(f"[g01] circuit: {len(circuit)} entities, "
          f"{len(split_sub)} split edges, "
          f"{len(drivers)} drivers / {len(targets)} descending")

    (n_nodes, rpost, rpre, rw, coup, node_rows, red_manifest,
     _audit) = C.compile_reduced_graph_v2(split_sub, reductions)
    comp_of = {r["entity_idx"]: {} for r in node_rows}
    for r in node_rows:
        comp_of[r["entity_idx"]][r["compartment"]] = r["runtime_idx"]
    matched, comp_of = matched_targets(node_rows, targets)
    soma_t = [comp_of[e]["SOMA"] for e in matched]
    dend_t = [comp_of[e]["DENDRITE_DIST"] for e in matched]
    print(f"[g01] reduced-v1 host: {n_nodes} nodes, "
          f"{len(rw)} runtime edges "
          f"(fallback {red_manifest['split_edges_fallback_to_soma']}/"
          f"{red_manifest['split_edges_total']})")
    print(f"[g01] matched targets (SOMA+DENDRITE_DIST): {len(matched)}")
    if not matched:
        print("[g01] FAIL: no matched target entities")
        return 1

    in_src = [comp_of[d]["SOMA"] for d in drivers if d in comp_of]
    # §19 telemetry: the DENDRITE_DIST landing compartments + their
    # somas (soma deltas are recorded, not required — §21)
    tel_nodes = dend_t + soma_t
    base = (n_nodes, rpost, rpre, rw)
    dr = in_src
    wi, wo = args.weight_in, args.weight_out

    res = {}
    res["A_host_only"] = _run_host(base, coup, None, dr, args.drive_hz,
                                   args.duration_ms, args.seed,
                                   args.replicates, tel_nodes)
    res["B_soma_graft"] = _run_host(
        base, coup, _graft_phen(10, in_src, soma_t, wi, wo, 0.5),
        dr, args.drive_hz, args.duration_ms, args.seed,
        args.replicates, tel_nodes)
    phen_c = _graft_phen(10, in_src, dend_t, wi, wo, 0.5)
    res["C_dendrite_graft"] = _run_host(
        base, coup, phen_c, dr, args.drive_hz, args.duration_ms,
        args.seed, args.replicates, tel_nodes)

    abl = {"full": res["C_dendrite_graft"]}
    abl["graft_disabled"] = _run_host(
        base, coup, None, dr, args.drive_hz, args.duration_ms,
        args.seed, args.replicates, tel_nodes)
    abl["afferent_disabled"] = _run_host(
        base, coup, _graft_phen(10, in_src, dend_t, wi, wo, 0.5,
                                enable_in=False),
        dr, args.drive_hz, args.duration_ms, args.seed,
        args.replicates, tel_nodes)
    abl["efferent_disabled"] = _run_host(
        base, coup, _graft_phen(10, in_src, dend_t, wi, wo, 0.5,
                                enable_out=False),
        dr, args.drive_hz, args.duration_ms, args.seed,
        args.replicates, tel_nodes)
    abl["internal_recurrence_disabled"] = _run_host(
        base, coup, _graft_phen(10, in_src, dend_t, wi, wo, 0.5,
                                internal_p=0.0),
        dr, args.drive_hz, args.duration_ms, args.seed,
        args.replicates, tel_nodes)

    deltas = _tel_delta(abl["full"]["telemetry"],
                        abl["efferent_disabled"]["telemetry"])
    causal = _any_nonzero_delta(deltas)

    again = _run_host(base, coup, phen_c, dr, args.drive_hz,
                      args.duration_ms, args.seed, args.replicates,
                      tel_nodes)
    deterministic = (
        again["spikes_total"] == res["C_dendrite_graft"]["spikes_total"]
        and again["graft_spikes"]
            == res["C_dendrite_graft"]["graft_spikes"]
        and json.dumps(again["telemetry"], sort_keys=True)
            == json.dumps(res["C_dendrite_graft"]["telemetry"],
                          sort_keys=True))

    host_to_graft = all(s > 0 for s in
                        res["C_dendrite_graft"]["graft_spikes"])
    ok = host_to_graft and causal and deterministic

    report = {
        "kind": "G0.1 compartment-aware graft (A2.1 hardening)",
        "store": args.store, "dataset": manifest.get("dataset"),
        "seed": args.seed, "paired": True,
        "replicates": args.replicates,
        "graph_algorithm": red_manifest["algorithm"],
        "mapping_mode": red_manifest["mapping_mode"],
        "circuit": {"n_entities": len(circuit),
                    "n_runtime_nodes": n_nodes,
                    "n_split_edges": int(len(split_sub)),
                    "n_runtime_edges": int(len(rw)),
                    "drivers": drivers,
                    "fallback_fraction":
                        red_manifest["fallback_fraction"]},
        "matched_target_entities": matched,
        "matched_target_count": len(matched),
        "matched_target_compartments": {"B": "SOMA",
                                        "C": "DENDRITE_DIST"},
        "graft": {"n_neurons": 10,
                  "synapse_provenance": "ARTIFICIAL_GRAFT",
                  "weights": {"in": wi, "out": wo},
                  "weight_provenance": "MODEL_INFERENCE"},
        "conditions": res, "ablation": abl,
        "telemetry_deltas_full_vs_efferent": deltas,
        "causal": {"host_to_graft": bool(host_to_graft),
                   "graft_to_target_compartment": bool(causal),
                   "graft_to_soma": bool(_any_nonzero_delta([
                       d for d in (deltas or [])
                       if d["runtime_idx"] in set(soma_t)])),
                   "graft_to_host_spike_output": any(
                       c != a for c, a in zip(
                           abl["full"]["host_spikes"],
                           abl["efferent_disabled"]["host_spikes"]))},
        "deterministic": deterministic,
        "reduction_manifest": {k: red_manifest[k] for k in
                               ("algorithm", "mapping_mode",
                                "split_edges_total",
                                "split_edges_exact_compartment",
                                "split_edges_unknown_compartment",
                                "split_edges_fallback_to_soma",
                                "fallback_fraction",
                                "compartment_reduction")},
        "wall_s_total": time.time() - t0,
    }
    out = Path(args.out or "/tmp/g01.json")
    out.write_text(json.dumps(report, indent=1, default=str))
    c_, b_, a_ = (res["C_dendrite_graft"], res["B_soma_graft"],
                  res["A_host_only"])
    print(f"[g01] A host={a_['host_spikes']} "
          f"B host={b_['host_spikes']} C host={c_['host_spikes']}")
    print(f"[g01] graft spikes B={b_['graft_spikes']} "
          f"C={c_['graft_spikes']}")
    print(f"[g01] causal: host->graft {host_to_graft} | "
          f"graft->compartment {causal} | determinism {deterministic}")
    print(f"[g01] report -> {out}")
    print(f"[g01] {'PASS' if ok else 'CHECK FAILED'}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
