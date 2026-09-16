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
from ..anatomy import morphology as M
from ..graft.schema import GraftSpec
from ..graft.build import compile_grafts

#: fixed compartment palette order for the Observatory legend (§28)
COMPARTMENTS = ["SOMA", "AXON", "DENDRITE", "PRIMARY_NEURITE", "UNKNOWN"]
_SWC_COMP = {1: "SOMA", 7: "SOMA", 2: "AXON", 3: "DENDRITE",
             4: "PRIMARY_NEURITE"}


def _load_any(store):
    """v2 stores get the richer loader; v1 keeps working unchanged."""
    try:
        return A.load_store_v2(store)
    except Exception:
        return A.load_store(store)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--store", required=True)
    ap.add_argument("--out",
                    default="experiments/mioba/gui/static/anatomy.json")
    ap.add_argument("--seed", type=int, default=20260916)
    ap.add_argument("--host-sample", type=int, default=40000)
    ap.add_argument("--edge-cap", type=int, default=4000)
    ap.add_argument("--skeleton-sample", type=int, default=120)
    ap.add_argument("--swc-dir",
                    default="data/cache/banc_888/split_swc")
    ap.add_argument("--physiology", default=None,
                    help="physiology overlay dir (A3 §44 layer)")
    args = ap.parse_args()

    manifest, neurons, conn = _load_any(args.store)
    idx_col = ("entity_idx" if "entity_idx" in neurons.columns
               else "neuron_idx")
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
                         random_state=args.seed).sort_values(idx_col)
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

    # --- §28: skeletons for graft-linked entities + a small sample.
    # Drawn at real EM coordinates (same nm space as the soma scatter —
    # never schematic). Only entities with labelled split SWCs appear;
    # absence is visible (no invented morphology).
    def nxy(x, y):
        return (round(0.02 + (float(x) - xlo) / xspan * 0.76, 4),
                round(0.97 - (float(y) - ylo) / yspan * 0.94, 4))

    graft_hosts = {i for att in phen["attachments"]
                   for i in (att.get("source_idx")
                             or att.get("target_idx") or [])}
    rng = random.Random(args.seed)
    sk_sample = set(graft_hosts)
    pool = neurons[has][idx_col].tolist()
    sk_sample |= set(rng.sample(pool, min(args.skeleton_sample,
                                          len(pool))))
    swc_dir = Path(args.swc_dir)
    if "root_id" in neurons.columns:
        e2r = {int(e): str(r) for e, r in
               zip(neurons[idx_col], neurons["root_id"])}
        M.fetch_split_swcs([e2r[e] for e in sk_sample if e in e2r],
                           swc_dir)
    else:
        e2r = {}
    comp_id = {c: i for i, c in enumerate(COMPARTMENTS)}
    skeletons = []
    max_segs = 400                 # per-entity cap (browser budget —
                                   # thinning is declared, not hidden)
    for ei in sorted(sk_sample):
        rid = e2r.get(ei)
        p = swc_dir / f"{rid}_split.swc" if rid else None
        if not p or not p.is_file() or p.stat().st_size == 0:
            continue
        nd = M.parse_swc(p)
        # keep every k-th node; connect consecutive kept nodes in file
        # order (SWC files walk the tree, so this stays a real sub-path
        # of the skeleton, not a schematic)
        stride = max(1, len(nd) // max_segs)
        keep = nd[::stride]
        pos = {int(i): nxy(x, y)
               for i, x, y in zip(keep[:, 0], keep[:, 2], keep[:, 3])}
        segs, comps = [], []
        prev = None
        for i, t in zip(keep[:, 0], keep[:, 1]):
            cur = pos[int(i)]
            if prev is not None:
                segs.append([*prev, *cur])
                comps.append(comp_id[_SWC_COMP.get(int(t), "UNKNOWN")])
            prev = cur
        if segs:
            skeletons.append({"e": int(ei), "s": segs, "c": comps,
                              "stride": int(stride),
                              "in_graft_link": ei in graft_hosts})

    # --- native synapse layer (§28): aggregated pairs among the
    # skeleton-displayed entities, capped — labelled "aggregated
    # directed connection pairs", never "individual synapses".
    sk_set = {s["e"] for s in skeletons}
    nat = []
    if {"pre_idx", "post_idx"} <= set(conn.columns):
        nc = conn[conn["pre_idx"].isin(sk_set)
                  & conn["post_idx"].isin(sk_set)]
        if len(nc) > args.edge_cap:
            nc = nc.sample(args.edge_cap, random_state=args.seed)
        for _, r in nc.iterrows():
            a, b2 = pt(int(r["pre_idx"])), pt(int(r["post_idx"]))
            if a and b2:
                nat.append([a[0], a[1], b2[0], b2[1]])

    # A3 §44: optional physiology overlay layer — per-entity profile,
    # channel set, and per-parameter provenance so the GUI can colour
    # measured/inferred/fallback/unknown distinctly. Live voltage is
    # not in this static export; the layer carries *mode + provenance*.
    phys_block = None
    if args.physiology:
        from ..physio.overlay import (load_overlay,
                                      resolve_entity_params)
        ov = load_overlay(args.physiology)
        ep_all = resolve_entity_params(ov)
        # only plotted entities need physiology in the static export —
        # all 204k rows would dwarf the anatomy payload
        keep = {int(i) for i in sub[idx_col]} | {
            int(s["e"]) for s in skeletons}
        ep = {k: v for k, v in ep_all.items() if k in keep}
        phys_block = {
            "overlay_version": ov["manifest"]["overlay_version"],
            "overlay_dir": str(args.physiology),
            "provenance_classes": [
                "DIRECT_MEASUREMENT", "CELL_TYPE_MEASUREMENT",
                "TRANSCRIPTOMIC_INFERENCE", "LITERATURE_PRIOR",
                "MODEL_INFERENCE", "GENERIC_FALLBACK", "UNKNOWN"],
            "per_entity": {str(k): {
                "profile": v["profile_id"],
                "assignment_provenance":
                    str(v["assignment_provenance"]),
                "runtime_mode": ("active" if any(
                    c["channel"] != "leak" for c in v["channels"])
                    else "passive"),
                "channels": [c["channel"] for c in v["channels"]],
                "membrane_provenance": {
                    p: str(r["provenance"])
                    for p, r in v["membrane"].items()}}
                for k, v in ep.items()},
        }

    doc = {
        "kind": "anatomy_view_v1",
        "dataset": backend.dataset_identity(),
        "host": {"points": host_pts, "flow_classes": classes,
                 # parallel to points — entity_idx per plotted soma so
                 # the physiology layer can join on real entity ids
                 "entity_idx": [int(i) for i in sub[idx_col]],
                 "n_total": int(manifest.get("n_neurons")
                                or manifest.get("n_entities", 0)),
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
                  "graft_internal": emit(edges_internal),
                  "native_pairs": nat},
        "skeletons": {"entities": skeletons,
                      "compartments": COMPARTMENTS,
                      "coord_provenance": "RAW_EM_DERIVED",
                      "thinning": "every k-th node kept (stride per "
                                  "entity); a real sub-path of the "
                                  "skeleton, not a schematic",
                      "note": "labelled split SWCs only; entities "
                              "without them show soma point only"},
        "physiology": phys_block,
        "links": phen.get("graft_resolution"),
        "n_host_edges_total": int(
            manifest.get("n_connections")
            or (manifest.get("connectivity") or {}).get(
                "aggregated_connection_pairs", 0)),
    }
    Path(args.out).write_text(json.dumps(doc, separators=(",", ":")))
    print(f"[anatomy-view] {len(host_pts)} host pts, "
          f"h->g {len(edges_h2g)}, g->h {len(edges_g2h)}, "
          f"internal {len(edges_internal)} -> {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
