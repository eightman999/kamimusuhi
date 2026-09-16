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
