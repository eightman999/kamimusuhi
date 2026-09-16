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
    unknown_swc_nodes: int = 0           # §31 audit
    unknown_nodes_fallback_mapped: int = 0

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
                "unknown_swc_nodes": self.unknown_swc_nodes,
                "unknown_nodes_fallback_mapped":
                    self.unknown_nodes_fallback_mapped,
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
    n_unknown = sum(1 for c in comp_of_node if c == "UNKNOWN")
    labelled = [c for c in comp_of_node if c != "UNKNOWN"]
    if not labelled:
        return EntityReduction(entity_idx, ["SOMA"], len(nodes),
                               {int(i): "SOMA" for i in ids},
                               v["status"] == "VALID", v["issues"],
                               unknown_swc_nodes=n_unknown,
                               unknown_nodes_fallback_mapped=0)

    # path distance from the soma along the tree. §27–28: the SWC tree
    # is traversed UNDIRECTED from the soma — nodes on the parent side
    # of a non-root soma must get distances too (v0 bug fix: previously
    # only soma children were walked, silently dropping parent-side
    # branches into dist=0).
    parent = {int(i): int(p) for i, p in zip(ids, nodes[:, 6])}
    pos = {int(i): nodes[j, 2:5] for j, i in enumerate(ids)}
    adj: dict[int, list[int]] = {}
    for i, p in parent.items():
        adj.setdefault(i, [])
        if p >= 0 and p in parent:
            adj[i].append(p)
            adj.setdefault(p, []).append(i)
    root = next((int(i) for i, p in parent.items() if p < 0),
                int(ids[0]))
    soma_id = next((int(i) for i, c in zip(ids, comp_of_node)
                    if c == "SOMA"), root)
    dist = {soma_id: 0.0}
    order = [soma_id]
    for cur in order:                      # BFS over undirected tree
        for i in adj.get(cur, ()):
            if i in dist:
                continue
            dist[i] = dist[cur] + float(
                np.linalg.norm(pos[i] - pos[cur]))
            order.append(i)
    dend_d = [dist[i] for i, c in zip(ids, comp_of_node)
              if c == "DENDRITE" and i in dist]
    med = float(np.median(dend_d)) if dend_d else 0.0

    node_map, comps = {}, set()
    n_unk_fallback = 0
    for i, c in zip(ids, comp_of_node):
        if c == "SOMA":
            rc = "SOMA"
        elif c == "AXON":
            rc = "AXON"
        elif c in ("DENDRITE", "PRIMARY_NEURITE"):
            rc = ("DENDRITE_PROX" if dist.get(i, 0.0) <= med
                  else "DENDRITE_DIST")
        else:                              # UNKNOWN label → PROX
            rc = "DENDRITE_PROX"           # recorded fallback (§30):
            n_unk_fallback += 1            # NOT "was really proximal"
        node_map[int(i)] = rc
        comps.add(rc)
    # guarantee the canonical chain exists even when a label is absent
    for rc in ("SOMA", "AXON"):
        comps.add(rc)
    order_out = [c for c in RUNTIME_COMPARTMENTS if c in comps]
    return EntityReduction(entity_idx, order_out, len(nodes), node_map,
                           v["status"] == "VALID", v["issues"],
                           unknown_swc_nodes=n_unknown,
                           unknown_nodes_fallback_mapped=n_unk_fallback)


def compile_reduced_graph(conn_df: pd.DataFrame,
                          reductions: dict[int, EntityReduction],
                          weight_col: str = "weight",
                          dendrite_target: str = "DENDRITE_DIST",
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
                "coupling_c": coupling_c,
                "coupling_model": "passive voltage diffusion, "
                                  "MODEL_INFERENCE",
                "entities_expanded": len(man_rows),
                "rows": man_rows}
    return (len(node_of), np.asarray(post_l), np.asarray(pre_l),
            np.asarray(w_l), (np.asarray(crow), np.asarray(ccol),
                              np.asarray(cval)),
            node_map_rows, manifest)


REDUCTION_VERSION_V1 = "reduce-v1-split-synapse"

#: anatomical post_compartment -> runtime compartment (A2.1 §1/§3).
#: PRIMARY_NEURITE is folded into the dendritic chain — recorded in the
#: manifest, never claimed to exist as a runtime compartment.
_POST_TO_RUNTIME = {"SOMA": "SOMA", "AXON": "AXON",
                    "DENDRITE": "DENDRITE_DIST",
                    "PRIMARY_NEURITE": "DENDRITE_PROX"}

COMPARTMENT_REDUCTION_NOTE = {
    "PRIMARY_NEURITE": {"runtime_mapping": "DENDRITE_PROX_OR_DIST",
                        "provenance": "MODEL_INFERENCE"}}


def compile_reduced_graph_v2(split_df: pd.DataFrame,
                             reductions: dict[int, EntityReduction],
                             mapping_mode: str = "permissive",
                             coupling_c: float = 1.0,
                             weight_div: float = 32.0):
    """Split-synapse runtime graph (A2.1 §2–§10).

    Primary input is ``connections_split`` — one row per
    (pre, post, pre_compartment, post_compartment, anatomical_count).
    Each compartment combination becomes its OWN runtime edge; nothing
    is collapsed onto a dominant label (§2). ``weight =
    anatomical_count / weight_div`` per split edge (§6).

    Returns ``(n_nodes, post, pre, w, coup, node_map_rows, manifest,
    edge_audit)`` where ``edge_audit`` is a DataFrame with the §5
    record fields (pre/post entity+compartment, anatomical_count,
    runtime_weight, weight_provenance, fallback flags).

    ``mapping_mode`` (§11): ``permissive`` routes UNKNOWN/absent
    compartments to SOMA and records the fallback; ``strict`` skips
    them and counts them under ``split_edges_skipped_strict``.
    """
    if mapping_mode not in ("permissive", "strict"):
        raise ValueError(f"mapping_mode {mapping_mode!r}")

    node_of, node_map_rows = {}, []

    def node(ei: int, comp: str) -> int:
        key = (ei, comp)
        if key not in node_of:
            node_of[key] = len(node_of)
            node_map_rows.append(
                {"runtime_idx": node_of[key], "entity_idx": ei,
                 "compartment": comp})
        return node_of[key]

    def comp_list(ei):
        r = reductions.get(ei)
        return r.compartments if r else ["SOMA"]

    entities = sorted({*split_df["pre_idx"], *split_df["post_idx"],
                       *reductions.keys()})
    for ei in entities:
        for c in comp_list(ei):
            node(ei, c)

    # intra-entity passive coupling chain (same as reduce-v0)
    crow, ccol, cval = [], [], []
    for ei in entities:
        cs = comp_list(ei)
        chain = [c for c in
                 ("AXON", "SOMA", "DENDRITE_PROX", "DENDRITE_DIST")
                 if c in cs]
        for a, b in zip(chain, chain[1:]):
            i, j = node_of[(ei, a)], node_of[(ei, b)]
            crow += [i, j, i, j]
            ccol += [j, i, i, j]
            cval += [coupling_c, coupling_c,
                     -coupling_c, -coupling_c]

    post_l, pre_l, w_l = [], [], []
    audit = {k: [] for k in
             ("pre_entity", "post_entity", "pre_compartment",
              "post_compartment", "anatomical_count", "runtime_weight",
              "weight_provenance", "fallback_used", "fallback_reason",
              "mapping_provenance")}
    n_exact = n_unknown = n_fallback = n_skipped = 0
    for _, r in split_df.iterrows():
        pe, qe = int(r["pre_idx"]), int(r["post_idx"])
        anat_pre = str(r.get("pre_compartment") or "UNKNOWN")
        anat_post = str(r.get("post_compartment") or "UNKNOWN")
        tgt = _POST_TO_RUNTIME.get(anat_post)
        fallback = False
        reason = ""
        if tgt is None:                       # UNKNOWN post compartment
            n_unknown += 1
            if mapping_mode == "strict":
                n_skipped += 1
                continue
            tgt, fallback, reason = "SOMA", True, \
                "UNKNOWN_POST_COMPARTMENT"
            n_fallback += 1
        elif tgt not in comp_list(qe):        # entity lacks the label
            if mapping_mode == "strict":
                n_skipped += 1
                continue
            tgt, fallback, reason = "SOMA", True, \
                "COMPARTMENT_ABSENT"
            n_fallback += 1
        else:
            n_exact += 1
        wv = float(r["anatomical_count"]) / weight_div
        post_l.append(node_of[(qe, tgt)])
        pre_l.append(node_of[(pe, "SOMA")])   # §7: emitter stays SOMA
        w_l.append(wv)
        for k, v_ in (("pre_entity", pe), ("post_entity", qe),
                      ("pre_compartment", anat_pre),
                      ("post_compartment", anat_post),
                      ("anatomical_count", int(r["anatomical_count"])),
                      ("runtime_weight", wv),
                      ("weight_provenance", "MODEL_INFERENCE"),
                      ("fallback_used", fallback),
                      ("fallback_reason", reason),
                      ("mapping_provenance",
                       "MODEL_INFERENCE" if fallback
                       else "RAW_EM_DERIVED")):
            audit[k].append(v_)

    total = int(len(split_df))
    man_rows = [reductions[e].manifest_row() for e in sorted(reductions)]
    manifest = {
        "algorithm": REDUCTION_VERSION_V1,
        "input_layer": "connections_split",
        "mapping_mode": mapping_mode,
        "weight_rule": f"weight = anatomical_count / {weight_div}",
        "weight_provenance": "MODEL_INFERENCE",
        "compartment_reduction": COMPARTMENT_REDUCTION_NOTE,
        "presynaptic_note": "anatomical pre_compartment is recorded; "
                            "runtime spike emission is always SOMA (§7)",
        "coupling_c": coupling_c,
        "coupling_model": "passive voltage diffusion, MODEL_INFERENCE",
        "split_edges_total": total,
        "split_edges_exact_compartment": n_exact,
        "split_edges_unknown_compartment": n_unknown,
        "split_edges_fallback_to_soma": n_fallback,
        "split_edges_skipped_strict": n_skipped,
        "fallback_fraction": round(n_fallback / total, 6)
            if total else 0.0,
        "runtime_edges_out": len(w_l),
        "entities_expanded": len(man_rows),
        "rows": man_rows,
    }
    return (len(node_of), np.asarray(post_l), np.asarray(pre_l),
            np.asarray(w_l), (np.asarray(crow), np.asarray(ccol),
                              np.asarray(cval)),
            node_map_rows, manifest, pd.DataFrame(audit))
