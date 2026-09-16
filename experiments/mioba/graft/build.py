"""Compile a GraftSpec into the runtime phenotype representation.

Resolution happens here — against the canonical anatomy store — so the
backend only ever sees plain index lists (AFC §3: no dataset knowledge
below this layer). Every attachment records which selector produced it
and how many host cells it resolved to; a selector that resolves to
zero host cells is an error, never silently rewired.
"""
from __future__ import annotations

import random
from pathlib import Path

import pandas as pd

from ..anatomy import schema as A
from ..graft.namespaces import host_id, parse_host_id
from ..graft.schema import GraftSpec, HostSelector


def _entity_idx_col(neurons: pd.DataFrame) -> str:
    """v2 stores name the canonical index ``entity_idx``; v1 called it
    ``neuron_idx``. Accept either (A0.1 migration window)."""
    return "entity_idx" if "entity_idx" in neurons.columns \
        else "neuron_idx"


#: G0.1 §23 validity: which compartments a link may use on the host
#: side. A host cell can only *emit* through axonal/somatic output —
#: requiring a dendritic-only source is the anatomically invalid case
#: the directive calls out. UNKNOWN handling is mode-dependent.
_HOST_SIDE_COMPARTMENTS = {
    "in": {"DENDRITE", "PRIMARY_NEURITE"},      # forbidden as a source
    "out": set(),                              # all input sites allowed
    "bidirectional": {"DENDRITE", "PRIMARY_NEURITE"},
}


def resolve_selector(sel: HostSelector, neurons: pd.DataFrame,
                     dataset_prefix: str, direction: str | None = None,
                     split_df: pd.DataFrame | None = None,
                     strict: bool = True) -> list[int]:
    """Selector → sorted canonical index list (then deterministic
    subsample to ``max_targets``). ``compartment_type`` narrows
    candidates to entities that provably carry that compartment on the
    relevant side of their split-edgelist connections — data-grounded,
    never assumed (G0.1 §21)."""
    if direction and sel.compartment_type:
        bad = _HOST_SIDE_COMPARTMENTS.get(direction, set())
        if sel.compartment_type in bad:
            msg = (f"{direction}-link cannot target compartment "
                   f"{sel.compartment_type}: anatomically invalid as a "
                   f"host-side source (§23)")
            if strict:
                raise ValueError(msg)
            import warnings
            warnings.warn(msg + " — allowed in permissive mode")

    if sel.dataset_ids:
        idx_col = _entity_idx_col(neurons)
        rid_to_idx = {did: i for did, i in
                      zip(neurons["dataset_id"].tolist(),
                          neurons[idx_col].tolist())}
        out = []
        for did in sel.dataset_ids:
            parse_host_id(did)          # validates + rejects graft ids
            if did in rid_to_idx:
                out.append(rid_to_idx[did])
        missing = len(sel.dataset_ids) - len(out)
        if missing:
            raise ValueError(f"{missing} dataset_ids not found in store")
        return sorted(int(i) for i in out)

    idx_col = _entity_idx_col(neurons)
    mask = pd.Series(True, index=neurons.index)
    used = {}
    if sel.neuropil is not None:
        mask &= neurons["neuropil"].fillna("") == sel.neuropil
        used["neuropil"] = sel.neuropil
    if sel.cell_type is not None:
        mask &= neurons["cell_type"].fillna("") == sel.cell_type
        used["cell_type"] = sel.cell_type
    if sel.flow_class is not None:
        mask &= neurons["flow_class"].fillna("") == sel.flow_class
        used["flow_class"] = sel.flow_class
    if sel.super_class is not None:
        mask &= neurons["super_class"].fillna("") == sel.super_class
        used["super_class"] = sel.super_class
    if not used:
        raise ValueError("HostSelector has no criteria")
    idx = sorted(int(i) for i in neurons[idx_col][mask].tolist())

    if sel.compartment_type and direction and split_df is not None:
        col = ("pre_idx" if direction == "in" else "post_idx")
        ccol = ("pre_compartment" if direction == "in"
                else "post_compartment")
        have = set(split_df.loc[split_df[ccol] == sel.compartment_type,
                                col].tolist())
        idx = [i for i in idx if i in have]
        used["compartment_type"] = sel.compartment_type

    if not idx:
        raise ValueError(f"selector {used} resolved to zero host cells")
    if len(idx) > sel.max_targets:
        idx = sorted(random.Random(sel.seed).sample(idx, sel.max_targets))
    return idx


def compile_grafts(spec: GraftSpec, store_dir: str | Path) -> dict:
    """Return a phenotype dict the backend can initialize() directly:
    artificial_organs + attachments (+ substrate identity). Each link
    carries its own edge probability (``GraftLink.p``); compartment-aware
    selectors are resolved against the store's split labels."""
    manifest, neurons, _conn = A.load_store(store_dir)
    ds = manifest.get("dataset") or {}
    ds_prefix = (ds.get("kind") or manifest.get("dataset_kind")
                 or "banc_888").split("_")[0]
    split_df = A.load_layer(store_dir, "connections_split")

    organs, attachments, resolved = [], [], []
    for g in spec.grafts:
        if not g.enabled:
            continue                      # non-destructive ablation
        organs.append({
            "organ_id": g.graft_uid(),
            "size": int(g.n_neurons),
            "kind": "graft",
            "internal_p": g.internal_p
                if g.internal_topology == "recurrent" else 0.0,
            "neuron_model": g.neuron_model.type,
            "provenance": g.provenance,
        })
        for link in g.links:
            host_idx = resolve_selector(
                link.host, neurons, ds_prefix, direction=link.direction,
                split_df=split_df, strict=link.strict)
            resolved.append({"link_id": link.link_id,
                             "direction": link.direction,
                             "compartment_type":
                                 link.host.compartment_type,
                             "n_host": len(host_idx)})
            base = {
                "attachment_id": link.link_id,
                "direction": "forward",     # explicit legs only
                "weight_scale": link.weight,
                "p": link.p,
                # §25: host<->graft synapses are ARTIFICIAL_GRAFT —
                # never presented as anatomical data
                "connection_provenance": link.provenance,
                "resolution": {
                    "n_host": len(host_idx),
                    "selector": {k: v for k, v in
                                 vars(link.host).items() if v},
                    "targeting_mode": link.targeting_mode,
                    "compartment_type": link.host.compartment_type,
                    "provenance": link.provenance,
                },
            }
            if link.direction == "in":
                # host population -> graft lobe
                attachments.append({**base,
                                    "source": f"host:{link.link_id}",
                                    "source_idx": host_idx,
                                    "target": g.graft_uid()})
            elif link.direction == "out":
                # graft lobe -> host population
                attachments.append({**base,
                                    "source": g.graft_uid(),
                                    "target": f"host:{link.link_id}",
                                    "target_idx": host_idx})
            elif link.direction == "bidirectional":
                # one bundle, both legs (backend adds the reverse leg)
                attachments.append({**base,
                                    "direction": "bidirectional",
                                    "source": f"host:{link.link_id}",
                                    "source_idx": host_idx,
                                    "target": g.graft_uid()})
            else:
                raise ValueError(f"link {link.link_id}: bad direction "
                                 f"{link.direction!r}")
    return {
        "substrates": [{"substrate_id": "banc0",
                        "kind": manifest.get("dataset_kind")}],
        "n_extra_neurons": sum(o["size"] for o in organs),
        "artificial_organs": organs,
        "attachments": attachments,
        "graft_resolution": resolved,
        "params": {},
    }
