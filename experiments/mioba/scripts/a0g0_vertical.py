"""A0/G0 vertical slice — real BANC host + fixed 10-neuron graft.

End-to-end check of the Anatomical Fly Core acceptance criteria
(AFC §24):

* host graph comes from the canonical store — real imported anatomy,
  no synthetic random edges;
* a fixed 10-neuron graft is wired onto anatomically selected host
  populations (in: central_brain sample; out: descending cells);
* host→graft and graft→host propagation are measured;
* ``graft enabled`` vs ``graft disabled`` run under identical seeds —
  paired ablation, the same convention as M1.5;
* determinism: the same spec+seed run twice must give identical
  spike counts.

Usage:
    python -m experiments.mioba.scripts.a0g0_vertical \
        --store experiments/mioba/datasets/banc_888/store \
        --duration-ms 500 --replicates 4
"""
from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

from ..anatomy import schema as A
from ..fba.replicates import replicate_seeds
from ..graft.schema import GraftSpec, Graft
from ..graft.build import compile_grafts


def _afferent_indices(neurons, max_targets=256, seed=0,
                      must_include=()):
    """Sensory afferents — the honest 'sensory neuron' entry point of a
    host CNS (flow=afferent in BANC meta). ``must_include`` cells are
    kept in the drive set so a graft tapping the sensory stream is
    actually stimulated (AFC §11 sensory port)."""
    import random
    idx = [int(i) for i in neurons["neuron_idx"]
           [neurons["flow_class"].fillna("") == "afferent"].tolist()]
    include = [i for i in must_include if i in set(idx)]
    rest = [i for i in idx if i not in set(include)]
    rng = random.Random(seed)
    if len(rest) > max_targets - len(include):
        rest = sorted(rng.sample(rest, max_targets - len(include)))
    return sorted(include + rest)


def _run_once(store_dir: str, spec: GraftSpec, seed: int,
              duration_ms: float, replicates: int,
              drive_idx: list[int], drive_hz: float) -> dict:
    from ..fba.torch_backend import TorchBackend
    from ..graft.build import compile_grafts

    backend = TorchBackend(anatomy_dir=store_dir, synthetic=False,
                           propagation_backend="event_csc")
    phenotype = compile_grafts(spec, store_dir)
    seeds = replicate_seeds(seed, replicates)
    backend.initialize(phenotype, batch_size=replicates, seed=seed,
                       device="cpu", replicate_seeds=seeds)
    # drive real afferents — the same stimulus on every lane
    backend.set_inputs({"rates_hz": {str(i): drive_hz
                                     for i in drive_idx}})
    run_out = backend.run(duration_ms)
    s = backend.get_state_summary()
    acts = backend.get_population_activity(
        ["all", "fba0"] + [f"organ:{o['organ_id']}"
                           for o in phenotype["artificial_organs"]])
    graft_ranges = [(o["organ_id"], backend.n - backend.n_extra
                     + sum(x["size"] for x in
                           phenotype["artificial_organs"]
                           [:phenotype["artificial_organs"].index(o)]),
                     0) for o in phenotype["artificial_organs"]]
    # graft per-neuron spikes (they are the trailing block)
    n_extra = backend.n - backend.n_base
    graft_spikes = (backend.spike_counts[:, backend.n_base:]
                    .sum(1).tolist() if n_extra else [0] * replicates)
    return {
        "seed": seed, "replicates": replicates,
        "duration_ms": duration_ms,
        "n_base": backend.n_base, "n_total": backend.n,
        "n_graft_neurons": n_extra,
        "per_lane_spikes_total": s["per_batch_spike_counts"],
        "per_lane_rate_hz": s["per_batch_mean_rate_hz"],
        "per_lane_graft_spikes": graft_spikes,
        "activity": acts,
        "propagated_edge_events": (s.get("resource") or {}).get(
            "active_propagated_edges"),
        "wall_s": run_out.get("wall_s"),
        "simulated_ms": s.get("t_ms"),
        "guard": s.get("guard"),
        "dataset_identity": backend.dataset_identity(),
        "graft_resolution": phenotype.get("graft_resolution"),
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--store", required=True)
    ap.add_argument("--duration-ms", type=float, default=500.0)
    ap.add_argument("--replicates", type=int, default=4)
    ap.add_argument("--seed", type=int, default=20260916)
    ap.add_argument("--drive-hz", type=float, default=30.0)
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    manifest, neurons, _c = A.load_store(args.store)
    print(f"[a0g0] host: {manifest['dataset_kind']} "
          f"{manifest['n_neurons']} neurons / "
          f"{manifest['n_connections']} connections")

    spec_on = GraftSpec.g0_demo(seed=args.seed)
    # Compile once up-front so the drive set covers the graft's actual
    # host targets — otherwise a 64-of-~12k afferent subsample is
    # almost surely disjoint from the cells we stimulate.
    phen = compile_grafts(spec_on, args.store)
    in_targets = sorted({i for a in phen["attachments"]
                         for i in a.get("source_idx", [])
                         if str(a["attachment_id"]).endswith(":in0")})
    drive_idx = _afferent_indices(neurons, seed=args.seed,
                                  must_include=in_targets)
    print(f"[a0g0] driving {len(drive_idx)} afferent sensory neurons "
          f"@ {args.drive_hz} Hz "
          f"(includes {len(in_targets)} graft-monitored cells)")
    spec_off = GraftSpec(grafts=[
        Graft(graft_id=g.graft_id, n_neurons=g.n_neurons,
              enabled=False, links=[])
        for g in spec_on.grafts])

    t0 = time.time()
    full = _run_once(args.store, spec_on, args.seed, args.duration_ms,
                     args.replicates, drive_idx, args.drive_hz)
    print(f"[a0g0] full: host+graft wall={full['wall_s']:.1f}s "
          f"edges={full['propagated_edge_events']} "
          f"graft_spikes={full['per_lane_graft_spikes']}")
    abl = _run_once(args.store, spec_off, args.seed, args.duration_ms,
                    args.replicates, drive_idx, args.drive_hz)
    print(f"[a0g0] ablated: host only wall={abl['wall_s']:.1f}s "
          f"edges={abl['propagated_edge_events']}")

    # determinism: identical rerun must reproduce exactly
    again = _run_once(args.store, spec_on, args.seed, args.duration_ms,
                      args.replicates, drive_idx, args.drive_hz)
    deterministic = (again["per_lane_spikes_total"]
                     == full["per_lane_spikes_total"])
    print(f"[a0g0] determinism check: "
          f"{'PASS' if deterministic else 'FAIL'}")

    deltas = [f - a for f, a in zip(full["per_lane_rate_hz"],
                                    abl["per_lane_rate_hz"])]
    deltas_fba0 = [f - a for f, a in
                   zip(full["activity"]["fba0"], abl["activity"]["fba0"])]
    report = {
        "kind": "A0/G0 vertical slice",
        "store": args.store,
        "dataset_identity": full["dataset_identity"],
        "seed": args.seed,
        "paired": True,
        "full": full, "ablated": abl,
        "delta_rate_hz_per_lane": deltas,
        # graft's own 25Hz cells inflate the 'all' mean — the host-only
        # delta isolates graft→host influence (AFC: activity ≠ function)
        "delta_fba0_rate_hz_per_lane": deltas_fba0,
        "graft_received_drive": any(x > 0
                                    for x in full["per_lane_graft_spikes"]),
        "deterministic": deterministic,
        "wall_s_total": time.time() - t0,
    }
    out = Path(args.out or
               "/tmp/a0g0_vertical.json")
    out.write_text(json.dumps(report, indent=1, default=str))
    print(f"[a0g0] report -> {out}")
    # acceptance-criteria verdicts
    ok = (full["dataset_identity"]["dataset_id"] == "banc_888"
          and full["n_graft_neurons"] == 10
          and report["graft_received_drive"]
          and deterministic)
    print(f"[a0g0] {'PASS' if ok else 'CHECK FAILED'}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
