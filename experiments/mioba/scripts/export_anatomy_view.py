"""Export the Observatory anatomical view (AFC §18/§24).

Writes ``gui/static/anatomy.json`` so the Observatory can render:

* host anatomy at *real* EM soma coordinates (never schematic),
* graft neurons in their own ``graft:*`` namespace, visually separated,
* the actually-wired host→graft / graft→host edges, recovered from the
  backend's realized ``W_extra`` matrix — not re-sampled here,
* provenance per element (coordinates EXACT_EM; graft geometry
  MODEL_INFERENCE; missing host coords declared UNKNOWN).

Usage:
    python -m experiments.mioba.scripts.export_anatomy_view \
        --store experiments/mioba/datasets/banc_888/store \
        --out experiments/mioba/gui/static/anatomy.json
"""
from __future__ import annotations

import argparse
import json
import math
import random
from pathlib import Path

from ..anatomy import schema as A
from ..graft.schema import GraftSpec
from ..graft.build import compile_grafts


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--store", required=True)
    ap.add_argument("--out",
                    default="experiments/mioba/gui/static/anatomy.json")
    ap.add_argument("--seed", type=int, default=20260916)
    ap.add_argument("--host-sample", type=int, default=40000)
    ap.add_argument("--edge-cap", type=int, default=4000)
    args = ap.parse_args()

    manifest, neurons, _conn = A.load_store(args.store)
    spec = GraftSpec.g0_demo(seed=args.seed)
    phen = compile_grafts(spec, args.store)

    # --- realised graft wiring: build the backend and read W_extra back
    # as COO pairs. Slower than re-sampling, but this is the *actual*
    # wiring the run uses (AFC §18: show real synapse positions).
    from ..fba.torch_backend import TorchBackend
    backend = TorchBackend(anatomy_dir=args.store, synthetic=False,
                           propagation_backend="event_csc")
    backend.initialize(phen, batch_size=1, seed=args.seed,
                       device="cpu", replicate_seeds=[args.seed])
    n_base = backend.n_base
    edges_h2g, edges_g2h, edges_internal = [], [], []
    if backend.W_extra is not None:
        # CSC store -> (post, pre) COO pairs, the exact realised wiring
        ij = backend.W_extra.to_coo().coalesce().indices()
        rng = random.Random(args.seed)
        for post, pre in zip(ij[0].tolist(), ij[1].tolist()):
            if post >= n_base and pre < n_base:
                edges_h2g.append((pre, post))
            elif post < n_base and pre >= n_base:
                edges_g2h.append((pre, post))
            else:
                edges_internal.append((pre, post))
        for lst in (edges_h2g, edges_g2h, edges_internal):
            if len(lst) > args.edge_cap:
                lst[:] = sorted(rng.sample(lst, args.edge_cap))

    # --- host scatter at real coordinates (x,y soma nm → normalised)
    has = neurons[["soma_x", "soma_y"]].notna().all(axis=1)
    sub = neurons[has]
    if len(sub) > args.host_sample:
        sub = sub.sample(args.host_sample,
                         random_state=args.seed).sort_values("neuron_idx")
    xs = sub["soma_x"].astype(float).tolist()
    ys = sub["soma_y"].astype(float).tolist()
    xlo, xspan = min(xs), (max(xs) - min(xs)) or 1.0
    ylo, yspan = min(ys), (max(ys) - min(ys)) or 1.0
    classes = sorted(sub["flow_class"].fillna("unknown").unique())
    cls_id = {c: i for i, c in enumerate(classes)}
    # host occupies the left ~78% of the canvas; graft sits in a
    # labelled inset on the right so the two never visually mix (§19)
    def hx(i):
        x = float(neurons["soma_x"].iat[int(i)])
        return 0.02 + (x - xlo) / xspan * 0.76 if not math.isnan(x) else None
    def hy(i):
        y = float(neurons["soma_y"].iat[int(i)])
        return 0.97 - (y - ylo) / yspan * 0.94 if not math.isnan(y) else None
    host_pts = [[round(0.02 + (x - xlo) / xspan * 0.76, 4),
                 round(0.97 - (y - ylo) / yspan * 0.94, 4),
                 cls_id[c]]
                for x, y, c in zip(xs, ys,
                                   sub["flow_class"].fillna("unknown"))]

    # --- graft block: artificial coordinates, declared MODEL_INFERENCE
    graft_pts, organ_ranges = [], []
    n_extra = backend.n - n_base
    for gi in range(n_extra):
        a = 2 * math.pi * gi / max(1, n_extra)
        graft_pts.append([round(0.89 + 0.07 * math.cos(a), 4),
                          round(0.50 + 0.20 * math.sin(a), 4)])
    def pt(i):
        if i < n_base:
            x, y = hx(i), hy(i)
            return None if x is None else [round(x, 4), round(y, 4)]
        return graft_pts[i - n_base]

    def emit(lst):
        out = []
        for pre, post in lst:
            a, b = pt(pre), pt(post)
            if a and b:
                out.append([a[0], a[1], b[0], b[1]])
        return out

    doc = {
        "kind": "anatomy_view_v0",
        "dataset": backend.dataset_identity(),
        "host": {"points": host_pts, "flow_classes": classes,
                 "n_total": int(manifest["n_neurons"]),
                 "n_with_coords": int(has.sum()),
                 "n_sampled": len(host_pts),
                 "coord_provenance": "EXACT_EM",
                 "coord_units": "nm, normalised x/y projection"},
        "grafts": [{"graft_id": o["organ_id"], "n": int(o["size"]),
                    "points": graft_pts,
                    "coord_provenance": "MODEL_INFERENCE",
                    "note": "artificial lobe — own namespace, own layout"}
                   for o in phen["artificial_organs"]],
        "edges": {"host_to_graft": emit(edges_h2g),
                  "graft_to_host": emit(edges_g2h),
                  "graft_internal": emit(edges_internal)},
        "links": phen.get("graft_resolution"),
        "n_host_edges_total": int(manifest["n_connections"]),
    }
    Path(args.out).write_text(json.dumps(doc, separators=(",", ":")))
    print(f"[anatomy-view] {len(host_pts)} host pts, "
          f"h->g {len(edges_h2g)}, g->h {len(edges_g2h)}, "
          f"internal {len(edges_internal)} -> {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
