"""Compartment schema + reduction compiler (A2, directive §14–§18).

Canonical compartments come from the labelled ``*_split.swc`` files
(SWC type 1/7 → SOMA, 2 → AXON, 3 → DENDRITE, 4 → PRIMARY_NEURITE).
Reduction folds each entity's full skeleton into a small runtime set:

    SOMA  — spike initiation + output source
    DENDRITE_PROX / DENDRITE_DIST — input sites (proximal/distal split
        by path distance from soma)
    AXON  — spike output carrier

Entities without labelled morphology stay single-node — that is the
honest "no data" reduction, not a hidden guess. Every reduction emits a
manifest row (§18): original node count, runtime compartments, ratio,
mapping hash, algorithm version.

Runtime semantics: compartments of one entity are coupled by passive
voltage diffusion (``voltage_coupling`` sparse matrix applied as
dv_i ∝ Σ c_ij (v_j − v_i)), so dendritic inputs reach the soma without
needing the dendrite itself to spike. Outgoing edges always emit from
the SOMA node — the LIF reset is instantaneous, so diffusion could
never carry the spike pulse down an axon compartment; the AXON
compartment still exists as an *input* site (axo-axonic synapses are
real in BANC). All coupling weights are
MODEL_INFERENCE parameters recorded in the manifest.
"""
from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass, field

import numpy as np
import pandas as pd

REDUCTION_VERSION = "reduce-v0"
RUNTIME_COMPARTMENTS = ["SOMA", "DENDRITE_PROX", "DENDRITE_DIST", "AXON"]


@dataclass
class EntityReduction:
    entity_idx: int
    compartments: list[str]          # runtime compartments present
    original_nodes: int
    node_map: dict                   # swc node_id -> compartment name
    valid: bool
    issues: list = field(default_factory=list)

    @property
    def n_runtime(self) -> int:
        return max(1, len(self.compartments))

    def mapping_hash(self) -> str:
        return hashlib.sha256(json.dumps(
            {str(k): v for k, v in sorted(self.node_map.items())},
            sort_keys=True).encode()).hexdigest()[:12]

    def manifest_row(self) -> dict:
        return {"entity_idx": self.entity_idx,
                "original_swc_nodes": self.original_nodes,
                "runtime_compartments": len(self.compartments),
                "reduction_ratio": round(
                    self.original_nodes / self.n_runtime, 3),
                "mapping_hash": self.mapping_hash(),
                "algorithm": REDUCTION_VERSION,
                "valid": self.valid, "issues": self.issues}


def reduce_entity(entity_idx: int, nodes: np.ndarray) -> EntityReduction:
    """SWC node array (id,type,x,y,z,r,parent) → 4-compartment model.

    Compartment assignment is the upstream SWC label where present;
    dendrites are split proximal/distal by path distance from the soma
    (median split — a model choice, recorded in the manifest).
    Unlabelled skeletons collapse to a single SOMA node (UNKNOWN
    compartments are never invented)."""
    from .morphology import SWC_TYPE_TO_COMPARTMENT, validate_swc
    v = validate_swc(nodes)
    ids = nodes[:, 0].astype(int)
    types = nodes[:, 1].astype(int)
    comp_of_node = [SWC_TYPE_TO_COMPARTMENT.get(int(t), "UNKNOWN")
                    for t in types]
    labelled = [c for c in comp_of_node if c != "UNKNOWN"]
    if not labelled:
        return EntityReduction(entity_idx, ["SOMA"], len(nodes),
                               {int(i): "SOMA" for i in ids},
                               v["status"] == "VALID", v["issues"])

    # path distance from the soma/root along the tree (children map —
    # iterating the full parent map per node is O(n^2))
    parent = {int(i): int(p) for i, p in zip(ids, nodes[:, 6])}
    pos = {int(i): nodes[j, 2:5] for j, i in enumerate(ids)}
    children: dict[int, list[int]] = {}
    for i, p in parent.items():
        children.setdefault(p, []).append(i)
    root = next((int(i) for i, p in parent.items() if p < 0),
                int(ids[0]))
    soma_id = next((int(i) for i, c in zip(ids, comp_of_node)
                    if c == "SOMA"), root)
    dist = {soma_id: 0.0}
    order = [soma_id]
    for cur in order:                      # BFS over children
        for i in children.get(cur, ()):
            if i in dist:
                continue
            dist[i] = dist[cur] + float(
                np.linalg.norm(pos[i] - pos[cur]))
            order.append(i)
    dend_d = [dist[i] for i, c in zip(ids, comp_of_node)
              if c == "DENDRITE" and i in dist]
    med = float(np.median(dend_d)) if dend_d else 0.0

    node_map, comps = {}, set()
    for i, c in zip(ids, comp_of_node):
        if c == "SOMA":
            rc = "SOMA"
        elif c == "AXON":
            rc = "AXON"
        elif c in ("DENDRITE", "PRIMARY_NEURITE"):
            rc = ("DENDRITE_PROX" if dist.get(i, 0.0) <= med
                  else "DENDRITE_DIST")
        else:                              # UNKNOWN label → PROX
            rc = "DENDRITE_PROX"           # (model choice, documented)
        node_map[int(i)] = rc
        comps.add(rc)
    # guarantee the canonical chain exists even when a label is absent
    for rc in ("SOMA", "AXON"):
        comps.add(rc)
    order_out = [c for c in RUNTIME_COMPARTMENTS if c in comps]
    return EntityReduction(entity_idx, order_out, len(nodes), node_map,
                           v["status"] == "VALID", v["issues"])


def compile_reduced_graph(conn_df: pd.DataFrame,
                          reductions: dict[int, EntityReduction],
                          weight_col: str = "weight",
                          dendrite_target: str = "DENDRITE_DIST",
                          soma_axon_w: float = 15.0,
                          coupling_c: float = 1.0):
    """Build the compartment-expanded runtime graph.

    Returns ``(n_nodes, post, pre, w, coup, node_map_rows, manifest)``:

    * ``post/pre/w`` — inter-entity edges routed to compartment nodes:
      pre side emits from SOMA (spike initiation site — the LIF reset
      is instantaneous, so an axon compartment cannot carry spikes via
      diffusion); post side lands on the compartment matching the
      edge's ``post_compartment`` label (UNKNOWN → SOMA: bypasses
      filtering, recorded honestly).
    * ``coup`` — symmetric intra-entity diffusion coupling matrix as a
      (row, col, val) triple; diagonal entries carry −Σc.
    * ``node_map_rows`` — one row per runtime node
      ``(runtime_idx, entity_idx, compartment)`` for provenance.
    """
    node_of = {}                 # (entity, compartment) -> runtime idx
    node_map_rows = []

    def node(ei: int, comp: str) -> int:
        key = (ei, comp)
        if key not in node_of:
            node_of[key] = len(node_of)
            node_map_rows.append(
                {"runtime_idx": node_of[key], "entity_idx": ei,
                 "compartment": comp})
        return node_of[key]

    red_of = reductions
    def comp_list(ei):           # entity's compartments (point fallback)
        r = red_of.get(ei)
        return r.compartments if r else ["SOMA"]

    # allocate all nodes up-front in entity order for determinism
    for ei in sorted({*conn_df["pre_idx"], *conn_df["post_idx"],
                      *red_of.keys()}):
        for c in comp_list(ei):
            node(ei, c)

    # intra-entity coupling (passive diffusion chain)
    crow, ccol, cval = [], [], []
    post_l, pre_l, w_l = [], [], []
    coup_pairs = []
    for ei in sorted({k[0] for k in node_of}):
        cs = [c for c in comp_list(ei)]
        if len(cs) < 2:
            continue
        chain = [c for c in
                 ("AXON", "SOMA", "DENDRITE_PROX", "DENDRITE_DIST")
                 if c in cs]
        for a, b in zip(chain, chain[1:]):
            coup_pairs.append((node_of[(ei, a)], node_of[(ei, b)],
                               coupling_c))
    for i, j, c in coup_pairs:
        crow += [i, j, i, j]
        ccol += [j, i, i, j]
        cval += [c, c, -c, -c]

    # inter-entity edges routed through compartments
    for _, r in conn_df.iterrows():
        pe, qe = int(r["pre_idx"]), int(r["post_idx"])
        src = "SOMA"          # spike emitter (see module docstring)
        tgt_lbl = r.get("post_compartment") or "UNKNOWN"
        tgt = {"DENDRITE": dendrite_target,
               "PRIMARY_NEURITE": "DENDRITE_PROX",
               "AXON": "AXON", "SOMA": "SOMA"}.get(str(tgt_lbl), "SOMA")
        if tgt not in comp_list(qe):
            tgt = "SOMA"
        post_l.append(node_of[(qe, tgt)])
        pre_l.append(node_of[(pe, src)])
        w_l.append(float(r[weight_col]))

    man_rows = [red_of[e].manifest_row() for e in sorted(red_of)]
    manifest = {"algorithm": REDUCTION_VERSION,
                "dendrite_target": dendrite_target,
                "soma_axon_w": soma_axon_w,
                "coupling_c": coupling_c,
                "coupling_model": "passive voltage diffusion, "
                                  "MODEL_INFERENCE",
                "entities_expanded": len(man_rows),
                "rows": man_rows}
    return (len(node_of), np.asarray(post_l), np.asarray(pre_l),
            np.asarray(w_l), (np.asarray(crow), np.asarray(ccol),
                              np.asarray(cval)),
            node_map_rows, manifest)
