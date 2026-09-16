"""ANATOMY_FIDELITY.json — the per-run honesty manifest (AFC §21).

Every run over an anatomical store emits one of these so that "how much
of this is real data and how much is model" is answerable from a single
file. Counters are computed from the canonical store, not estimated.
"""
from __future__ import annotations

import json
from pathlib import Path

from .provenance import Provenance


def fidelity_counts(neurons_df, conn_df, manifest: dict) -> dict:
    """Compute the §21 counters from canonical tables."""
    n = len(neurons_df)

    def populated(col: str) -> int:
        if col not in neurons_df.columns:
            return 0
        return int(neurons_df[col].notna().sum())

    return {
        "neurons_total": n,
        "neurons_with_dataset_id": populated("dataset_id"),
        "neurons_with_soma_coordinate":
            int(neurons_df[["soma_x", "soma_y", "soma_z"]]
                .notna().all(axis=1).sum())
            if {"soma_x", "soma_y", "soma_z"} <= set(neurons_df.columns)
            else 0,
        "neurons_with_cell_type": populated("cell_type"),
        "neurons_with_neuropil": populated("neuropil"),
        "neurons_with_flow_class": populated("flow_class"),
        "neurons_with_nt_annotation": populated("nt_top"),
        "neurons_with_morphology_ref": populated("morphology_ref"),
        "neurons_with_measured_membrane_params": 0,   # v0: none exist
        "neurons_with_inferred_membrane_params": 0,   # v0: none exist
        "neurons_with_channel_data": 0,               # v0: none exist
        "neurons_with_neuromodulator_annotation": 0,  # v0: none exist
        "chemical_synapses_imported": int(
            conn_df["anatomical_count"].sum())
            if "anatomical_count" in conn_df.columns else 0,
        "connections_imported": int(len(conn_df)),
        "connections_with_physiological_weight": int(
            conn_df["weight"].notna().sum())
            if "weight" in conn_df.columns else 0,
        "weight_provenance":
            str((manifest.get("connectivity_provenance") or {})
                .get("weight")),
        "known_gap_junctions": 0,   # whole-CNS gap data absent → UNKNOWN
        "gap_junction_status": Provenance.UNKNOWN.value,
    }


def write_fidelity(store_dir: str | Path, neurons_df, conn_df,
                   manifest: dict, out_path: str | Path | None = None
                   ) -> dict:
    fid = fidelity_counts(neurons_df, conn_df, manifest)
    doc = {
        "kind": "ANATOMY_FIDELITY",
        "store_version": manifest.get("store_version"),
        "dataset_kind": manifest.get("dataset_kind"),
        "counts": fid,
        "note": "exact vs inferred vs unknown per AFC §21; "
                "UNKNOWN fields are declared in manifest "
                "field_provenance and are never silently filled",
    }
    path = Path(out_path) if out_path else \
        Path(store_dir) / "ANATOMY_FIDELITY.json"
    path.write_text(json.dumps(doc, indent=1, default=str))
    return doc


# ---------------------------------------------------------------- A0.1 v2
def fidelity_counts_v2(entities_df, conn_df, split_df,
                       manifest: dict) -> dict:
    """§31 counters. Entity classes first — a store must never report a
    bare 'neurons' total again (§3)."""
    e = entities_df

    def populated(col: str) -> int:
        return int(e[col].notna().sum()) if col in e.columns else 0

    cls = (e["entity_class"].value_counts().to_dict()
           if "entity_class" in e.columns else {})
    has_soma = (int(e[["soma_x", "soma_y", "soma_z"]]
                    .notna().all(axis=1).sum())
                if {"soma_x", "soma_y", "soma_z"} <= set(e.columns) else 0)
    morph = e["morphology_status"].value_counts().to_dict() \
        if "morphology_status" in e.columns else {}
    valid = e["morphology_validation"].value_counts().to_dict() \
        if "morphology_validation" in e.columns else {}
    pre_c = int(conn_df["pre_compartment"].notna().sum()) \
        if "pre_compartment" in conn_df.columns else 0
    post_c = int(conn_df["post_compartment"].notna().sum()) \
        if "post_compartment" in conn_df.columns else 0
    return {
        "entities_total": int(len(e)),
        "biological_neurons": int(cls.get("BIOLOGICAL_NEURON", 0)),
        "rough_neurons": int(cls.get("ROUGH_NEURON", 0)),
        "orphan_fragments": int(cls.get("ORPHAN_FRAGMENT", 0)),
        "glia": int(cls.get("GLIA", 0)),
        "trachea": int(cls.get("TRACHEA", 0)),
        "other_cells": int(cls.get("OTHER_CELL", 0)),
        "unknown_segments": int(cls.get("UNKNOWN_SEGMENT", 0)),
        "entities_with_soma": has_soma,
        "entities_with_cell_type": populated("cell_type"),
        "entities_with_neuropil": populated("neuropil"),
        "entities_with_flow_class": populated("flow_class"),
        "entities_with_nt_prediction": populated("nt_top"),
        "entities_with_morphology": int(morph.get("AVAILABLE", 0)),
        "entities_with_valid_skeleton": int(valid.get("VALID", 0)),
        "entities_with_annotation_conflict":
            int(e["annotation_conflict"].sum())
            if "annotation_conflict" in e.columns else 0,
        "raw_synapse_records": int(len(split_df)) if split_df is not None
            else 0,
        "aggregated_connection_pairs": int(len(conn_df)),
        "aggregated_synapse_count": int(conn_df["anatomical_count"].sum())
            if "anatomical_count" in conn_df.columns else 0,
        "runtime_edges": int(len(conn_df)),
        "connections_with_pre_compartment": pre_c,
        "connections_with_post_compartment": post_c,
        "entities_with_direct_membrane_measurement": 0,
        "entities_with_inferred_membrane_parameters": 0,
        "entities_with_channel_data": 0,
        "entities_with_neuromodulator_annotation": 0,
        "known_gap_junctions": 0,
        "gap_junction_status": Provenance.UNKNOWN.value,
        "graft_synapses": 0,
    }


def write_fidelity_v2(store_dir: str | Path, entities_df, conn_df,
                      split_df, manifest: dict,
                      out_path: str | Path | None = None) -> dict:
    doc = {
        "kind": "ANATOMY_FIDELITY",
        "schema_version": 2,
        "store_version": manifest.get("store_version"),
        "dataset_identity": manifest.get("dataset"),
        "counts": fidelity_counts_v2(entities_df, conn_df, split_df,
                                     manifest),
        "terminology": {
            "entities_total": "canonical anatomical entities "
                              "(NOT a neuron count)",
            "aggregated_connection_pairs": "directed neuron-pair edges; "
                              "each represents anatomical_count chemical "
                              "synapses, not one synapse",
        },
        "note": "A0.1 §31 manifest — entity classes, layer separation "
                "and provenance breakdown; UNKNOWN is never 0-filled",
    }
    path = Path(out_path) if out_path else \
        Path(store_dir) / "ANATOMY_FIDELITY.json"
    path.write_text(json.dumps(doc, indent=1, default=str))
    return doc
